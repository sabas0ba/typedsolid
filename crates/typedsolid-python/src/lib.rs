use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use typedsolid_core::{Model, Report};

#[pyfunction]
fn normalize_model(json: &str) -> PyResult<String> {
    let model = Model::from_json(json).map_err(PyValueError::new_err)?;
    serde_json::to_string(&model).map_err(|e| PyValueError::new_err(e.to_string()))
}

#[pyfunction]
fn preflight(json: &str) -> PyResult<String> {
    let model = Model::from_json(json).map_err(PyValueError::new_err)?;
    let report = model.preflight().map_err(PyValueError::new_err)?;
    serde_json::to_string(&report).map_err(|e| PyValueError::new_err(e.to_string()))
}

#[pyfunction]
fn export_allowed(json: &str) -> PyResult<bool> {
    let report: Report =
        serde_json::from_str(json).map_err(|e| PyValueError::new_err(e.to_string()))?;
    Ok(report.export_allowed())
}

#[pymodule]
fn _native(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_function(wrap_pyfunction!(normalize_model, module)?)?;
    module.add_function(wrap_pyfunction!(preflight, module)?)?;
    module.add_function(wrap_pyfunction!(export_allowed, module)?)?;
    Ok(())
}
