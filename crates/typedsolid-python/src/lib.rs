use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use typedsolid_core::{Model, Report, mesh};

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

/// 出力STLの構造検査。解析失敗は例外にせず、failのcheckとして返す。
#[pyfunction]
fn inspect_mesh(
    stl: &[u8],
    target: &str,
    solid_volume_mm3: f64,
    relative_tolerance: f64,
) -> PyResult<String> {
    let checks = mesh::inspect_binary_stl(stl, target, solid_volume_mm3, relative_tolerance);
    serde_json::to_string(&checks).map_err(|e| PyValueError::new_err(e.to_string()))
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
    module.add_function(wrap_pyfunction!(inspect_mesh, module)?)?;
    module.add_function(wrap_pyfunction!(export_allowed, module)?)?;
    Ok(())
}
