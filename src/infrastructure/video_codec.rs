use crate::infrastructure::gstreamer_util::ensure_gst_init;
use crossbeam_channel::{bounded, Receiver, TryRecvError};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use gstreamer_video as gst_video;
use tracing::{info, warn};

/// Detect the best available H264 decoder plugin at runtime.
/// Priority: nvv4l2decoder (Jetson GPU) > avdec_h264 (libav software).
/// Returns an error if no suitable decoder is found.
fn detect_h264_decoder() -> Result<&'static str, String> {
    let registry = gst::Registry::get();

    // Priority 1: Jetson NVDEC hardware decoder
    if registry
        .find_feature("nvv4l2decoder", gst::ElementFactory::static_type())
        .is_some()
    {
        return Ok("nvv4l2decoder");
    }

    // Priority 2: GStreamer libav software decoder
    if registry
        .find_feature("avdec_h264", gst::ElementFactory::static_type())
        .is_some()
    {
        return Ok("avdec_h264");
    }

    Err("No H264 decoder plugin found in GStreamer. \
         Install gst-plugins-bad (avdec_h264) or run on Jetson (nvv4l2decoder)."
        .to_string())
}

pub struct H264Decoder {
    pipeline: gst::Pipeline,
    appsrc: gst_app::AppSrc,
    frame_rx: Receiver<DecodedFrame>,
    annexb_buffer: Vec<u8>,
    decoder_name: String,
}

// SAFETY: H264Decoder is Send because:
// 1. GStreamer elements (Pipeline, AppSrc) are internally thread-safe via GObject refcounting
// 2. crossbeam Receiver is Send
// 3. Vec buffers are owned by this struct
// 4. This decoder is ONLY used within a single tokio task (spawn_video_handler) and never
//    shared across threads - it's a single-owner pattern.
// 5. The GStreamer pipeline's internal threads are managed by GStreamer itself.
unsafe impl Send for H264Decoder {}

impl H264Decoder {
    pub fn new() -> Result<Self, String> {
        ensure_gst_init()?;

        let decoder_name = detect_h264_decoder()?;
        info!(
            event = "video_decoder_init",
            backend = "gstreamer",
            decoder = decoder_name,
            "GStreamer H264 decoder selected"
        );

        let pipeline_str = format!(
            "appsrc name=src is-live=true do-timestamp=true format=time block=false \
             caps=video/x-h264,stream-format=byte-stream,alignment=nal \
             ! h264parse \
             ! {decoder_name} \
             ! videoconvert \
             ! video/x-raw,format=BGR \
             ! appsink name=sink emit-signals=false sync=false \
               max-buffers=2 drop=true"
        );

        let pipeline = gst::parse::launch(&pipeline_str)
            .map_err(|e| format!("Failed to create GStreamer pipeline: {e}"))?
            .downcast::<gst::Pipeline>()
            .map_err(|_| "Pipeline element is not a Pipeline".to_string())?;

        let appsrc = pipeline
            .by_name("src")
            .ok_or_else(|| "appsrc element not found in pipeline".to_string())?
            .downcast::<gst_app::AppSrc>()
            .map_err(|_| "Element 'src' is not an AppSrc".to_string())?;

        let appsink = pipeline
            .by_name("sink")
            .ok_or_else(|| "appsink element not found in pipeline".to_string())?
            .downcast::<gst_app::AppSink>()
            .map_err(|_| "Element 'sink' is not an AppSink".to_string())?;

        // Callback-based frame delivery: GStreamer streaming thread pushes
        // decoded frames into a bounded channel. decode() polls non-blocking.
        let (frame_tx, frame_rx) = bounded::<DecodedFrame>(4);

        appsink.set_callbacks(
            gst_app::AppSinkCallbacks::builder()
                .new_sample(move |sink| {
                    let sample = sink.pull_sample().map_err(|_| gst::FlowError::Error)?;

                    let buffer = sample.buffer().ok_or(gst::FlowError::Error)?;
                    let caps = sample.caps().ok_or(gst::FlowError::Error)?;

                    let video_info =
                        gst_video::VideoInfo::from_caps(caps).map_err(|_| gst::FlowError::Error)?;

                    let width = video_info.width();
                    let height = video_info.height();

                    let map = buffer.map_readable().map_err(|_| gst::FlowError::Error)?;

                    let data = map.as_slice().to_vec();

                    let frame = DecodedFrame {
                        data,
                        width,
                        height,
                    };

                    // Non-blocking send; drop frame if consumer is slow
                    let _ = frame_tx.try_send(frame);

                    Ok(gst::FlowSuccess::Ok)
                })
                .build(),
        );

        pipeline
            .set_state(gst::State::Playing)
            .map_err(|e| format!("Failed to set pipeline to Playing state: {e:?}"))?;

        let annexb_buffer = Vec::with_capacity(65536);

        Ok(Self {
            pipeline,
            appsrc,
            frame_rx,
            annexb_buffer,
            decoder_name: decoder_name.to_string(),
        })
    }

    pub fn decode(&mut self, nal_data: &[u8]) -> Result<Option<DecodedFrame>, String> {
        if nal_data.is_empty() {
            return Ok(None);
        }

        // Build Annex B byte-stream: 0x00000001 + NAL unit
        self.annexb_buffer.clear();
        self.annexb_buffer
            .extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        self.annexb_buffer.extend_from_slice(nal_data);

        // Push buffer into appsrc (non-blocking)
        let mut gst_buffer = gst::Buffer::with_size(self.annexb_buffer.len())
            .map_err(|e| format!("Failed to allocate GStreamer buffer: {e}"))?;
        {
            let buffer_ref = gst_buffer.get_mut().ok_or("Failed to get mutable buffer")?;
            let mut map = buffer_ref
                .map_writable()
                .map_err(|e| format!("Failed to map buffer for writing: {e}"))?;
            map.copy_from_slice(&self.annexb_buffer);
        }

        self.appsrc
            .push_buffer(gst_buffer)
            .map_err(|e| format!("Failed to push buffer to appsrc: {e:?}"))?;

        // Non-blocking: check if GStreamer has delivered a decoded frame
        match self.frame_rx.try_recv() {
            Ok(frame) => Ok(Some(frame)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => {
                Err("GStreamer decoder frame channel disconnected".to_string())
            }
        }
    }

    pub fn decoder_name(&self) -> &str {
        &self.decoder_name
    }
}

impl Drop for H264Decoder {
    fn drop(&mut self) {
        if let Err(e) = self.pipeline.set_state(gst::State::Null) {
            warn!(
                event = "gstreamer_pipeline_stop_failed",
                error = ?e,
                "Failed to stop GStreamer pipeline on drop"
            );
        }
    }
}

pub struct DecodedFrame {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gstreamer_init() {
        assert!(ensure_gst_init().is_ok(), "GStreamer init failed");
        let version = gst::version_string();
        println!("GStreamer version: {version}");
        assert!(!version.is_empty());
    }

    #[test]
    fn test_decoder_detection() {
        assert!(ensure_gst_init().is_ok(), "GStreamer init failed");
        let result = detect_h264_decoder();
        assert!(
            result.is_ok(),
            "No H264 decoder found. Install gst-plugins-bad."
        );
        let name = result.unwrap();
        println!("Detected H264 decoder: {name}");
        assert!(name == "nvv4l2decoder" || name == "avdec_h264");
    }

    #[test]
    fn test_pipeline_creation() {
        assert!(ensure_gst_init().is_ok(), "GStreamer init failed");
        let decoder = H264Decoder::new();
        assert!(
            decoder.is_ok(),
            "Failed to create H264Decoder: {:?}",
            decoder.err()
        );
        let decoder = decoder.unwrap();
        let state = decoder.pipeline.current_state();
        assert!(
            state == gst::State::Playing || state == gst::State::Paused,
            "Pipeline in unexpected state: {state:?}"
        );
        println!("Pipeline created with decoder: {}", decoder.decoder_name());
    }

    #[test]
    fn test_decode_empty_data_returns_none() {
        assert!(ensure_gst_init().is_ok(), "GStreamer init failed");
        let mut decoder = H264Decoder::new().expect("Failed to create decoder");
        let result = decoder.decode(&[]);
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }
}
