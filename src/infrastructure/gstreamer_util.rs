use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Once;
use tracing::warn;

static GST_INIT: Once = Once::new();
static INIT_RESULT: AtomicBool = AtomicBool::new(false);

pub fn ensure_gst_init() -> Result<(), String> {
    GST_INIT.call_once(|| match gstreamer::init() {
        Ok(_) => INIT_RESULT.store(true, Ordering::SeqCst),
        Err(e) => {
            warn!(
                event = "gstreamer_init_failed",
                error = ?e,
                "Failed to initialize GStreamer"
            );
        }
    });

    if INIT_RESULT.load(Ordering::SeqCst) {
        Ok(())
    } else {
        Err("GStreamer initialization failed. Is GStreamer installed?".to_string())
    }
}
