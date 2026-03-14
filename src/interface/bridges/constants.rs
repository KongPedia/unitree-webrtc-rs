use pyo3::prelude::*;

#[pyclass(name = "WebRTCConnectionMethod")]
pub struct PyWebRTCConnectionMethod;

#[allow(non_upper_case_globals)]
#[pymethods]
impl PyWebRTCConnectionMethod {
    #[classattr]
    const LocalAP: i32 = 1;
    #[classattr]
    const LocalSTA: i32 = 2;
    #[classattr]
    const Remote: i32 = 3;
}

#[pyclass(name = "VUI_COLOR")]
pub struct PyVuiColor;

#[allow(non_upper_case_globals)]
#[pymethods]
impl PyVuiColor {
    #[classattr]
    const WHITE: &'static str = "white";
    #[classattr]
    const RED: &'static str = "red";
    #[classattr]
    const YELLOW: &'static str = "yellow";
    #[classattr]
    const BLUE: &'static str = "blue";
    #[classattr]
    const GREEN: &'static str = "green";
    #[classattr]
    const CYAN: &'static str = "cyan";
    #[classattr]
    const PURPLE: &'static str = "purple";
}
