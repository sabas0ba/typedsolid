use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::PyBytes;
use typedsolid_core::{
    MAX_LOCATIONS, ManufacturingPlan, Model, Policy, Report, analysis, mesh, voxel,
};

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

/// 解析ケースの線形弾性と定常熱伝導。IRから直接rasterizeするためbackendに依存しない。
/// `{"checks": [...], "fields": [...]}`を返す。fieldsは表面の分布で、判定には使わない。
#[pyfunction]
fn evaluate_analyses(json: &str) -> PyResult<String> {
    let model = Model::from_json(json).map_err(PyValueError::new_err)?;
    let (checks, fields) = analysis::evaluate_with_fields(&model).map_err(PyValueError::new_err)?;
    serde_json::to_string(&serde_json::json!({"checks": checks, "fields": fields}))
        .map_err(|e| PyValueError::new_err(e.to_string()))
}

/// 分布を点に標本化する。positionsは点ごとに3つのlittle endianのf32を並べたもの。
/// 点ごとの値 (little endianのf32、表面のcellが近くに無ければNaN) と、判定しない近傍か
/// (0か1のbyte) を返す。
/// 分布を色で描くための三角形。positionsは頂点ごとに3つのlittle endianのf32、indicesは
/// 三角形ごとの頂点番号 (little endianのu32) で、Noneならpositionsが3頂点ずつ並ぶ。fieldsは
/// 部品の分布のJSONの配列。三角形ごとに3頂点を並べたpositionsを返す。
#[pyfunction]
#[pyo3(signature = (fields, positions, indices, max_edge, tolerance))]
fn refine_for_fields<'py>(
    py: Python<'py>,
    fields: &str,
    positions: &[u8],
    indices: Option<&[u8]>,
    max_edge: f64,
    tolerance: f64,
) -> PyResult<Bound<'py, PyBytes>> {
    let fields: Vec<analysis::Field> =
        serde_json::from_str(fields).map_err(|e| PyValueError::new_err(e.to_string()))?;
    let points = points_of(positions)?;
    let corners: Vec<usize> = match indices {
        Some(bytes) => bytes
            .chunks_exact(4)
            .map(|chunk| u32::from_le_bytes(chunk.try_into().expect("4 bytes")) as usize)
            .collect(),
        None => (0..points.len()).collect(),
    };
    if !corners.len().is_multiple_of(3) || corners.iter().any(|k| *k >= points.len()) {
        return Err(PyValueError::new_err(
            "indices must name three existing vertices per triangle",
        ));
    }
    let triangles: Vec<[[f32; 3]; 3]> = corners
        .chunks_exact(3)
        .map(|t| [points[t[0]], points[t[1]], points[t[2]]])
        .collect();
    let refined = analysis::refine(&triangles, &fields, max_edge, tolerance);
    let bytes: Vec<u8> = refined
        .iter()
        .flatten()
        .flatten()
        .flat_map(|c| c.to_le_bytes())
        .collect();
    Ok(PyBytes::new(py, &bytes))
}

fn points_of(positions: &[u8]) -> PyResult<Vec<[f32; 3]>> {
    if !positions.len().is_multiple_of(12) {
        return Err(PyValueError::new_err("positions must hold 3 f32 per point"));
    }
    Ok(positions
        .chunks_exact(12)
        .map(|chunk| {
            std::array::from_fn(|axis| {
                f32::from_le_bytes(chunk[4 * axis..4 * axis + 4].try_into().expect("4 bytes"))
            })
        })
        .collect())
}

#[pyfunction]
fn sample_field<'py>(
    py: Python<'py>,
    field: &str,
    positions: &[u8],
) -> PyResult<(Bound<'py, PyBytes>, Bound<'py, PyBytes>)> {
    let field: analysis::Field =
        serde_json::from_str(field).map_err(|e| PyValueError::new_err(e.to_string()))?;
    let points = points_of(positions)?;
    let (values, excluded) = analysis::sample(&field, &points);
    let values: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
    let excluded: Vec<u8> = excluded.iter().map(|e| u8::from(*e)).collect();
    Ok((PyBytes::new(py, &values), PyBytes::new(py, &excluded)))
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

fn policy_of(json: &str) -> PyResult<Policy> {
    serde_json::from_str(json).map_err(|e| PyValueError::new_err(e.to_string()))
}

fn plan_of(json: &str) -> PyResult<ManufacturingPlan> {
    serde_json::from_str(json).map_err(|e| PyValueError::new_err(e.to_string()))
}

fn planes_of(json: &str) -> PyResult<Vec<voxel::Plane>> {
    serde_json::from_str(json).map_err(|e| PyValueError::new_err(e.to_string()))
}

/// 外部のSTL (binary又はASCII) を読み、最終形状のruleを評価する。IRを介さない。
/// planは製造案のJSONで、採用したものとして扱う。
#[pyfunction]
fn evaluate_stl_voxels(stl: &[u8], target: &str, policy: &str, plan: &str) -> PyResult<String> {
    let policy = policy_of(policy)?;
    let plan = plan_of(plan)?;
    let checks = voxel::evaluate_stl(stl, target, &policy, &plan).map_err(PyValueError::new_err)?;
    serde_json::to_string(&checks).map_err(|e| PyValueError::new_err(e.to_string()))
}

/// IRの1部品の断面。planesは[{"axis": "x", "coordinate": 1.0}, ...]。planは製造案のidで、
/// Noneなら採用した製造案で判定する。
#[pyfunction]
#[pyo3(signature = (json, part, plan, planes))]
fn voxel_sections(json: &str, part: &str, plan: Option<&str>, planes: &str) -> PyResult<String> {
    let model = Model::from_json(json).map_err(PyValueError::new_err)?;
    let planes = planes_of(planes)?;
    let sections =
        voxel::sections_of_part(&model, part, plan, &planes).map_err(PyValueError::new_err)?;
    serde_json::to_string(&sections).map_err(|e| PyValueError::new_err(e.to_string()))
}

/// 外部のSTLの断面。planは製造案のJSON。
#[pyfunction]
fn stl_sections(stl: &[u8], policy: &str, plan: &str, planes: &str) -> PyResult<String> {
    let policy = policy_of(policy)?;
    let plan = plan_of(plan)?;
    let planes = planes_of(planes)?;
    let sections =
        voxel::sections_of_stl(stl, &policy, &plan, &planes).map_err(PyValueError::new_err)?;
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
    module.add_function(wrap_pyfunction!(evaluate_analyses, module)?)?;
    module.add_function(wrap_pyfunction!(sample_field, module)?)?;
    module.add_function(wrap_pyfunction!(refine_for_fields, module)?)?;
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
