use pyo3::prelude::*;
use pyo3::types::PyDict;
use std::sync::Once;
use tracing::info;
use tracing_subscriber::EnvFilter;

pub mod audio;
pub mod connection;
pub mod datachannel;
pub mod infrastructure;
pub mod interface;
pub mod protocol;
pub mod video;

static TRACING_INIT: Once = Once::new();

fn init_tracing() {
    TRACING_INIT.call_once(|| {
        let filter = std::env::var("UNITREE_WEBRTC_RS_LOG").unwrap_or_else(|_| {
            "unitree_webrtc_rs=info,webrtc=error,webrtc_ice=error,webrtc_mdns=error,dtls=error,warn"
                .to_string()
        });

        let _ = tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::new(filter))
            .with_target(false)
            .with_thread_names(false)
            .with_thread_ids(false)
            .compact()
            .try_init();
    });
}

fn register_constants(py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    // DATA_CHANNEL_TYPE: Dict[str, str]
    let dc_type = PyDict::new(py);
    for (k, v) in protocol::constants::data_channel_type() {
        dc_type.set_item(*k, *v)?;
    }
    m.add("DATA_CHANNEL_TYPE", dc_type)?;

    // RTC_TOPIC: Dict[str, str]
    let rtc_topic = PyDict::new(py);
    for (k, v) in protocol::constants::rtc_topic() {
        rtc_topic.set_item(*k, *v)?;
    }
    m.add("RTC_TOPIC", rtc_topic)?;

    // SPORT_CMD: Dict[str, int]
    let sport_cmd = PyDict::new(py);
    for (k, v) in protocol::constants::sport_cmd() {
        sport_cmd.set_item(*k, *v)?;
    }
    m.add("SPORT_CMD", sport_cmd)?;

    // AUDIO_API: Dict[str, int]
    let audio_api = PyDict::new(py);
    for (k, v) in protocol::constants::audio_api() {
        audio_api.set_item(*k, *v)?;
    }
    m.add("AUDIO_API", audio_api)?;

    // APP_ERROR_MESSAGES: Dict[str, str]
    let app_errors = PyDict::new(py);
    for (k, v) in protocol::constants::app_error_messages() {
        app_errors.set_item(*k, *v)?;
    }
    m.add("APP_ERROR_MESSAGES", app_errors)?;

    Ok(())
}

#[pymodule]
fn unitree_webrtc_rs(_py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    init_tracing();
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    info!("unitree_webrtc_rs module initialized");

    m.add_class::<interface::bridges::PyUnitreeWebRTCConnection>()?;
    m.add_class::<interface::bridges::PyWebRTCConnectionMethod>()?;
    m.add_class::<interface::bridges::PyVuiColor>()?;
    register_constants(_py, m)?;

    let driver_module = PyModule::new(_py, "webrtc_driver")?;
    driver_module.add_class::<interface::bridges::PyUnitreeWebRTCConnection>()?;
    driver_module.add_class::<interface::bridges::PyWebRTCConnectionMethod>()?;
    driver_module.add_class::<interface::bridges::PyVuiColor>()?;
    register_constants(_py, &driver_module)?;
    m.add_submodule(&driver_module)?;

    let sys = _py.import("sys")?;
    let modules = sys.getattr("modules")?;
    modules.set_item("unitree_webrtc_rs.webrtc_driver", &driver_module)?;

    Ok(())
}
