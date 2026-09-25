//! CADカーネルから独立した意味モデルとpreflight検証。

pub mod mesh;
pub mod voxel;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// 本buildが生成するschema。読み込みは旧版からの昇格も受理する。
pub const SCHEMA_VERSION: u32 = 4;

/// 座標の絶対値上限。単位はmm。
const COORDINATE_LIMIT_MM: f64 = 1e6;
/// primitiveの最小寸法。単位はmm。
const MINIMUM_EXTENT_MM: f64 = 0.001;
/// 分解stepの上限。部品数の上限と揃える。
const MAX_STEPS: usize = 100;
/// 1 stepの経路区間の上限。
const MAX_SEGMENTS: usize = 16;
/// はめ合い隙間の上限。単位はmm。
const MAX_FIT_CLEARANCE_MM: f64 = 100.0;
/// 掃引の上限。v3のaccessからの昇格で1 keepoutあたり最大6件生じる。
const MAX_SWEEPS: usize = 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    X,
    Y,
    Z,
}

impl Axis {
    pub fn index(self) -> usize {
        match self {
            Self::X => 0,
            Self::Y => 1,
            Self::Z => 2,
        }
    }

    /// 軸に垂直な2軸のindex。cylinderのcenterはこの順に並ぶ。
    pub fn plane(self) -> [usize; 2] {
        match self {
            Self::X => [1, 2],
            Self::Y => [0, 2],
            Self::Z => [0, 1],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    MinusX,
    PlusX,
    MinusY,
    PlusY,
    MinusZ,
    PlusZ,
}

impl Direction {
    pub const ALL: [Self; 6] = [
        Self::MinusX,
        Self::PlusX,
        Self::MinusY,
        Self::PlusY,
        Self::MinusZ,
        Self::PlusZ,
    ];

    pub fn axis(self) -> Axis {
        match self {
            Self::MinusX | Self::PlusX => Axis::X,
            Self::MinusY | Self::PlusY => Axis::Y,
            Self::MinusZ | Self::PlusZ => Axis::Z,
        }
    }

    pub fn is_positive(self) -> bool {
        matches!(self, Self::PlusX | Self::PlusY | Self::PlusZ)
    }
}

impl std::fmt::Display for Direction {
    /// checkのmessageはJSONと同じ綴りで方向を示す。
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::MinusX => "minus_x",
            Self::PlusX => "plus_x",
            Self::MinusY => "minus_y",
            Self::PlusY => "plus_y",
            Self::MinusZ => "minus_z",
            Self::PlusZ => "plus_z",
        })
    }
}

/// 軸平行のprimitive。回転は扱わない。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Shape {
    Box {
        min: [f64; 3],
        max: [f64; 3],
    },
    /// 軸平行の円柱。centerは軸に垂直な平面上の2座標、spanは軸方向の範囲。
    Cylinder {
        axis: Axis,
        center: [f64; 2],
        radius: f64,
        span: [f64; 2],
    },
}

fn finite_coordinate(value: f64) -> Result<(), String> {
    if !value.is_finite() || value.abs() > COORDINATE_LIMIT_MM {
        return Err(format!(
            "coordinates must be finite and within ±{COORDINATE_LIMIT_MM:.0} mm"
        ));
    }
    Ok(())
}

impl Shape {
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Box { min, max } => {
                for axis in 0..3 {
                    finite_coordinate(min[axis])?;
                    finite_coordinate(max[axis])?;
                    if max[axis] - min[axis] < MINIMUM_EXTENT_MM {
                        return Err(format!(
                            "box dimensions must be at least {MINIMUM_EXTENT_MM} mm"
                        ));
                    }
                }
            }
            Self::Cylinder {
                center,
                radius,
                span,
                ..
            } => {
                for value in center.iter().chain(span.iter()) {
                    finite_coordinate(*value)?;
                }
                if !radius.is_finite() || *radius <= 0.0 {
                    return Err("cylinder radius must be finite and positive".into());
                }
                if radius * 2.0 < MINIMUM_EXTENT_MM {
                    return Err(format!(
                        "cylinder diameter must be at least {MINIMUM_EXTENT_MM} mm"
                    ));
                }
                if span[1] - span[0] < MINIMUM_EXTENT_MM {
                    return Err(format!(
                        "cylinder span must be at least {MINIMUM_EXTENT_MM} mm"
                    ));
                }
                // 半径を含めた到達範囲も座標上限に収める。
                let (low, high) = self.aabb();
                for axis in 0..3 {
                    finite_coordinate(low[axis])?;
                    finite_coordinate(high[axis])?;
                }
            }
        }
        Ok(())
    }

    /// 軸平行境界box。keepoutの膨張や掃引はこれを基準にする。
    pub fn aabb(&self) -> ([f64; 3], [f64; 3]) {
        match self {
            Self::Box { min, max } => (*min, *max),
            Self::Cylinder {
                axis,
                center,
                radius,
                span,
            } => {
                let mut low = [0.0; 3];
                let mut high = [0.0; 3];
                low[axis.index()] = span[0];
                high[axis.index()] = span[1];
                for (slot, index) in axis.plane().into_iter().enumerate() {
                    low[index] = center[slot] - radius;
                    high[index] = center[slot] + radius;
                }
                (low, high)
            }
        }
    }

    /// 最小feature寸法ルールが測る値。cylinderは直径と高さの小さい方とする。
    pub fn minimum_dimension(&self) -> f64 {
        match self {
            Self::Box { min, max } => (0..3)
                .map(|i| max[i] - min[i])
                .fold(f64::INFINITY, f64::min),
            Self::Cylinder { radius, span, .. } => (radius * 2.0).min(span[1] - span[0]),
        }
    }

    pub fn is_box(&self) -> bool {
        matches!(self, Self::Box { .. })
    }

    /// 点が形状の内部または境界にあるか。rasterizeはvoxel中心をこの判定にかける。
    pub fn contains(&self, point: [f64; 3]) -> bool {
        match self {
            Self::Box { min, max } => (0..3).all(|i| point[i] >= min[i] && point[i] <= max[i]),
            Self::Cylinder {
                axis,
                center,
                radius,
                span,
            } => {
                let along = point[axis.index()];
                if along < span[0] || along > span[1] {
                    return false;
                }
                let [first, second] = axis.plane();
                let du = point[first] - center[0];
                let dv = point[second] - center[1];
                du * du + dv * dv <= radius * radius
            }
        }
    }
}

/// 面ごとのclearance。指定のない面にはdefaultを適用する。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Clearance {
    pub default: f64,
    #[serde(flatten)]
    pub faces: BTreeMap<Direction, f64>,
}

impl Clearance {
    pub fn for_face(&self, face: Direction) -> f64 {
        *self.faces.get(&face).unwrap_or(&self.default)
    }

    pub fn validate(&self) -> Result<(), String> {
        for value in std::iter::once(&self.default).chain(self.faces.values()) {
            if !value.is_finite() || !(0.0..=1000.0).contains(value) {
                return Err("clearance_mm must be finite and in [0, 1000]".into());
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Add,
    Cut,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Base,
    Wall,
    Mount,
    Rib,
    Generic,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Feature {
    pub id: String,
    pub role: Role,
    pub operation: Operation,
    pub shape: Shape,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Part {
    pub id: String,
    pub features: Vec<Feature>,
}

impl Part {
    /// 付加形状を含むAABB。cutは範囲を広げないため対象にしない。
    pub fn bounds(&self) -> Option<([f64; 3], [f64; 3])> {
        let mut result: Option<([f64; 3], [f64; 3])> = None;
        for feature in self
            .features
            .iter()
            .filter(|f| f.operation == Operation::Add)
        {
            let (low, high) = feature.shape.aabb();
            result = Some(match result {
                None => (low, high),
                Some((current_low, current_high)) => {
                    let mut merged = (current_low, current_high);
                    for axis in 0..3 {
                        merged.0[axis] = merged.0[axis].min(low[axis]);
                        merged.1[axis] = merged.1[axis].max(high[axis]);
                    }
                    merged
                }
            });
        }
        result
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Keepout {
    pub id: String,
    pub shape: Shape,
    pub clearance_mm: Clearance,
}

/// 経路区間の移動量を数値以外で指定する語。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DistanceKeyword {
    /// 残っている部品のAABBの外まで動かす。
    Exit,
}

/// 経路区間の移動量。数値はmm。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Distance {
    Millimetres(f64),
    Keyword(DistanceKeyword),
}

/// 軸平行の直線区間。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Move {
    pub direction: Direction,
    pub distance_mm: Distance,
}

/// 分解の1手順。partsを一体として経路に沿って動かし、以降の状態から除く。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    pub id: String,
    pub parts: Vec<String>,
    pub path: Vec<Move>,
    /// assemblyの値をこのstepだけ上書きする。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fit_clearance_mm: Option<f64>,
}

/// IRに記述した部品位置を組立完了の状態とし、stepsを順に実行して分解する。
/// 組立順序は分解の逆とする。どのstepにも現れない部品は最後まで残る。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Assembly {
    /// 移動方向に垂直な向きに要求する隙間。単位はmm。0は硬い干渉だけを見る。
    #[serde(default)]
    pub fit_clearance_mm: f64,
    #[serde(default)]
    pub steps: Vec<Step>,
}

fn fit_clearance(value: f64, owner: &str) -> Result<(), String> {
    if !value.is_finite() || !(0.0..=MAX_FIT_CLEARANCE_MM).contains(&value) {
        return Err(format!(
            "{owner}: fit_clearance_mm must be finite and in [0, {MAX_FIT_CLEARANCE_MM}]"
        ));
    }
    Ok(())
}

impl Assembly {
    fn validate(&self, part_ids: &BTreeSet<&String>) -> Result<(), String> {
        fit_clearance(self.fit_clearance_mm, "assembly")?;
        if self.steps.len() > MAX_STEPS {
            return Err(format!(
                "at most {MAX_STEPS} disassembly steps are supported"
            ));
        }
        let mut step_ids = BTreeSet::new();
        let mut removed = BTreeSet::new();
        for step in &self.steps {
            if !identifier(&step.id) || !step_ids.insert(&step.id) {
                return Err(format!("invalid or duplicate step id: {}", step.id));
            }
            if let Some(value) = step.fit_clearance_mm {
                fit_clearance(value, &step.id)?;
            }
            if step.parts.is_empty() {
                return Err(format!("step {} moves no parts", step.id));
            }
            for part in &step.parts {
                if !part_ids.contains(part) {
                    return Err(format!("step {} refers to unknown part {part}", step.id));
                }
                // 取り除いた部品は以降の状態に無い。同じstep内の重複も同じ扱いとする。
                if !removed.insert(part) {
                    return Err(format!(
                        "step {} moves part {part} that is already removed",
                        step.id
                    ));
                }
            }
            if step.path.is_empty() || step.path.len() > MAX_SEGMENTS {
                return Err(format!(
                    "step {} requires 1..{MAX_SEGMENTS} path segments",
                    step.id
                ));
            }
            let last = step.path.len() - 1;
            for (index, segment) in step.path.iter().enumerate() {
                segment
                    .distance_mm
                    .validate()
                    .map_err(|e| format!("step {} segment {index}: {e}", step.id))?;
                // 外へ出た後に続く区間は意味を持たない。
                if index != last && segment.distance_mm == EXIT {
                    return Err(format!(
                        "step {} segment {index}: exit is allowed only on the last segment",
                        step.id
                    ));
                }
            }
        }
        Ok(())
    }
}

const EXIT: Distance = Distance::Keyword(DistanceKeyword::Exit);

impl Distance {
    fn validate(self) -> Result<(), String> {
        match self {
            Self::Millimetres(value)
                if !value.is_finite() || value <= 0.0 || value > 2.0 * COORDINATE_LIMIT_MM =>
            {
                Err("distance_mm must be positive and finite".into())
            }
            _ => Ok(()),
        }
    }
}

/// 工具・ケーブル・コネクタ、またはkeepoutを取り出す際に通る領域。
///
/// 形状は`shape`で直接与えるか、`keepout`を参照してそのclearance込みのboxを使う。
/// `after_step`を与えるとそのstepを終えた状態で評価し、取り外した部品は障害物に
/// ならない。省略すると組立完了の状態で評価する。形状は工具などの包絡であり、
/// 指の入る余地などの余裕を含める。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sweep {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shape: Option<Shape>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keepout: Option<String>,
    pub direction: Direction,
    pub distance_mm: Distance,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_step: Option<String>,
}

impl Sweep {
    fn validate(
        &self,
        keepout_ids: &BTreeSet<&String>,
        step_ids: &BTreeSet<&String>,
    ) -> Result<(), String> {
        match (&self.shape, &self.keepout) {
            (Some(shape), None) => shape
                .validate()
                .map_err(|e| format!("sweep {}: {e}", self.id))?,
            (None, Some(keepout)) => {
                if !keepout_ids.contains(keepout) {
                    return Err(format!(
                        "sweep {} refers to unknown keepout {keepout}",
                        self.id
                    ));
                }
            }
            _ => {
                return Err(format!(
                    "sweep {} requires exactly one of shape and keepout",
                    self.id
                ));
            }
        }
        if let Some(step) = &self.after_step
            && !step_ids.contains(step)
        {
            return Err(format!("sweep {} refers to unknown step {step}", self.id));
        }
        self.distance_mm
            .validate()
            .map_err(|e| format!("sweep {}: {e}", self.id))
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum Units {
    #[serde(rename = "mm")]
    Mm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pass,
    Fail,
    NotEvaluated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rule {
    FeatureThickness,
    ValidSolid,
    SingleSolid,
    KeepoutClearance,
    AccessClearance,
    PartInterference,
    MeshManifold,
    MeshVolume,
    FinalWallThickness,
    NeckSection,
    ClosedCavity,
    SupportFree,
    DisassemblyPath,
    DisassemblySeparation,
    Strength,
    Thermal,
}

impl Rule {
    pub const ALL: [Self; 16] = [
        Self::FeatureThickness,
        Self::ValidSolid,
        Self::SingleSolid,
        Self::KeepoutClearance,
        Self::AccessClearance,
        Self::PartInterference,
        Self::MeshManifold,
        Self::MeshVolume,
        Self::FinalWallThickness,
        Self::NeckSection,
        Self::ClosedCavity,
        Self::SupportFree,
        Self::DisassemblyPath,
        Self::DisassemblySeparation,
        Self::Strength,
        Self::Thermal,
    ];

    /// voxel化した最終形状から判定するrule。
    pub const VOXEL: [Self; 4] = [
        Self::FinalWallThickness,
        Self::NeckSection,
        Self::ClosedCavity,
        Self::SupportFree,
    ];
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub min_feature_mm: f64,
    /// 出力STLとsolidの体積差の相対許容量。tessellationの弦誤差を吸収する。
    #[serde(default = "default_mesh_volume_tolerance")]
    pub mesh_volume_tolerance: f64,
    /// 最終形状をrasterizeする格子の間隔。単位はmm。細かいほど検査は正確になり、
    /// cell数は3乗で増える。
    #[serde(default = "default_voxel_mm")]
    pub voxel_mm: f64,
    /// 最終形状に要求する最小肉厚。単位はmm。primitive寸法とは別に指定する。
    #[serde(default = "default_min_wall_mm")]
    pub min_wall_mm: f64,
    /// 接続部に要求する最小断面。単位はmm。
    #[serde(default = "default_min_neck_mm")]
    pub min_neck_mm: f64,
    /// 印刷時に上となる方向。層はこの軸に沿って積む。
    #[serde(default = "default_build_direction")]
    pub build_direction: Direction,
    /// 支持なしで許す、印刷方向に対する最大傾斜角。単位は度。
    #[serde(default = "default_overhang_angle_deg")]
    pub overhang_angle_deg: f64,
    /// 両端が支持された未支持区間の許容長。単位はmm。
    #[serde(default = "default_bridge_max_mm")]
    pub bridge_max_mm: f64,
    pub required: Vec<Rule>,
}

fn default_mesh_volume_tolerance() -> f64 {
    0.01
}

fn default_voxel_mm() -> f64 {
    0.2
}

fn default_min_wall_mm() -> f64 {
    1.2
}

fn default_min_neck_mm() -> f64 {
    1.2
}

fn default_build_direction() -> Direction {
    Direction::PlusZ
}

fn default_overhang_angle_deg() -> f64 {
    45.0
}

fn default_bridge_max_mm() -> f64 {
    5.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Model {
    pub schema_version: u32,
    pub units: Units,
    pub parts: Vec<Part>,
    pub keepouts: Vec<Keepout>,
    #[serde(default)]
    pub sweeps: Vec<Sweep>,
    #[serde(default)]
    pub assembly: Assembly,
    pub policy: Policy,
}

fn identifier(id: &str) -> bool {
    let reserved = [
        "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
        "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
    ];
    !id.is_empty()
        && id.len() <= 64
        && id.as_bytes()[0].is_ascii_lowercase()
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        && !reserved.contains(&id)
}

/// v1の`bounds`をv2の`shape`へ移す。v1は軸平行boxだけを表現できる。
fn upgrade_bounds(value: &mut Value) -> Result<(), String> {
    let object = value
        .as_object_mut()
        .ok_or("feature and keepout entries must be JSON objects")?;
    if let Some(mut bounds) = object.remove("bounds") {
        bounds
            .as_object_mut()
            .ok_or("bounds must be a JSON object")?
            .insert("kind".into(), json!("box"));
        object.insert("shape".into(), bounds);
    }
    Ok(())
}

/// schema v1のJSONをv2の構造へ変換する。v1は軸平行box、一様clearance、単一accessだけを
/// 表現できるため、対応は一意に定まる。
fn upgrade_v1(value: &mut Value) -> Result<(), String> {
    let root = value.as_object_mut().ok_or("model must be a JSON object")?;
    root.insert("schema_version".into(), json!(2));
    if let Some(parts) = root.get_mut("parts").and_then(Value::as_array_mut) {
        for part in parts {
            let features = part
                .as_object_mut()
                .ok_or("part entries must be JSON objects")?
                .get_mut("features")
                .and_then(Value::as_array_mut);
            for feature in features.into_iter().flatten() {
                upgrade_bounds(feature)?;
            }
        }
    }
    if let Some(keepouts) = root.get_mut("keepouts").and_then(Value::as_array_mut) {
        for keepout in keepouts {
            upgrade_bounds(keepout)?;
            let object = keepout
                .as_object_mut()
                .ok_or("keepout entries must be JSON objects")?;
            if let Some(uniform) = object.get("clearance_mm").and_then(Value::as_f64) {
                object.insert("clearance_mm".into(), json!({ "default": uniform }));
            }
            let access = match object.get("access") {
                None | Some(Value::Null) => json!([]),
                Some(Value::String(single)) => json!([single]),
                Some(other) => other.clone(),
            };
            object.insert("access".into(), access);
        }
    }
    Ok(())
}

/// schema v2のJSONをv3へ変換する。v2は分解手順を持たないため、空のassemblyを補う。
fn upgrade_v2(value: &mut Value) -> Result<(), String> {
    let root = value.as_object_mut().ok_or("model must be a JSON object")?;
    if root.contains_key("assembly") {
        return Err("schema_version 2 does not define assembly".into());
    }
    root.insert("schema_version".into(), json!(3));
    root.insert("assembly".into(), json!(Assembly::default()));
    Ok(())
}

/// schema v3のJSONをv4へ変換する。keepoutの`access`は、組立完了の状態で外まで
/// 掃引する`sweeps`へ移す。idは`<keepout>_<direction>`とする。
fn upgrade_v3(value: &mut Value) -> Result<(), String> {
    let root = value.as_object_mut().ok_or("model must be a JSON object")?;
    if root.contains_key("sweeps") {
        return Err("schema_version 3 does not define sweeps".into());
    }
    let mut sweeps = Vec::new();
    if let Some(keepouts) = root.get_mut("keepouts").and_then(Value::as_array_mut) {
        for keepout in keepouts {
            let object = keepout
                .as_object_mut()
                .ok_or("keepout entries must be JSON objects")?;
            let id = object
                .get("id")
                .and_then(Value::as_str)
                .ok_or("keepout requires a string id")?
                .to_owned();
            let access = object.remove("access").unwrap_or(json!([]));
            let directions = access
                .as_array()
                .ok_or_else(|| format!("keepout {id}: access must be an array"))?;
            for direction in directions {
                let name = direction
                    .as_str()
                    .ok_or_else(|| format!("keepout {id}: access entries must be strings"))?;
                let sweep_id = format!("{id}_{name}");
                if !identifier(&sweep_id) {
                    return Err(format!(
                        "cannot upgrade access of keepout {id}: sweep id {sweep_id} exceeds the id rules; shorten the keepout id"
                    ));
                }
                sweeps.push(json!({
                    "id": sweep_id, "keepout": id, "direction": name, "distance_mm": "exit",
                }));
            }
        }
    }
    root.insert("schema_version".into(), json!(SCHEMA_VERSION));
    root.insert("sweeps".into(), Value::Array(sweeps));
    Ok(())
}

impl Model {
    pub fn from_json(json: &str) -> Result<Self, String> {
        if json.len() > 1_000_000 {
            return Err("model exceeds 1 MB limit".into());
        }
        let mut value: Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
        match value.get("schema_version").and_then(Value::as_u64) {
            Some(1) => {
                upgrade_v1(&mut value)?;
                upgrade_v2(&mut value)?;
                upgrade_v3(&mut value)?;
            }
            Some(2) => {
                upgrade_v2(&mut value)?;
                upgrade_v3(&mut value)?;
            }
            Some(3) => upgrade_v3(&mut value)?,
            Some(version) if version == u64::from(SCHEMA_VERSION) => {}
            _ => {
                return Err(format!(
                    "unsupported schema_version; this build reads 1 to {SCHEMA_VERSION}"
                ));
            }
        }
        let model: Self = serde_json::from_value(value).map_err(|e| e.to_string())?;
        model.validate()?;
        Ok(model)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != SCHEMA_VERSION {
            return Err("unsupported schema_version".into());
        }
        if self.parts.is_empty() || self.parts.len() > 100 {
            return Err("model requires 1..100 parts".into());
        }
        if self.keepouts.len() > 100 {
            return Err("at most 100 keepouts are supported".into());
        }
        let thickness = self.policy.min_feature_mm;
        if !thickness.is_finite() || !(0.001..=1000.0).contains(&thickness) {
            return Err("min_feature_mm must be finite and in [0.001, 1000]".into());
        }
        let tolerance = self.policy.mesh_volume_tolerance;
        if !tolerance.is_finite() || !(0.0..=1.0).contains(&tolerance) {
            return Err("mesh_volume_tolerance must be finite and in [0, 1]".into());
        }
        let pitch = self.policy.voxel_mm;
        if !pitch.is_finite() || !(0.01..=10.0).contains(&pitch) {
            return Err("voxel_mm must be finite and in [0.01, 10]".into());
        }
        for (name, value) in [
            ("min_wall_mm", self.policy.min_wall_mm),
            ("min_neck_mm", self.policy.min_neck_mm),
        ] {
            if !value.is_finite() || !(0.01..=1000.0).contains(&value) {
                return Err(format!("{name} must be finite and in [0.01, 1000]"));
            }
        }
        let overhang = self.policy.overhang_angle_deg;
        if !overhang.is_finite() || !(0.0..=89.0).contains(&overhang) {
            return Err("overhang_angle_deg must be finite and in [0, 89]".into());
        }
        let bridge = self.policy.bridge_max_mm;
        if !bridge.is_finite() || !(0.0..=1000.0).contains(&bridge) {
            return Err("bridge_max_mm must be finite and in [0, 1000]".into());
        }
        let mut required = BTreeSet::new();
        for rule in &self.policy.required {
            if !required.insert(rule) {
                return Err("duplicate required rule".into());
            }
        }
        let mut part_ids = BTreeSet::new();
        let mut total = 0;
        for part in &self.parts {
            if !identifier(&part.id) || !part_ids.insert(&part.id) {
                return Err(format!("invalid or duplicate part id: {}", part.id));
            }
            if !part.features.iter().any(|f| f.operation == Operation::Add) {
                return Err(format!("{} requires additive geometry", part.id));
            }
            total += part.features.len();
            if total > 1000 {
                return Err("at most 1000 features are supported".into());
            }
            let mut ids = BTreeSet::new();
            for feature in &part.features {
                if !identifier(&feature.id) || !ids.insert(&feature.id) {
                    return Err(format!("invalid or duplicate feature id: {}", feature.id));
                }
                feature
                    .shape
                    .validate()
                    .map_err(|e| format!("{}/{}: {e}", part.id, feature.id))?;
            }
            // rasterizeできない大きさを受理して評価時に失敗させるより、入力の時点で拒否する。
            match voxel::grid_cells(part, pitch) {
                Some(cells) if cells <= voxel::MAX_GRID_CELLS => {}
                _ => {
                    return Err(format!(
                        "{} exceeds the voxel grid limit of {} cells at voxel_mm={pitch}; use a coarser grid or split the part",
                        part.id,
                        voxel::MAX_GRID_CELLS
                    ));
                }
            }
        }
        let mut ids = BTreeSet::new();
        for keepout in &self.keepouts {
            if !identifier(&keepout.id) || !ids.insert(&keepout.id) {
                return Err(format!("invalid or duplicate keepout id: {}", keepout.id));
            }
            keepout.shape.validate()?;
            // 面別clearanceと、それを掃引に使うkeepout参照はboxの面を前提とする。
            // cylinderのkeepoutは対応する面が定まらないため受理しない。
            if !keepout.shape.is_box() {
                return Err(format!("keepout {} must be a box", keepout.id));
            }
            keepout.clearance_mm.validate()?;
        }
        self.assembly.validate(&part_ids)?;
        if self.sweeps.len() > MAX_SWEEPS {
            return Err(format!("at most {MAX_SWEEPS} sweeps are supported"));
        }
        let step_ids: BTreeSet<&String> = self.assembly.steps.iter().map(|s| &s.id).collect();
        let mut sweep_ids = BTreeSet::new();
        for sweep in &self.sweeps {
            if !identifier(&sweep.id) || !sweep_ids.insert(&sweep.id) {
                return Err(format!("invalid or duplicate sweep id: {}", sweep.id));
            }
            sweep.validate(&ids, &step_ids)?;
        }
        Ok(())
    }

    pub fn preflight(&self) -> Result<Report, String> {
        self.validate()?;
        let mut report = Report {
            checks: Vec::new(),
            required: self.policy.required.clone(),
        };
        for part in &self.parts {
            for feature in &part.features {
                if feature.operation == Operation::Add {
                    let measured = feature.shape.minimum_dimension();
                    report.checks.push(Check {
                        rule: Rule::FeatureThickness,
                        status: if measured >= self.policy.min_feature_mm { Status::Pass } else { Status::Fail },
                        target: format!("{}/{}", part.id, feature.id),
                        message: format!("primitive minimum dimension: {measured} mm; required: {} mm; does not measure post-cut wall thickness", self.policy.min_feature_mm),
                    });
                }
            }
        }
        for rule in Rule::ALL
            .into_iter()
            .filter(|r| *r != Rule::FeatureThickness)
        {
            report.checks.push(Check {
                rule,
                status: Status::NotEvaluated,
                target: "model".into(),
                message: "not evaluated by semantic preflight".into(),
            });
        }
        Ok(report)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Check {
    pub rule: Rule,
    pub status: Status,
    pub target: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Report {
    pub checks: Vec<Check>,
    pub required: Vec<Rule>,
}

impl Report {
    /// 未実施の必須検査を成功扱いしない。幾何学的証拠の真正性はbackendの責務。
    pub fn export_allowed(&self) -> bool {
        !self.checks.iter().any(|c| c.status == Status::Fail)
            && self.required.iter().all(|rule| {
                let checks: Vec<_> = self.checks.iter().filter(|c| c.rule == *rule).collect();
                !checks.is_empty() && checks.iter().all(|c| c.status == Status::Pass)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plate() -> Shape {
        Shape::Box {
            min: [0.; 3],
            max: [10., 10., 2.],
        }
    }

    fn model() -> Model {
        Model {
            schema_version: SCHEMA_VERSION,
            units: Units::Mm,
            parts: vec![Part {
                id: "base".into(),
                features: vec![Feature {
                    id: "plate".into(),
                    role: Role::Base,
                    operation: Operation::Add,
                    shape: plate(),
                }],
            }],
            keepouts: vec![],
            sweeps: vec![],
            assembly: Assembly::default(),
            policy: Policy {
                min_feature_mm: 1.2,
                mesh_volume_tolerance: 0.01,
                voxel_mm: 0.2,
                min_wall_mm: 1.2,
                min_neck_mm: 1.2,
                build_direction: Direction::PlusZ,
                overhang_angle_deg: 45.0,
                bridge_max_mm: 5.0,
                required: vec![Rule::SingleSolid],
            },
        }
    }

    fn set_max(model: &mut Model, axis: usize, value: f64) {
        if let Shape::Box { max, .. } = &mut model.parts[0].features[0].shape {
            max[axis] = value;
        }
    }

    #[test]
    fn json_round_trip() {
        let json = serde_json::to_string(&model()).unwrap();
        assert!(Model::from_json(&json).is_ok());
    }

    #[test]
    fn preflight_is_not_geometry_approval() {
        assert!(!model().preflight().unwrap().export_allowed());
    }

    #[test]
    fn thin_feature_fails() {
        let mut m = model();
        set_max(&mut m, 2, 0.4);
        assert_eq!(m.preflight().unwrap().checks[0].status, Status::Fail);
    }

    #[test]
    fn boundary_thickness_passes() {
        let mut m = model();
        set_max(&mut m, 2, 1.2);
        assert_eq!(m.preflight().unwrap().checks[0].status, Status::Pass);
    }

    #[test]
    fn invalid_numbers_rejected() {
        for n in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1., 0., 1e7] {
            let mut m = model();
            set_max(&mut m, 0, n);
            assert!(m.validate().is_err());
        }
    }

    #[test]
    fn invalid_policy_rejected() {
        for n in [f64::NAN, f64::INFINITY, -1., 0.] {
            let mut m = model();
            m.policy.min_feature_mm = n;
            assert!(m.validate().is_err());
        }
        for n in [f64::NAN, f64::INFINITY, -1., 1.5] {
            let mut m = model();
            m.policy.mesh_volume_tolerance = n;
            assert!(m.validate().is_err());
        }
    }

    #[test]
    fn mesh_volume_tolerance_defaults_when_absent() {
        let mut value = serde_json::to_value(model()).unwrap();
        value["policy"]
            .as_object_mut()
            .unwrap()
            .remove("mesh_volume_tolerance");
        let restored = Model::from_json(&value.to_string()).unwrap();
        assert_eq!(restored.policy.mesh_volume_tolerance, 0.01);
    }

    #[test]
    fn duplicate_ids_rejected() {
        let mut m = model();
        m.parts.push(m.parts[0].clone());
        assert!(m.validate().is_err());
    }

    #[test]
    fn empty_or_cut_only_part_rejected() {
        let mut m = model();
        m.parts[0].features[0].operation = Operation::Cut;
        assert!(m.validate().is_err());
        m.parts.clear();
        assert!(m.validate().is_err());
    }

    #[test]
    fn unsafe_names_rejected() {
        for id in ["../escape", "a/b", "", "CON", "con", "lpt1", "a.b", "a\\b"] {
            assert!(!identifier(id));
        }
        assert!(identifier("board_tray"));
    }

    #[test]
    fn unknown_fields_and_version_rejected() {
        let mut m = model();
        m.schema_version = SCHEMA_VERSION + 1;
        assert!(m.validate().is_err());
        assert!(
            serde_json::from_str::<Shape>(r#"{"kind":"box","min":[0,0,0],"max":[1,1,1],"typo":1}"#)
                .is_err()
        );
        assert!(serde_json::from_str::<Shape>(r#"{"min":[0,0,0],"max":[1,1,1]}"#).is_err());
    }

    #[test]
    fn cylinder_measures_diameter_and_height() {
        let shape = Shape::Cylinder {
            axis: Axis::Z,
            center: [5., 5.],
            radius: 1.6,
            span: [0., 10.],
        };
        assert_eq!(shape.minimum_dimension(), 3.2);
        let (low, high) = shape.aabb();
        assert_eq!(low, [3.4, 3.4, 0.]);
        assert_eq!(high, [6.6, 6.6, 10.]);
        assert!(shape.validate().is_ok());
    }

    #[test]
    fn cylinder_axes_place_the_centre_on_the_perpendicular_plane() {
        for (axis, expected_low, expected_high) in [
            (Axis::X, [0., 1., 3.], [10., 3., 5.]),
            (Axis::Y, [1., 0., 3.], [3., 10., 5.]),
            (Axis::Z, [1., 3., 0.], [3., 5., 10.]),
        ] {
            let shape = Shape::Cylinder {
                axis,
                center: [2., 4.],
                radius: 1.,
                span: [0., 10.],
            };
            let (low, high) = shape.aabb();
            assert_eq!((low, high), (expected_low, expected_high), "{axis:?}");
        }
    }

    #[test]
    fn invalid_cylinders_rejected() {
        let base = |radius: f64, span: [f64; 2]| Shape::Cylinder {
            axis: Axis::Z,
            center: [0., 0.],
            radius,
            span,
        };
        for shape in [
            base(0., [0., 10.]),
            base(-1., [0., 10.]),
            base(f64::NAN, [0., 10.]),
            base(0.0004, [0., 10.]),
            base(1., [0., 0.0005]),
            base(1., [10., 0.]),
            // 半径を足すと座標上限を超える。
            base(2., [0., 10.]).shifted(1e6),
        ] {
            assert!(shape.validate().is_err(), "{shape:?}");
        }
    }

    impl Shape {
        /// testで境界条件を作るため、垂直平面上の中心を移動する。
        fn shifted(self, offset: f64) -> Self {
            match self {
                Self::Cylinder {
                    axis,
                    center,
                    radius,
                    span,
                } => Self::Cylinder {
                    axis,
                    center: [center[0] + offset, center[1] + offset],
                    radius,
                    span,
                },
                other => other,
            }
        }
    }

    #[test]
    fn keepout_must_be_a_box() {
        let mut m = model();
        m.keepouts.push(Keepout {
            id: "pcb".into(),
            shape: Shape::Cylinder {
                axis: Axis::Z,
                center: [0., 0.],
                radius: 1.,
                span: [0., 1.],
            },
            clearance_mm: Clearance {
                default: 0.5,
                faces: BTreeMap::new(),
            },
        });
        assert!(m.validate().is_err());
    }

    #[test]
    fn clearance_falls_back_to_the_default() {
        let clearance = Clearance {
            default: 0.5,
            faces: BTreeMap::from([(Direction::MinusZ, 0.0)]),
        };
        assert_eq!(clearance.for_face(Direction::MinusZ), 0.0);
        assert_eq!(clearance.for_face(Direction::PlusZ), 0.5);
        assert!(clearance.validate().is_ok());
    }

    #[test]
    fn clearance_reads_faces_beside_the_default() {
        let clearance: Clearance =
            serde_json::from_str(r#"{"default":0.5,"minus_z":0.0}"#).unwrap();
        assert_eq!(clearance.for_face(Direction::MinusZ), 0.0);
        assert_eq!(clearance.for_face(Direction::PlusX), 0.5);
        // 面の名前として解釈できないkeyは受理しない。
        assert!(serde_json::from_str::<Clearance>(r#"{"default":0.5,"typo":0.0}"#).is_err());
    }

    fn with_keepout() -> Model {
        let mut m = model();
        m.keepouts.push(Keepout {
            id: "pcb".into(),
            shape: plate(),
            clearance_mm: Clearance {
                default: 0.5,
                faces: BTreeMap::new(),
            },
        });
        m
    }

    fn sweep(id: &str) -> Sweep {
        Sweep {
            id: id.into(),
            shape: None,
            keepout: Some("pcb".into()),
            direction: Direction::PlusZ,
            distance_mm: EXIT,
            after_step: None,
        }
    }

    #[test]
    fn sweeps_are_validated() {
        let driver = Shape::Cylinder {
            axis: Axis::Z,
            center: [5., 5.],
            radius: 1.,
            span: [2., 6.],
        };
        let cases: Vec<(&str, Sweep)> = vec![
            ("duplicate id", sweep("a")),
            (
                "unknown keepout",
                Sweep {
                    keepout: Some("ghost".into()),
                    ..sweep("b")
                },
            ),
            (
                "both shape and keepout",
                Sweep {
                    shape: Some(driver.clone()),
                    ..sweep("b")
                },
            ),
            (
                "neither shape nor keepout",
                Sweep {
                    keepout: None,
                    ..sweep("b")
                },
            ),
            (
                "unknown step",
                Sweep {
                    after_step: Some("open_lid".into()),
                    ..sweep("b")
                },
            ),
            (
                "zero distance",
                Sweep {
                    distance_mm: Distance::Millimetres(0.0),
                    ..sweep("b")
                },
            ),
            ("invalid id", sweep("../b")),
            (
                "invalid shape",
                Sweep {
                    shape: Some(Shape::Box {
                        min: [0.; 3],
                        max: [0.; 3],
                    }),
                    keepout: None,
                    ..sweep("b")
                },
            ),
        ];
        for (name, bad) in cases {
            let mut m = with_keepout();
            m.sweeps = vec![sweep("a"), bad];
            assert!(m.validate().is_err(), "{name}");
        }
        let mut m = with_keepout();
        m.sweeps = vec![
            sweep("a"),
            Sweep {
                shape: Some(driver),
                keepout: None,
                distance_mm: Distance::Millimetres(10.0),
                ..sweep("driver")
            },
        ];
        assert!(m.validate().is_ok());
    }

    #[test]
    fn sweep_after_a_step_refers_to_it() {
        let mut m = two_parts();
        m.keepouts = with_keepout().keepouts;
        m.assembly.steps = vec![step(
            "open_lid",
            &["lid"],
            vec![segment(Direction::PlusZ, EXIT)],
        )];
        m.sweeps = vec![Sweep {
            after_step: Some("open_lid".into()),
            ..sweep("pcb_out")
        }];
        assert!(m.validate().is_ok());
        let json = serde_json::to_string(&m).unwrap();
        assert!(json.contains(r#""after_step":"open_lid""#), "{json}");
        assert!(!json.contains(r#""shape":null"#), "{json}");
    }

    #[test]
    fn schema_v3_access_becomes_sweeps() {
        let v3 = r#"{
            "schema_version": 3,
            "units": "mm",
            "parts": [{"id":"base","features":[
                {"id":"plate","role":"base","operation":"add",
                 "shape":{"kind":"box","min":[0,0,0],"max":[10,10,2]}}
            ]}],
            "keepouts": [{"id":"pcb","shape":{"kind":"box","min":[1,1,1],"max":[2,2,2]},
                          "clearance_mm":{"default":0.5},"access":["plus_z","minus_x"]},
                         {"id":"cable","shape":{"kind":"box","min":[3,3,3],"max":[4,4,4]},
                          "clearance_mm":{"default":0.5},"access":[]}],
            "assembly": {"fit_clearance_mm":0.0,"steps":[]},
            "policy": {"min_feature_mm":1.2,"required":["single_solid"]}
        }"#;
        let model = Model::from_json(v3).unwrap();
        assert_eq!(model.schema_version, SCHEMA_VERSION);
        let ids: Vec<&str> = model.sweeps.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["pcb_plus_z", "pcb_minus_x"]);
        for sweep in &model.sweeps {
            assert_eq!(sweep.keepout.as_deref(), Some("pcb"));
            assert_eq!(sweep.distance_mm, EXIT);
            assert!(sweep.after_step.is_none());
        }
        // 重複した方向は同じidの掃引となり、検証で拒否される。
        let duplicated = v3.replace(r#"["plus_z","minus_x"]"#, r#"["plus_z","plus_z"]"#);
        assert!(Model::from_json(&duplicated).is_err());
        // v3はsweepsを定義していない。昇格前の入力に現れたら拒否する。
        let with_sweeps = v3.replace(r#""assembly""#, r#""sweeps": [], "assembly""#);
        assert!(Model::from_json(&with_sweeps).is_err());
    }

    #[test]
    fn upgrade_rejects_an_access_id_that_would_be_too_long() {
        let long = "k".repeat(60);
        let v3 = format!(
            r#"{{"schema_version":3,"units":"mm",
                "parts":[{{"id":"base","features":[{{"id":"plate","role":"base","operation":"add",
                  "shape":{{"kind":"box","min":[0,0,0],"max":[10,10,2]}}}}]}}],
                "keepouts":[{{"id":"{long}","shape":{{"kind":"box","min":[1,1,1],"max":[2,2,2]}},
                  "clearance_mm":{{"default":0.5}},"access":["plus_z"]}}],
                "policy":{{"min_feature_mm":1.2,"required":[]}}}}"#
        );
        let error = Model::from_json(&v3).unwrap_err();
        assert!(error.contains("shorten the keepout id"), "{error}");
    }

    #[test]
    fn schema_v1_is_upgraded() {
        let v1 = r#"{
            "schema_version": 1,
            "units": "mm",
            "parts": [{"id":"base","features":[
                {"id":"plate","role":"base","operation":"add",
                 "bounds":{"min":[0,0,0],"max":[10,10,2]}}
            ]}],
            "keepouts": [{"id":"pcb","bounds":{"min":[1,1,1],"max":[2,2,2]},
                          "clearance_mm":0.25,"access":"plus_z"}],
            "policy": {"min_feature_mm":1.2,"required":["single_solid"]}
        }"#;
        let model = Model::from_json(v1).unwrap();
        assert_eq!(model.schema_version, SCHEMA_VERSION);
        assert!(model.parts[0].features[0].shape.is_box());
        let keepout = &model.keepouts[0];
        assert_eq!(keepout.clearance_mm.for_face(Direction::PlusX), 0.25);
        let ids: Vec<&str> = model.sweeps.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["pcb_plus_z"]);
    }

    #[test]
    fn schema_v1_without_access_upgrades_to_no_directions() {
        let v1 = r#"{
            "schema_version": 1,
            "units": "mm",
            "parts": [{"id":"base","features":[
                {"id":"plate","role":"base","operation":"add",
                 "bounds":{"min":[0,0,0],"max":[10,10,2]}}
            ]}],
            "keepouts": [{"id":"pcb","bounds":{"min":[1,1,1],"max":[2,2,2]},
                          "clearance_mm":0.5,"access":null}],
            "policy": {"min_feature_mm":1.2,"required":["single_solid"]}
        }"#;
        assert!(Model::from_json(v1).unwrap().sweeps.is_empty());
    }

    #[test]
    fn unknown_schema_version_rejected() {
        for version in ["0", "5", "\"2\""] {
            let json = format!(
                r#"{{"schema_version":{version},"units":"mm","parts":[],"keepouts":[],
                     "policy":{{"min_feature_mm":1.2,"required":[]}}}}"#
            );
            assert!(Model::from_json(&json).is_err(), "{version}");
        }
    }

    fn two_parts() -> Model {
        let mut m = model();
        m.parts.push(Part {
            id: "lid".into(),
            features: vec![Feature {
                id: "panel".into(),
                role: Role::Generic,
                operation: Operation::Add,
                shape: Shape::Box {
                    min: [0., 0., 2.],
                    max: [10., 10., 4.],
                },
            }],
        });
        m
    }

    fn step(id: &str, parts: &[&str], path: Vec<Move>) -> Step {
        Step {
            id: id.into(),
            parts: parts.iter().map(|p| (*p).into()).collect(),
            path,
            fit_clearance_mm: None,
        }
    }

    fn segment(direction: Direction, distance: Distance) -> Move {
        Move {
            direction,
            distance_mm: distance,
        }
    }

    #[test]
    fn schema_v2_is_upgraded_with_an_empty_assembly() {
        let v2 = r#"{
            "schema_version": 2,
            "units": "mm",
            "parts": [{"id":"base","features":[
                {"id":"plate","role":"base","operation":"add",
                 "shape":{"kind":"box","min":[0,0,0],"max":[10,10,2]}}
            ]}],
            "keepouts": [],
            "policy": {"min_feature_mm":1.2,"required":["single_solid"]}
        }"#;
        let model = Model::from_json(v2).unwrap();
        assert_eq!(model.schema_version, SCHEMA_VERSION);
        assert!(model.assembly.steps.is_empty());
        assert_eq!(model.assembly.fit_clearance_mm, 0.0);
        // v2はassemblyを定義していない。昇格前の入力に現れたら拒否する。
        let with_assembly = v2.replace(r#""keepouts": [],"#, r#""keepouts": [], "assembly": {},"#);
        assert!(Model::from_json(&with_assembly).is_err());
    }

    #[test]
    fn assembly_round_trips_numbers_and_exit() {
        let mut m = two_parts();
        m.assembly.fit_clearance_mm = 0.2;
        m.assembly.steps.push(Step {
            fit_clearance_mm: Some(0.3),
            ..step(
                "open_lid",
                &["lid"],
                vec![
                    segment(Direction::PlusX, Distance::Millimetres(5.0)),
                    segment(Direction::PlusZ, EXIT),
                ],
            )
        });
        let json = serde_json::to_string(&m).unwrap();
        assert!(json.contains(r#""distance_mm":5.0"#), "{json}");
        assert!(json.contains(r#""distance_mm":"exit""#), "{json}");
        let back = Model::from_json(&json).unwrap();
        assert_eq!(back.assembly.steps[0].path[1].distance_mm, EXIT);
        assert_eq!(back.assembly.steps[0].fit_clearance_mm, Some(0.3));
    }

    #[test]
    fn assembly_without_steps_omits_nothing_and_validates() {
        let m = model();
        let json = serde_json::to_string(&m).unwrap();
        assert!(
            json.contains(r#""assembly":{"fit_clearance_mm":0.0,"steps":[]}"#),
            "{json}"
        );
        assert!(m.validate().is_ok());
    }

    #[test]
    fn invalid_steps_are_rejected() {
        let lift = || vec![segment(Direction::PlusZ, EXIT)];
        let cases: Vec<(&str, Vec<Step>, f64)> = vec![
            ("unknown part", vec![step("a", &["ghost"], lift())], 0.0),
            ("empty parts", vec![step("a", &[], lift())], 0.0),
            ("empty path", vec![step("a", &["lid"], vec![])], 0.0),
            (
                "duplicate step id",
                vec![step("a", &["lid"], lift()), step("a", &["base"], lift())],
                0.0,
            ),
            (
                "removed twice",
                vec![step("a", &["lid"], lift()), step("b", &["lid"], lift())],
                0.0,
            ),
            (
                "duplicate within a step",
                vec![step("a", &["lid", "lid"], lift())],
                0.0,
            ),
            ("invalid step id", vec![step("../a", &["lid"], lift())], 0.0),
            (
                "exit before the last segment",
                vec![step(
                    "a",
                    &["lid"],
                    vec![
                        segment(Direction::PlusZ, EXIT),
                        segment(Direction::PlusX, Distance::Millimetres(1.0)),
                    ],
                )],
                0.0,
            ),
            (
                "zero distance",
                vec![step(
                    "a",
                    &["lid"],
                    vec![segment(Direction::PlusZ, Distance::Millimetres(0.0))],
                )],
                0.0,
            ),
            (
                "negative distance",
                vec![step(
                    "a",
                    &["lid"],
                    vec![segment(Direction::PlusZ, Distance::Millimetres(-1.0))],
                )],
                0.0,
            ),
            (
                "non-finite distance",
                vec![step(
                    "a",
                    &["lid"],
                    vec![segment(
                        Direction::PlusZ,
                        Distance::Millimetres(f64::INFINITY),
                    )],
                )],
                0.0,
            ),
            (
                "too many segments",
                vec![step(
                    "a",
                    &["lid"],
                    vec![segment(Direction::PlusZ, Distance::Millimetres(1.0)); MAX_SEGMENTS + 1],
                )],
                0.0,
            ),
            (
                "negative clearance",
                vec![step("a", &["lid"], lift())],
                -0.1,
            ),
            (
                "non-finite clearance",
                vec![step("a", &["lid"], lift())],
                f64::NAN,
            ),
        ];
        for (name, steps, clearance) in cases {
            let mut m = two_parts();
            m.assembly.steps = steps;
            m.assembly.fit_clearance_mm = clearance;
            assert!(m.validate().is_err(), "{name}");
        }
        let mut m = two_parts();
        m.assembly.steps = vec![Step {
            fit_clearance_mm: Some(-1.0),
            ..step("a", &["lid"], lift())
        }];
        assert!(m.validate().is_err(), "negative step clearance");
    }

    #[test]
    fn every_part_may_be_removed() {
        let mut m = two_parts();
        m.assembly.steps = vec![
            step("a", &["lid"], vec![segment(Direction::PlusZ, EXIT)]),
            step(
                "b",
                &["base"],
                vec![segment(Direction::MinusZ, Distance::Millimetres(3.0))],
            ),
        ];
        assert!(m.validate().is_ok());
    }

    #[test]
    fn unknown_distance_keyword_is_rejected() {
        let json = serde_json::to_string(&two_parts())
            .unwrap()
            .replace(
                r#""steps":[]"#,
                r#""steps":[{"id":"a","parts":["lid"],"path":[{"direction":"plus_z","distance_mm":"far"}]}]"#,
            );
        assert!(Model::from_json(&json).is_err());
    }

    #[test]
    fn missing_required_and_any_failure_block_export() {
        let mut r = Report {
            required: vec![Rule::SingleSolid],
            checks: vec![],
        };
        assert!(!r.export_allowed());
        r.checks.push(Check {
            rule: Rule::SingleSolid,
            status: Status::Pass,
            target: "x".into(),
            message: "one solid".into(),
        });
        assert!(r.export_allowed());
        r.checks.push(Check {
            rule: Rule::Thermal,
            status: Status::Fail,
            target: "x".into(),
            message: "too hot".into(),
        });
        assert!(!r.export_allowed());
    }
}
