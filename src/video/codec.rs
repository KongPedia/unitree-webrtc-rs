use crate::infrastructure::gstreamer_util::ensure_gst_init;
use crossbeam_channel::{bounded, Receiver, TryRecvError};
use gst_video::prelude::*;
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use gstreamer_video as gst_video;
use std::env;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tracing::{debug, warn};

/// Detect the best available H264 decoder plugin at runtime.
/// Priority:
///   1. explicit `UNITREE_WEBRTC_H264_DECODER`
///   2. auto-detect: nvv4l2decoder (Jetson GPU) > avdec_h264 (libav software)
///
/// Returns an error if no suitable decoder is found.
fn detect_h264_decoder() -> Result<&'static str, String> {
    let registry = gst::Registry::get();
    let requested = env::var("UNITREE_WEBRTC_H264_DECODER")
        .unwrap_or_else(|_| "auto".to_string())
        .trim()
        .to_ascii_lowercase();

    let has_nvv4l2 = registry
        .find_feature("nvv4l2decoder", gst::ElementFactory::static_type())
        .is_some();
    let has_avdec = registry
        .find_feature("avdec_h264", gst::ElementFactory::static_type())
        .is_some();

    match requested.as_str() {
        "" | "auto" => {}
        "nvv4l2decoder" => {
            if has_nvv4l2 {
                return Ok("nvv4l2decoder");
            }
            return Err(
                "UNITREE_WEBRTC_H264_DECODER=nvv4l2decoder was requested, but the plugin is not available."
                    .to_string(),
            );
        }
        "avdec_h264" => {
            if has_avdec {
                return Ok("avdec_h264");
            }
            return Err(
                "UNITREE_WEBRTC_H264_DECODER=avdec_h264 was requested, but the plugin is not available."
                    .to_string(),
            );
        }
        other => {
            return Err(
                format!(
                    "Unsupported UNITREE_WEBRTC_H264_DECODER='{other}'. Use auto, nvv4l2decoder, or avdec_h264."
                ),
            );
        }
    }

    // Priority 1: Jetson NVDEC hardware decoder
    if has_nvv4l2 {
        return Ok("nvv4l2decoder");
    }

    // Priority 2: GStreamer libav software decoder
    if has_avdec {
        return Ok("avdec_h264");
    }

    Err("No H264 decoder plugin found in GStreamer. \
         Install the GStreamer libav plugin (avdec_h264) or run on Jetson (nvv4l2decoder)."
        .to_string())
}

// Per-decoder admission policy: at most 64 queued NALs and 8 MiB including Annex B.
// Overflow terminates the stream; dropping encoded NALs would corrupt decoder references.
const MAX_ENCODED_BUFFERS: usize = 64;
const MAX_ENCODED_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum H264DecodeError {
    EncodedQueueOverflow,
    PipelineFailed,
    EndOfStream,
    BufferAllocationFailed,
    BufferMappingFailed,
    PushFailed,
    FrameChannelClosed,
}

impl std::fmt::Display for H264DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Fixed classifications only: bus errors can contain peer-controlled data.
        write!(f, "{self:?}")
    }
}

// Older supported GStreamer APIs expose queue bytes but not buffer count. Count
// buffers until appsrc's source pad releases them; the sole producer is decode(&mut self).
fn track_encoded_buffers(appsrc: &gst_app::AppSrc) -> Result<Arc<AtomicUsize>, String> {
    let queued = Arc::new(AtomicUsize::new(0));
    let released = queued.clone();
    appsrc
        .static_pad("src")
        .ok_or("appsrc source pad missing")?
        .add_probe(gst::PadProbeType::BUFFER, move |_, _| {
            released.fetch_sub(1, Ordering::Relaxed);
            gst::PadProbeReturn::Ok
        })
        .ok_or("appsrc buffer probe installation failed")?;
    Ok(queued)
}

pub struct H264Decoder {
    pipeline: gst::Pipeline,
    appsrc: gst_app::AppSrc,
    frame_rx: Receiver<DecodedFrame>,
    queued_buffers: Arc<AtomicUsize>,
    terminal_error: Option<H264DecodeError>,
    #[allow(dead_code)]
    decoder_name: String,
}

impl H264Decoder {
    pub fn new() -> Result<Self, String> {
        ensure_gst_init()?;

        let decoder_name = detect_h264_decoder()?;
        debug!(
            event = "video_decoder_init",
            backend = "gstreamer",
            decoder = decoder_name,
            "GStreamer H264 decoder selected"
        );

        let pipeline_str = if decoder_name == "nvv4l2decoder" {
            // Jetson GPU decoder requires nvvidconv to convert from NVMM to CPU memory
            format!(
                "appsrc name=src is-live=true do-timestamp=true format=time block=false \
                 caps=video/x-h264,stream-format=byte-stream,alignment=nal \
                 ! h264parse \
                 ! {decoder_name} \
                 ! nvvidconv \
                 ! video/x-raw,format=BGRx \
                 ! videoconvert \
                 ! video/x-raw,format=BGR \
                 ! appsink name=sink emit-signals=false sync=false \
                   max-buffers=2 drop=true"
            )
        } else {
            // Software decoder (avdec_h264) can use videoconvert directly
            format!(
                "appsrc name=src is-live=true do-timestamp=true format=time block=false \
                 caps=video/x-h264,stream-format=byte-stream,alignment=nal \
                 ! h264parse \
                 ! {decoder_name} \
                 ! videoconvert \
                 ! video/x-raw,format=BGR \
                 ! appsink name=sink emit-signals=false sync=false \
                   max-buffers=2 drop=true"
            )
        };

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

                    let frame = packed_bgr_frame(buffer, &video_info)?;

                    // Non-blocking send; drop frame if consumer is slow
                    let _ = frame_tx.try_send(frame);

                    Ok(gst::FlowSuccess::Ok)
                })
                .build(),
        );

        let queued_buffers = track_encoded_buffers(&appsrc)?;
        appsrc.set_max_bytes(MAX_ENCODED_BYTES as u64);

        pipeline
            .set_state(gst::State::Playing)
            .map_err(|e| format!("Failed to set pipeline to Playing state: {e:?}"))?;

        Ok(Self {
            pipeline,
            appsrc,
            frame_rx,
            queued_buffers,
            terminal_error: None,
            decoder_name: decoder_name.to_string(),
        })
    }

    /// Poll terminal bus messages even when the RTP track is idle.
    pub fn check_health(&mut self) -> Result<(), H264DecodeError> {
        if let Some(error) = self.terminal_error {
            return Err(error);
        }
        if let Some(message) = self
            .pipeline
            .bus()
            .and_then(|bus| bus.pop_filtered(&[gst::MessageType::Error, gst::MessageType::Eos]))
        {
            let error = match message.view() {
                gst::MessageView::Eos(_) => H264DecodeError::EndOfStream,
                _ => H264DecodeError::PipelineFailed,
            };
            self.terminal_error = Some(error);
            return Err(error);
        }
        Ok(())
    }

    pub fn decode(&mut self, nal_data: &[u8]) -> Result<Option<DecodedFrame>, H264DecodeError> {
        let result = self.decode_inner(nal_data);
        if let Err(error) = result {
            self.terminal_error = Some(error);
        }
        result
    }

    fn decode_inner(&mut self, nal_data: &[u8]) -> Result<Option<DecodedFrame>, H264DecodeError> {
        self.check_health()?;
        if nal_data.is_empty() {
            return Ok(None);
        }

        let encoded_bytes = nal_data
            .len()
            .checked_add(4)
            .filter(|size| *size <= MAX_ENCODED_BYTES)
            .ok_or(H264DecodeError::EncodedQueueOverflow)?;
        // No other producer can enqueue between this check and push. GStreamer
        // can only reduce these levels, so the pre-push limits are conservative.
        if self.queued_buffers.load(Ordering::Relaxed) >= MAX_ENCODED_BUFFERS
            || self.appsrc.current_level_bytes() > (MAX_ENCODED_BYTES - encoded_bytes) as u64
        {
            return Err(H264DecodeError::EncodedQueueOverflow);
        }
        let mut gst_buffer = gst::Buffer::with_size(encoded_bytes)
            .map_err(|_| H264DecodeError::BufferAllocationFailed)?;
        {
            let buffer_ref = gst_buffer
                .get_mut()
                .ok_or(H264DecodeError::BufferMappingFailed)?;
            let mut map = buffer_ref
                .map_writable()
                .map_err(|_| H264DecodeError::BufferMappingFailed)?;
            map[..4].copy_from_slice(&[0, 0, 0, 1]);
            map[4..].copy_from_slice(nal_data);
        }

        self.queued_buffers.fetch_add(1, Ordering::Relaxed);
        self.appsrc
            .push_buffer(gst_buffer)
            .map_err(|_| H264DecodeError::PushFailed)?;
        self.check_health()?;
        match self.frame_rx.try_recv() {
            Ok(frame) => Ok(Some(frame)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(H264DecodeError::FrameChannelClosed),
        }
    }

    #[allow(dead_code)]
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

// Allocation policy: 64 MiB per BGR frame/plane permits 4K with headroom, not a model limit.
const MAX_VIDEO_BUFFER_BYTES: usize = 64 * 1024 * 1024;

fn bgr_output_size(width: u32, height: u32) -> Result<(usize, usize), gst::FlowError> {
    let row_bytes = (width as usize)
        .checked_mul(3)
        .ok_or(gst::FlowError::Error)?;
    let output_bytes = row_bytes
        .checked_mul(height as usize)
        .ok_or(gst::FlowError::Error)?;
    if row_bytes == 0 || height == 0 || output_bytes > MAX_VIDEO_BUFFER_BYTES {
        return Err(gst::FlowError::Error);
    }
    Ok((row_bytes, output_bytes))
}

fn packed_bgr_frame(
    buffer: &gst::BufferRef,
    info: &gst_video::VideoInfo,
) -> Result<DecodedFrame, gst::FlowError> {
    if info.format() != gst_video::VideoFormat::Bgr {
        return Err(gst::FlowError::Error);
    }
    bgr_output_size(info.width(), info.height())?;
    // VideoFrame mapping honors VideoMeta offsets/strides as well as negotiated caps.
    let frame = gst_video::VideoFrameRef::from_buffer_ref_readable(buffer, info)
        .map_err(|_| gst::FlowError::Error)?;
    let (row_bytes, output_bytes) = bgr_output_size(frame.width(), frame.height())?;
    let stride = usize::try_from(frame.plane_stride()[0]).map_err(|_| gst::FlowError::Error)?;
    let plane_bytes = stride
        .checked_mul(frame.height() as usize)
        .ok_or(gst::FlowError::Error)?;
    if stride < row_bytes
        || plane_bytes > MAX_VIDEO_BUFFER_BYTES
        || frame.plane_offset()[0]
            .checked_add(plane_bytes)
            .is_none_or(|end| end > buffer.size())
    {
        return Err(gst::FlowError::Error);
    }
    let plane = frame.plane_data(0).map_err(|_| gst::FlowError::Error)?;
    if plane.len() < plane_bytes {
        return Err(gst::FlowError::Error);
    }
    let mut data = Vec::with_capacity(output_bytes);
    for row in plane.chunks_exact(stride).take(frame.height() as usize) {
        data.extend_from_slice(&row[..row_bytes]);
    }
    Ok(DecodedFrame {
        data,
        width: frame.width(),
        height: frame.height(),
    })
}

pub struct DecodedFrame {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    // Keep the pipeline stopped so appsrc cannot drain; no decoder plugin is needed.
    fn stalled_decoder() -> (H264Decoder, crossbeam_channel::Sender<DecodedFrame>) {
        ensure_gst_init().unwrap();
        let pipeline = gst::parse::launch("appsrc name=src block=false ! fakesink")
            .unwrap()
            .downcast::<gst::Pipeline>()
            .unwrap();
        let appsrc = pipeline.by_name("src").unwrap().downcast().unwrap();
        let queued_buffers = track_encoded_buffers(&appsrc).unwrap();
        let (tx, frame_rx) = bounded(4);
        (
            H264Decoder {
                pipeline,
                appsrc,
                frame_rx,
                queued_buffers,
                terminal_error: None,
                decoder_name: "offline-fixture".into(),
            },
            tx,
        )
    }

    #[test]
    fn encoded_admission_bounds_stalled_queue_count() {
        let (mut decoder, _tx) = stalled_decoder();
        for _ in 0..64 {
            assert!(decoder.decode(&[0x65]).unwrap().is_none());
        }
        assert_eq!(
            decoder.decode(&[0x65]).err(),
            Some(H264DecodeError::EncodedQueueOverflow)
        );
        assert_eq!(decoder.appsrc.current_level_bytes(), 64 * 5);
        // Overflow is terminal, not a blind drop followed by continued decoding.
        assert!(decoder.decode(&[]).is_err());
    }

    #[test]
    fn encoded_admission_releases_count_when_appsrc_drains() {
        let (mut decoder, _tx) = stalled_decoder();
        decoder.pipeline.set_state(gst::State::Playing).unwrap();
        for _ in 0..3 {
            for _ in 0..MAX_ENCODED_BUFFERS {
                assert!(decoder.decode(&[0x65]).unwrap().is_none());
            }
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            while decoder.queued_buffers.load(Ordering::Relaxed) != 0 {
                assert!(std::time::Instant::now() < deadline, "appsrc did not drain");
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            assert_eq!(decoder.appsrc.current_level_bytes(), 0);
        }
    }

    #[test]
    fn encoded_admission_bounds_bytes_before_allocation() {
        let (mut decoder, _tx) = stalled_decoder();
        let nal = vec![0x65; 8 * 1024 * 1024 - 4];
        assert!(decoder.decode(&nal).unwrap().is_none());
        assert!(decoder.decode(&[0x65]).is_err());
        assert_eq!(decoder.appsrc.current_level_bytes(), 8 * 1024 * 1024);
        let (mut decoder, _tx) = stalled_decoder();
        assert!(decoder.decode(&vec![0; 8 * 1024 * 1024]).is_err());
        assert_eq!(decoder.appsrc.current_level_bytes(), 0);
    }

    #[test]
    fn terminal_pipeline_error_is_not_no_output() {
        let (mut decoder, _tx) = stalled_decoder();
        assert!(decoder.decode(&[]).unwrap().is_none());
        decoder
            .pipeline
            .bus()
            .unwrap()
            .post(
                gst::message::Error::builder(gst::StreamError::Failed, "secret payload")
                    .debug("secret pipeline details")
                    .build(),
            )
            .unwrap();
        let error = decoder
            .decode(&[])
            .err()
            .expect("bus error must be terminal");
        assert_eq!(error, H264DecodeError::PipelineFailed);
        assert!(!error.to_string().contains("secret"));
        assert!(decoder.decode(&[0x65]).is_err());
    }

    #[test]
    fn terminal_eos_and_closed_frame_channel_are_not_no_output() {
        let (mut decoder, tx) = stalled_decoder();
        drop(tx);
        assert!(decoder.decode(&[0x65]).is_err());
        let (mut decoder, _tx) = stalled_decoder();
        decoder
            .pipeline
            .bus()
            .unwrap()
            .post(gst::message::Eos::new())
            .unwrap();
        assert!(decoder.decode(&[]).is_err());
    }

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
    #[test]
    fn padded_bgr_plane_offset_is_repacked_to_tight_rows() {
        ensure_gst_init().unwrap();
        let info = gst_video::VideoInfo::builder(gst_video::VideoFormat::Bgr, 2, 2)
            .build()
            .unwrap();
        let mut buffer = gst::Buffer::from_mut_slice(vec![
            99, 99, 99, 99, 1, 2, 3, 4, 5, 6, 88, 88, 7, 8, 9, 10, 11, 12, 88, 88,
        ]);
        gst_video::VideoMeta::add_full(
            buffer.get_mut().unwrap(),
            gst_video::VideoFrameFlags::empty(),
            gst_video::VideoFormat::Bgr,
            2,
            2,
            &[4],
            &[8],
        )
        .unwrap();
        let frame = packed_bgr_frame(buffer.as_ref(), &info).unwrap();
        assert_eq!(frame.data, (1..=12).collect::<Vec<u8>>());
        assert_eq!((frame.width, frame.height), (2, 2));
    }

    #[test]
    fn tight_bgr_layout_is_preserved_and_large_shapes_are_rejected() {
        ensure_gst_init().unwrap();
        let info = gst_video::VideoInfo::builder(gst_video::VideoFormat::Bgr, 4, 2)
            .build()
            .unwrap();
        let bytes = (0..24).collect::<Vec<u8>>();
        let buffer = gst::Buffer::from_mut_slice(bytes.clone());
        assert_eq!(
            packed_bgr_frame(buffer.as_ref(), &info).unwrap().data,
            bytes
        );
        assert!(bgr_output_size(u32::MAX, u32::MAX).is_err());
        assert!(bgr_output_size(0, 2).is_err());
    }
}
