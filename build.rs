#[cfg(feature = "python")]
fn main() {
    pyo3_build_config::add_extension_module_link_args();
    // uv Python reports an empty framework prefix; never emit an empty rpath.
    if pyo3_build_config::get()
        .python_framework_prefix
        .as_deref()
        .is_some_and(|prefix| !prefix.is_empty())
    {
        pyo3_build_config::add_python_framework_link_args();
    }
    pyo3_build_config::add_libpython_rpath_link_args();
}

#[cfg(not(feature = "python"))]
fn main() {}
