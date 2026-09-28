use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use typedsolid_core::{Model, Policy, Report, mesh, voxel};

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

/// 最終形状の肉厚・接続部・空洞の検査。IRから直接rasterizeするためbackendに依存しない。
#[pyfunction]
fn evaluate_voxels(json: &str) -> PyResult<String> {
    let model = Model::from_json(json).map_err(PyValueError::new_err)?;
    let checks = voxel::evaluate(&model).map_err(PyValueError::new_err)?;
    serde_json::to_string(&checks).map_err(|e| PyValueError::new_err(e.to_string()))
}

/// snap fitのうち寸法だけで決まる検査 (ひずみ、積層方向)。
#[pyfunction]
fn evaluate_snap_fits(json: &str) -> PyResult<String> {
    let model = Model::from_json(json).map_err(PyValueError::new_err)?;
    let checks = model.evaluate_snap_fits().map_err(PyValueError::new_err)?;
    serde_json::to_string(&checks).map_err(|e| PyValueError::new_err(e.to_string()))
}

/// ネジを外す順序の検査。形状を使わない。
#[pyfunction]
fn evaluate_fastener_releases(json: &str) -> PyResult<String> {
    let model = Model::from_json(json).map_err(PyValueError::new_err)?;
    let checks = model
        .evaluate_fastener_releases()
        .map_err(PyValueError::new_err)?;
    serde_json::to_string(&checks).map_err(|e| PyValueError::new_err(e.to_string()))
}

/// コネクタ開口の寸法の整合。形状を使わない。
#[pyfunction]
fn evaluate_connectors(json: &str) -> PyResult<String> {
    let model = Model::from_json(json).map_err(PyValueError::new_err)?;
    let checks = model.evaluate_connectors().map_err(PyValueError::new_err)?;
    serde_json::to_string(&checks).map_err(|e| PyValueError::new_err(e.to_string()))
}

/// 外部のSTL (binary又はASCII) を読み、最終形状のruleを評価する。IRを介さない。
#[pyfunction]
fn evaluate_stl_voxels(stl: &[u8], target: &str, policy: &str) -> PyResult<String> {
    let policy: Policy =
        serde_json::from_str(policy).map_err(|e| PyValueError::new_err(e.to_string()))?;
    let checks = voxel::evaluate_stl(stl, target, &policy).map_err(PyValueError::new_err)?;
    serde_json::to_string(&checks).map_err(|e| PyValueError::new_err(e.to_string()))
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
    module.add_function(wrap_pyfunction!(evaluate_voxels, module)?)?;
    module.add_function(wrap_pyfunction!(evaluate_snap_fits, module)?)?;
    module.add_function(wrap_pyfunction!(evaluate_fastener_releases, module)?)?;
    module.add_function(wrap_pyfunction!(evaluate_connectors, module)?)?;
    module.add_function(wrap_pyfunction!(evaluate_stl_voxels, module)?)?;
    module.add_function(wrap_pyfunction!(inspect_mesh, module)?)?;
    module.add_function(wrap_pyfunction!(export_allowed, module)?)?;
    Ok(())
}
