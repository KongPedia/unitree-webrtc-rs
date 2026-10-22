use crate::connection::{SessionOptions, UnitreeSession};
use crate::interface::bridges::audio::PyAudioBridge;
use crate::interface::bridges::datachannel::PyDataChannelBridge;
use crate::interface::bridges::pubsub::CallbackRegistry;
use crate::interface::bridges::video::PyVideoBridge;
use crate::interface::callbacks::spawn_callback_dispatcher;
use crate::interface::utils::to_py_error;
use crate::protocol::constants::WebRTCConnectionMethod;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::PyDict;
use std::collections::HashMap;
use std::env;
use std::sync::{Arc, Mutex};

const AES_128_KEY_ENV_VAR: &str = "GO2_DORA_UNITREE_AES_128_KEY";

fn resolve_aes_128_key(explicit_key: Option<String>) -> Option<String> {
    normalize_aes_128_key(explicit_key).or_else(|| {
        env::var(AES_128_KEY_ENV_VAR)
            .ok()
            .and_then(|value| normalize_aes_128_key(Some(value)))
    })
}

fn normalize_aes_128_key(key: Option<String>) -> Option<String> {
    key.map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[pyclass(name = "UnitreeWebRTCConnection")]
pub struct PyUnitreeWebRTCConnection {
    connection_method: WebRTCConnectionMethod,
    serial_number: Option<String>,
    ip: Option<String>,
    _username: Option<String>,
    _password: Option<String>,
    _aes_128_key: Option<String>,
    session: Arc<UnitreeSession>,
    callback_registry: CallbackRegistry,
}

#[pymethods]
impl PyUnitreeWebRTCConnection {
    #[new]
    #[pyo3(signature = (connection_method, serial_number=None, ip=None, username=None, password=None, aes_128_key=None, **kwargs))]
    fn new(
        connection_method: i32,
        serial_number: Option<String>,
        ip: Option<String>,
        username: Option<String>,
        password: Option<String>,
        aes_128_key: Option<String>,
        kwargs: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<Self> {
        let mut resolved_method = connection_method;
        let mut resolved_serial = serial_number.clone();
        let mut resolved_aes_128_key = aes_128_key.clone();

        if let Some(kwargs) = kwargs {
            if let Ok(Some(value)) = kwargs.get_item("connectionMethod") {
                resolved_method = value.extract::<i32>()?;
            }

            if let Ok(Some(value)) = kwargs.get_item("serialNumber") {
                resolved_serial = Some(value.extract::<String>()?);
            }

            for key in ["aes_128_key", "aes128Key", "aes128_key"] {
                if let Ok(Some(value)) = kwargs.get_item(key) {
                    if !value.is_none() {
                        resolved_aes_128_key = Some(value.extract::<String>()?);
                    }
                }
            }
        }

        let resolved_aes_128_key = resolve_aes_128_key(resolved_aes_128_key);

        let connection_method =
            WebRTCConnectionMethod::try_from(resolved_method).map_err(PyValueError::new_err)?;

        let (session, callback_events_rx) = UnitreeSession::new(
            SessionOptions::new(connection_method, ip.clone()),
            resolved_aes_128_key.clone(),
        )
        .map_err(to_py_error)?;
        let callback_registry: CallbackRegistry = Arc::new(Mutex::new(HashMap::new()));
        spawn_callback_dispatcher(callback_events_rx, Arc::clone(&callback_registry));

        Ok(Self {
            connection_method,
            serial_number: resolved_serial,
            ip,
            _username: username,
            _password: password,
            _aes_128_key: resolved_aes_128_key,
            session: Arc::new(session),
            callback_registry,
        })
    }

    fn connect<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let session = Arc::clone(&self.session);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            session.connect().await.map_err(to_py_error)
        })
    }

    fn disconnect<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let session = Arc::clone(&self.session);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            session.disconnect().await;
            Ok(())
        })
    }

    fn reconnect<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let session = Arc::clone(&self.session);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            session.reconnect().await.map_err(to_py_error)
        })
    }

    #[pyo3(name = "_auto_reconnect")]
    #[pyo3(signature = (max_retries=5))]
    fn auto_reconnect_legacy<'py>(
        &self,
        py: Python<'py>,
        max_retries: u32,
    ) -> PyResult<Bound<'py, PyAny>> {
        let session = Arc::clone(&self.session);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            Ok(session.auto_reconnect(max_retries).await)
        })
    }

    #[getter]
    fn connection_method(&self) -> i32 {
        self.connection_method as i32
    }

    #[getter(connectionMethod)]
    fn connection_method_legacy(&self) -> i32 {
        self.connection_method as i32
    }

    #[getter]
    fn is_connected(&self) -> bool {
        self.session.is_connected()
    }

    #[getter(isConnected)]
    fn is_connected_legacy(&self) -> bool {
        self.session.is_connected()
    }

    #[getter]
    fn ip(&self) -> Option<String> {
        self.ip.clone()
    }

    #[getter]
    fn video(&self, py: Python<'_>) -> PyResult<Py<PyVideoBridge>> {
        Py::new(
            py,
            PyVideoBridge {
                _session: Arc::clone(&self.session),
                datachannel_service: self.session.datachannel(),
                callback_registry: Arc::clone(&self.callback_registry),
            },
        )
    }

    #[getter]
    fn audio(&self, py: Python<'_>) -> PyResult<Py<PyAudioBridge>> {
        Py::new(
            py,
            PyAudioBridge {
                datachannel_service: self.session.datachannel(),
                callback_registry: Arc::clone(&self.callback_registry),
                session: Arc::clone(&self.session),
            },
        )
    }

    #[getter]
    fn serial_number(&self) -> Option<String> {
        self.serial_number.clone()
    }

    #[getter]
    fn datachannel(&self, py: Python<'_>) -> PyResult<Py<PyDataChannelBridge>> {
        Py::new(
            py,
            PyDataChannelBridge {
                _session: Arc::clone(&self.session),
                service: self.session.datachannel(),
                callback_registry: Arc::clone(&self.callback_registry),
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn resolve_aes_128_key_prefers_explicit_key() {
        let _guard = env_lock().lock().unwrap();
        env::set_var(
            "GO2_DORA_UNITREE_AES_128_KEY",
            "ffeeddccbbaa99887766554433221100",
        );

        assert_eq!(
            resolve_aes_128_key(Some(" 00112233445566778899aabbccddeeff ".to_string())),
            Some("00112233445566778899aabbccddeeff".to_string())
        );

        env::remove_var("GO2_DORA_UNITREE_AES_128_KEY");
    }

    #[test]
    fn resolve_aes_128_key_uses_only_canonical_env_key() {
        let _guard = env_lock().lock().unwrap();
        env::remove_var("GO2_DORA_UNITREE_AES_128_KEY");
        env::set_var("UNITREE_AES_128_KEY", "ffeeddccbbaa99887766554433221100");

        assert_eq!(resolve_aes_128_key(None), None);

        env::set_var(
            "GO2_DORA_UNITREE_AES_128_KEY",
            "00112233445566778899aabbccddeeff",
        );
        assert_eq!(
            resolve_aes_128_key(None),
            Some("00112233445566778899aabbccddeeff".to_string())
        );

        env::remove_var("GO2_DORA_UNITREE_AES_128_KEY");
        env::remove_var("UNITREE_AES_128_KEY");
    }
    #[test]
    fn actual_python_getters_keep_session_alive_until_last_child_drops() {
        Python::initialize();
        Python::attach(|py| {
            for kind in ["audio", "video", "datachannel", "pub_sub"] {
                let root = py
                    .get_type::<PyUnitreeWebRTCConnection>()
                    .call1((1,))
                    .unwrap();
                let weak = {
                    let connection = root
                        .extract::<PyRef<'_, PyUnitreeWebRTCConnection>>()
                        .unwrap();
                    Arc::downgrade(&connection.session)
                };
                let child = if kind == "pub_sub" {
                    root.getattr("datachannel")
                        .unwrap()
                        .getattr("pub_sub")
                        .unwrap()
                } else {
                    root.getattr(kind).unwrap()
                };
                drop(root);
                assert!(
                    weak.upgrade().is_some(),
                    "{kind} must retain the complete session owner"
                );
                drop(child);
                assert!(
                    weak.upgrade().is_none(),
                    "{kind} must release the final session owner"
                );
            }
        });
    }
}
