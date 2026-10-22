#[cfg(feature = "python")]
use pyo3::exceptions::PyValueError;
#[cfg(feature = "python")]
use pyo3::prelude::*;
#[cfg(feature = "python")]
use pyo3::types::PyDict;
#[cfg(feature = "python")]
use tracing::debug;
#[cfg(feature = "python")]
use tracing_subscriber::EnvFilter;

pub mod audio;
pub mod connection;
pub mod datachannel;
mod diagnostics;
pub mod infrastructure;
#[cfg(feature = "python")]
pub mod interface;
pub mod protocol;
#[cfg(test)]
mod test_support;
pub mod video;

#[cfg(feature = "python")]
fn init_tracing() -> PyResult<()> {
    // A host's earlier global or scoped subscriber owns logging policy, not this SDK.
    if tracing::dispatcher::has_been_set() {
        return Ok(());
    }
    match std::env::var("UNITREE_WEBRTC_RS_LOG_INIT") {
        Ok(value) if value == "0" => return Ok(()),
        Ok(value) if value == "1" => {}
        Err(std::env::VarError::NotPresent) => {}
        _ => {
            return Err(PyValueError::new_err(
                "UNITREE_WEBRTC_RS_LOG_INIT must be 0 or 1",
            ));
        }
    }
    let filter = match std::env::var("UNITREE_WEBRTC_RS_LOG") {
        Ok(value) => EnvFilter::try_new(value)
            .map_err(|_| PyValueError::new_err("invalid UNITREE_WEBRTC_RS_LOG filter"))?,
        Err(std::env::VarError::NotPresent) => EnvFilter::new("warn"),
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err(PyValueError::new_err(
                "UNITREE_WEBRTC_RS_LOG must be Unicode",
            ));
        }
    };
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .with_target(true)
        .with_thread_names(false)
        .with_thread_ids(false)
        .compact()
        .finish();
    // The only possible failure is a competing host installation; preserve its policy.
    let _ = tracing::subscriber::set_global_default(subscriber);
    Ok(())
}

#[cfg(feature = "python")]
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

#[cfg(feature = "python")]
#[pymodule]
fn unitree_webrtc_rs(_py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    init_tracing()?;
    debug!(
        event = "module_initialized",
        "unitree_webrtc_rs module initialized"
    );

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

#[cfg(all(test, feature = "python"))]
mod logging_tests {
    use super::*;

    #[test]
    fn native_subscriber_policy_is_process_isolated() {
        const CASE: &str = "UNITREE_LOGGING_FIXTURE";
        let Ok(case) = std::env::var(CASE) else {
            for case in ["default", "info", "debug", "host-invalid", "host-after"] {
                let output = std::process::Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "logging_tests::native_subscriber_policy_is_process_isolated",
                        "--nocapture",
                    ])
                    .env(CASE, case)
                    .env_remove("UNITREE_WEBRTC_RS_LOG")
                    .env_remove("UNITREE_WEBRTC_RS_LOG_INIT")
                    .output()
                    .unwrap();
                let stderr = String::from_utf8(output.stderr).unwrap();
                assert!(output.status.success(), "{case}: {stderr}");
                if case.starts_with("host-") {
                    assert!(stderr.contains("HOST "), "{stderr}");
                }
                assert!(stderr.contains("WARN"), "{case}: {stderr}");
                assert_eq!(stderr.contains("INFO"), case != "default");
                assert_eq!(
                    stderr.contains("DEBUG"),
                    case == "debug" || case.starts_with("host-")
                );
                assert!(stderr.contains("unitree_webrtc_rs"), "{stderr}");
            }
            return;
        };
        let install_host = || {
            tracing::subscriber::set_global_default(
                tracing_subscriber::fmt()
                    .with_writer(std::io::stderr)
                    .with_ansi(false)
                    .with_max_level(tracing::Level::DEBUG)
                    .with_target(true)
                    .with_timer(HostPrefix)
                    .finish(),
            )
            .unwrap();
        };
        if case == "host-invalid" {
            install_host();
            std::env::set_var("UNITREE_WEBRTC_RS_LOG_INIT", "secret-invalid-init");
            std::env::set_var("UNITREE_WEBRTC_RS_LOG", "sdk=secret-invalid-level");
        } else if case == "host-after" {
            std::env::set_var("UNITREE_WEBRTC_RS_LOG_INIT", "0");
            std::env::set_var("UNITREE_WEBRTC_RS_LOG", "sdk=secret-invalid-level");
        } else if case != "default" {
            std::env::set_var("UNITREE_WEBRTC_RS_LOG", format!("unitree_webrtc_rs={case}"));
        }
        Python::initialize();
        Python::attach(|py| {
            let module = PyModule::new(py, "unitree_webrtc_rs").unwrap();
            unitree_webrtc_rs(py, &module).unwrap();
        });
        if case == "host-after" {
            assert!(!tracing::dispatcher::has_been_set());
            install_host();
        }
        tracing::warn!(event = "logging_fixture_warn");
        tracing::info!(event = "logging_fixture_info");
        tracing::debug!(event = "logging_fixture_debug");
    }

    struct HostPrefix;

    impl tracing_subscriber::fmt::time::FormatTime for HostPrefix {
        fn format_time(
            &self,
            writer: &mut tracing_subscriber::fmt::format::Writer<'_>,
        ) -> std::fmt::Result {
            write!(writer, "HOST")
        }
    }
}
