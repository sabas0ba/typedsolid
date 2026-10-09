use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::PyBytes;
use typedsolid_core::{MAX_LOCATIONS, Model, Policy, Report, mesh, voxel};

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

/// IRの1部品の断面。planesは[{"axis": "x", "coordinate": 1.0}, ...]。
#[pyfunction]
fn voxel_sections(json: &str, part: &str, planes: &str) -> PyResult<String> {
    let model = Model::from_json(json).map_err(PyValueError::new_err)?;
    let planes: Vec<voxel::Plane> =
        serde_json::from_str(planes).map_err(|e| PyValueError::new_err(e.to_string()))?;
    let sections = voxel::sections_of_part(&model, part, &planes).map_err(PyValueError::new_err)?;
    serde_json::to_string(&sections).map_err(|e| PyValueError::new_err(e.to_string()))
}

/// 外部のSTLの断面。
#[pyfunction]
fn stl_sections(stl: &[u8], policy: &str, planes: &str) -> PyResult<String> {
    let policy: Policy =
        serde_json::from_str(policy).map_err(|e| PyValueError::new_err(e.to_string()))?;
    let planes: Vec<voxel::Plane> =
        serde_json::from_str(planes).map_err(|e| PyValueError::new_err(e.to_string()))?;
    let sections = voxel::sections_of_stl(stl, &policy, &planes).map_err(PyValueError::new_err)?;
    serde_json::to_string(&sections).map_err(|e| PyValueError::new_err(e.to_string()))
}

/// STLの三角形の頂点座標。三角形ごとに9個のlittle endianのf32を並べる。
/// 3D viewerが、判定に使ったmeshをそのまま描くために使う。
#[pyfunction]
fn stl_triangles<'py>(py: Python<'py>, stl: &[u8]) -> PyResult<Bound<'py, PyBytes>> {
    let triangles = mesh::parse_stl(stl).map_err(PyValueError::new_err)?;
    let mut out = Vec::with_capacity(triangles.len() * 36);
    for triangle in &triangles {
        for vertex in triangle.vertices() {
            for coordinate in vertex {
                out.extend_from_slice(&(coordinate as f32).to_le_bytes());
            }
        }
    }
    Ok(PyBytes::new(py, &out))
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
    module.add_function(wrap_pyfunction!(voxel_sections, module)?)?;
    module.add_function(wrap_pyfunction!(stl_sections, module)?)?;
    module.add_function(wrap_pyfunction!(stl_triangles, module)?)?;
    module.add_function(wrap_pyfunction!(inspect_mesh, module)?)?;
    module.add_function(wrap_pyfunction!(export_allowed, module)?)?;
    // backendが求める検出箇所も同じ上限で切る。
    module.add("MAX_LOCATIONS", MAX_LOCATIONS)?;
    Ok(())
}
