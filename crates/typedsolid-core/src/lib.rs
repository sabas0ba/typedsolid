//! CADカーネルから独立した意味モデルとpreflight検証。

pub mod mesh;
pub mod voxel;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// 本buildが生成するschema。読み込みは旧版からの昇格も受理する。
pub const SCHEMA_VERSION: u32 = 9;

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
/// ネジ固定の上限。
const MAX_FASTENERS: usize = 1000;
/// 材料の上限。
const MAX_MATERIALS: usize = 100;
/// snap fitの上限。
const MAX_SNAP_FITS: usize = 1000;
/// コネクタ開口の上限。
const MAX_CONNECTORS: usize = 1000;

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
    /// `materials`のid。材料に依存する検査 (snap fit) を持つ部品では必須。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub material: Option<String>,
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
    /// 取り付けられている部品。分解stepでその部品と一緒に動き、以降の状態から除かれる。
    /// 省略した場合は外部に固定され、最後まで残る。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attached_to: Option<String>,
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

/// ネジの寸法。値は利用者が与える。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Screw {
    /// 頭の座面から先端までの長さ。
    pub length_mm: f64,
    /// ねじ部の外径。
    pub major_mm: f64,
    /// 頭の外径。
    pub head_mm: f64,
}

/// 受け側でネジを保持する方式。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Anchor {
    /// 印刷した下穴へ直接ねじ込む。
    SelfTapping { pilot_mm: f64 },
    /// 熱圧入インサート。受け側の境目と面一に埋まる。
    Insert { hole_mm: f64, length_mm: f64 },
}

/// ネジ固定。clampの部品をbaseの部品へ締める。
///
/// directionは締め込む向き (頭から先端へ)、centerは軸に垂直な面上の座標、
/// seat_mmは頭が当たる面、joint_mmはclampとbaseの境目の軸方向の座標である。
/// clampが空の場合、締める対象は部品として記述されていない (基板など)。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fastener {
    pub id: String,
    pub base: String,
    pub clamp: Vec<String>,
    pub direction: Direction,
    pub center: [f64; 2],
    pub seat_mm: f64,
    pub joint_mm: f64,
    pub screw: Screw,
    /// clampに開ける貫通穴の径。頭の座面の内径になる。
    pub through_mm: f64,
    pub anchor: Anchor,
    pub min_engagement_mm: f64,
    pub min_boss_wall_mm: f64,
    /// ネジを外す状態。省略するとネジは外さない。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<Release>,
    /// ネジで締めているkeepout (基板など部品として記述しない物)。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clamp_keepouts: Vec<String>,
}

/// ネジを外す状態。`after_step`を終えた状態で外し、省略すると組立完了の状態で外す。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Release {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_step: Option<String>,
}

fn positive_length(value: f64, name: &str, owner: &str) -> Result<(), String> {
    if !value.is_finite() || value <= 0.0 || value > COORDINATE_LIMIT_MM {
        return Err(format!("{owner}: {name} must be positive and finite"));
    }
    Ok(())
}

impl Fastener {
    fn validate(&self, part_ids: &BTreeSet<&String>) -> Result<(), String> {
        if !part_ids.contains(&self.base) {
            return Err(format!(
                "fastener {} refers to unknown base part {}",
                self.id, self.base
            ));
        }
        let mut clamps = BTreeSet::new();
        for part in &self.clamp {
            if !part_ids.contains(part) || part == &self.base || !clamps.insert(part) {
                return Err(format!(
                    "fastener {} has an invalid clamp part {part}",
                    self.id
                ));
            }
        }
        for value in [self.center[0], self.center[1], self.seat_mm, self.joint_mm] {
            finite_coordinate(value).map_err(|e| format!("fastener {}: {e}", self.id))?;
        }
        let screw = &self.screw;
        for (name, value) in [
            ("length_mm", screw.length_mm),
            ("major_mm", screw.major_mm),
            ("head_mm", screw.head_mm),
            ("through_mm", self.through_mm),
            ("min_engagement_mm", self.min_engagement_mm),
            ("min_boss_wall_mm", self.min_boss_wall_mm),
        ] {
            positive_length(value, name, &format!("fastener {}", self.id))?;
        }
        if self.through_mm < screw.major_mm || screw.head_mm <= self.through_mm {
            return Err(format!(
                "fastener {}: requires major_mm <= through_mm < head_mm",
                self.id
            ));
        }
        match self.anchor {
            Anchor::SelfTapping { pilot_mm } => {
                positive_length(pilot_mm, "pilot_mm", &format!("fastener {}", self.id))?;
                if pilot_mm >= screw.major_mm {
                    return Err(format!(
                        "fastener {}: pilot_mm must be below major_mm",
                        self.id
                    ));
                }
            }
            Anchor::Insert { hole_mm, length_mm } => {
                positive_length(hole_mm, "hole_mm", &format!("fastener {}", self.id))?;
                positive_length(length_mm, "length_mm", &format!("fastener {}", self.id))?;
                if hole_mm <= screw.major_mm {
                    return Err(format!(
                        "fastener {}: insert hole_mm must exceed major_mm",
                        self.id
                    ));
                }
            }
        }
        // 締め込む向きに見て、頭の座面が境目より手前にある。clampがあれば厚みは正。
        let sign = if self.direction.is_positive() {
            1.0
        } else {
            -1.0
        };
        let thickness = sign * (self.joint_mm - self.seat_mm);
        if thickness < 0.0 || (!self.clamp.is_empty() && thickness == 0.0) {
            return Err(format!(
                "fastener {}: joint_mm must lie beyond seat_mm along the direction",
                self.id
            ));
        }
        Ok(())
    }
}

/// 材料。値は利用者が与え、sourceに出典を書く。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Material {
    pub id: String,
    pub name: String,
    pub source: String,
    /// 曲げの許容ひずみ (無次元)。snap fitを持つ部品の材料では必須。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowable_strain: Option<f64>,
}

impl Material {
    fn validate(&self) -> Result<(), String> {
        for (name, value) in [("name", &self.name), ("source", &self.source)] {
            if value.trim().is_empty() || value.len() > 256 {
                return Err(format!(
                    "material {}: {name} must be 1 to 256 bytes",
                    self.id
                ));
            }
        }
        if let Some(strain) = self.allowable_strain
            && (!strain.is_finite() || strain <= 0.0 || strain >= 1.0)
        {
            return Err(format!(
                "material {}: allowable_strain must be in (0, 1)",
                self.id
            ));
        }
        Ok(())
    }
}

/// 矩形断面の片持ち梁によるsnap fit。
///
/// `beam`と`hook`は`part`のadd boxのfeatureである。`length_direction`は梁の根元から
/// 先端への向き、`deflection`は外すときにフックが動く向きで、`deflection_mm`だけ
/// たわませると外れる。`step`は`part`か`mate`の一方を動かし、この結合を外す分解stepである。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapFit {
    pub id: String,
    pub part: String,
    pub beam: String,
    pub hook: String,
    pub length_direction: Direction,
    pub deflection: Direction,
    pub deflection_mm: f64,
    pub mate: String,
    pub step: String,
}

/// 梁の寸法。ひずみの計算に使う。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BeamGeometry {
    /// 根元からフックの根元側の端までの長さ。荷重点を最も根元側に置くため保守側になる。
    pub arm_mm: f64,
    /// たわむ向きの厚み。
    pub thickness_mm: f64,
    /// 長さとたわみの両方に垂直な幅。
    pub width_mm: f64,
}

fn box_bounds(part: &Part, feature_id: &str) -> Option<([f64; 3], [f64; 3])> {
    part.features
        .iter()
        .find(|f| f.id == feature_id && f.operation == Operation::Add)
        .and_then(|f| match f.shape {
            Shape::Box { min, max } => Some((min, max)),
            Shape::Cylinder { .. } => None,
        })
}

impl SnapFit {
    /// 梁の寸法。featureが見つからないか、フックが梁の範囲に収まらない場合はNone。
    pub fn geometry(&self, part: &Part) -> Option<BeamGeometry> {
        let (beam_min, beam_max) = box_bounds(part, &self.beam)?;
        let (hook_min, hook_max) = box_bounds(part, &self.hook)?;
        let along = self.length_direction.axis().index();
        let across = self.deflection.axis().index();
        let width = 3 - along - across;
        let (root, hook_edge) = if self.length_direction.is_positive() {
            (beam_min[along], hook_min[along])
        } else {
            (beam_max[along], hook_max[along])
        };
        // フックは梁の長さの範囲にあり、梁に接するか重なる。
        let within = hook_min[along] >= beam_min[along] && hook_max[along] <= beam_max[along];
        let touches = (0..3).all(|i| hook_min[i] <= beam_max[i] && hook_max[i] >= beam_min[i]);
        let arm = (hook_edge - root).abs();
        (within && touches && arm > 0.0).then_some(BeamGeometry {
            arm_mm: arm,
            thickness_mm: beam_max[across] - beam_min[across],
            width_mm: beam_max[width] - beam_min[width],
        })
    }

    /// 先端荷重を受ける一様矩形断面の片持ち梁の根元表面ひずみ。
    ///
    /// たわみ δ = F·L³/(3·E·I) と曲げ応力 σ = F·L·(t/2)/I から ε = σ/E = 3·t·δ/(2·L²)。
    pub fn strain(&self, geometry: BeamGeometry) -> f64 {
        1.5 * geometry.thickness_mm * self.deflection_mm / geometry.arm_mm.powi(2)
    }

    fn validate(
        &self,
        parts: &BTreeMap<&String, &Part>,
        materials: &BTreeMap<&String, &Material>,
        steps: &[Step],
    ) -> Result<(), String> {
        let id = &self.id;
        let part = parts
            .get(&self.part)
            .ok_or_else(|| format!("snap fit {id} refers to unknown part {}", self.part))?;
        let strain = part
            .material
            .as_ref()
            .and_then(|m| materials.get(m))
            .and_then(|m| m.allowable_strain);
        if strain.is_none() {
            return Err(format!(
                "snap fit {id}: part {} needs a material with allowable_strain",
                self.part
            ));
        }
        if self.beam == self.hook
            || box_bounds(part, &self.beam).is_none()
            || box_bounds(part, &self.hook).is_none()
        {
            return Err(format!(
                "snap fit {id}: beam and hook must be distinct add boxes of part {}",
                self.part
            ));
        }
        if self.length_direction.axis() == self.deflection.axis() {
            return Err(format!(
                "snap fit {id}: deflection must be perpendicular to the length"
            ));
        }
        positive_length(
            self.deflection_mm,
            "deflection_mm",
            &format!("snap fit {id}"),
        )?;
        if self.geometry(part).is_none() {
            return Err(format!(
                "snap fit {id}: the hook must lie along the beam, away from its root"
            ));
        }
        if !parts.contains_key(&self.mate) || self.mate == self.part {
            return Err(format!("snap fit {id} has an invalid mate {}", self.mate));
        }
        let index = steps
            .iter()
            .position(|s| s.id == self.step)
            .ok_or_else(|| format!("snap fit {id} refers to unknown step {}", self.step))?;
        // 先のstepで取り外した部品は、この結合を外すstepの時点で残っていない。
        for earlier in &steps[..index] {
            for removed in [&self.part, &self.mate] {
                if earlier.parts.contains(removed) {
                    return Err(format!(
                        "snap fit {id}: {removed} is removed by step {} before step {}",
                        earlier.id, self.step
                    ));
                }
            }
        }
        let step = &steps[index];
        let moves_part = step.parts.contains(&self.part);
        let moves_mate = step.parts.contains(&self.mate);
        if moves_part == moves_mate {
            return Err(format!(
                "snap fit {id}: step {} must move exactly one of {} and {}",
                self.step, self.part, self.mate
            ));
        }
        Ok(())
    }
}

/// プラグ外形の値の出典の種別。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    /// 部品メーカーのdatasheet。referenceに品番と資料名を記す。
    Datasheet,
    /// 実物の測定。referenceに測定対象と方法を記す。
    Measured,
    /// 上記以外。referenceに根拠を記す。
    Other,
}

/// プラグ外形の値の出典。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlugSource {
    pub kind: SourceKind,
    pub reference: String,
}

/// 筐体の壁に開けるコネクタ開口。
///
/// `opening`は`part`のcut featureで、壁を貫くbox。`sweep`はプラグを抜く掃引で、
/// 嵌合状態のプラグ外形のboxを抜く向きへ動かす。`plug_mm`は抜く向きに垂直な
/// プラグ断面の寸法で、並びは`Axis::plane`の順 (x方向に抜くなら[y, z])。開口は
/// プラグとの間に各辺で`clearance_mm`以上の隙間を持つ。値は利用者が与え、
/// 根拠を`source`に記す。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Connector {
    pub id: String,
    pub part: String,
    pub opening: String,
    pub sweep: String,
    pub plug_mm: [f64; 2],
    pub clearance_mm: f64,
    pub source: PlugSource,
}

/// 出典の記述の上限。単位は文字。
const MAX_SOURCE_REFERENCE: usize = 500;
/// 寸法の一致とみなす差。単位はmm。
const CONNECTOR_TOLERANCE_MM: f64 = 1e-6;

impl Connector {
    /// 参照先の型と値域を確かめる。寸法の整合は`connector_fit`で判定する。
    fn validate(&self, parts: &BTreeMap<&String, &Part>, sweeps: &[Sweep]) -> Result<(), String> {
        let part = parts
            .get(&self.part)
            .ok_or_else(|| format!("connector {} refers to unknown part {}", self.id, self.part))?;
        let opening = part
            .features
            .iter()
            .find(|f| f.id == self.opening)
            .ok_or_else(|| {
                format!(
                    "connector {} refers to unknown feature {}/{}",
                    self.id, self.part, self.opening
                )
            })?;
        if opening.operation != Operation::Cut || !opening.shape.is_box() {
            return Err(format!(
                "connector {}: opening {} must be a cut box",
                self.id, self.opening
            ));
        }
        let sweep = sweeps.iter().find(|s| s.id == self.sweep).ok_or_else(|| {
            format!(
                "connector {} refers to unknown sweep {}",
                self.id, self.sweep
            )
        })?;
        if !sweep.shape.as_ref().is_some_and(Shape::is_box) {
            return Err(format!(
                "connector {}: sweep {} must have a box shape",
                self.id, self.sweep
            ));
        }
        for value in self.plug_mm {
            if !value.is_finite() || !(MINIMUM_EXTENT_MM..=COORDINATE_LIMIT_MM).contains(&value) {
                return Err(format!(
                    "connector {}: plug_mm must be finite and at least {MINIMUM_EXTENT_MM} mm",
                    self.id
                ));
            }
        }
        if !self.clearance_mm.is_finite() || !(0.0..=1000.0).contains(&self.clearance_mm) {
            return Err(format!(
                "connector {}: clearance_mm must be finite and in [0, 1000]",
                self.id
            ));
        }
        let reference = self.source.reference.trim();
        if reference.is_empty() || reference.chars().count() > MAX_SOURCE_REFERENCE {
            return Err(format!(
                "connector {}: source reference must have 1..{MAX_SOURCE_REFERENCE} characters",
                self.id
            ));
        }
        Ok(())
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
    FastenerFit,
    SnapFit,
    FastenerRelease,
    ConnectorFit,
    Strength,
    Thermal,
}

impl Rule {
    pub const ALL: [Self; 20] = [
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
        Self::FastenerFit,
        Self::SnapFit,
        Self::FastenerRelease,
        Self::ConnectorFit,
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

impl Policy {
    /// 値域と必須ruleの重複を確かめる。IRの検証と外部形状の評価が共有する。
    pub fn validate(&self) -> Result<(), String> {
        let thickness = self.min_feature_mm;
        if !thickness.is_finite() || !(0.001..=1000.0).contains(&thickness) {
            return Err("min_feature_mm must be finite and in [0.001, 1000]".into());
        }
        let tolerance = self.mesh_volume_tolerance;
        if !tolerance.is_finite() || !(0.0..=1.0).contains(&tolerance) {
            return Err("mesh_volume_tolerance must be finite and in [0, 1]".into());
        }
        let pitch = self.voxel_mm;
        if !pitch.is_finite() || !(0.01..=10.0).contains(&pitch) {
            return Err("voxel_mm must be finite and in [0.01, 10]".into());
        }
        for (name, value) in [
            ("min_wall_mm", self.min_wall_mm),
            ("min_neck_mm", self.min_neck_mm),
        ] {
            if !value.is_finite() || !(0.01..=1000.0).contains(&value) {
                return Err(format!("{name} must be finite and in [0.01, 1000]"));
            }
        }
        let overhang = self.overhang_angle_deg;
        if !overhang.is_finite() || !(0.0..=89.0).contains(&overhang) {
            return Err("overhang_angle_deg must be finite and in [0, 89]".into());
        }
        let bridge = self.bridge_max_mm;
        if !bridge.is_finite() || !(0.0..=1000.0).contains(&bridge) {
            return Err("bridge_max_mm must be finite and in [0, 1000]".into());
        }
        let mut required = BTreeSet::new();
        for rule in &self.required {
            if !required.insert(rule) {
                return Err("duplicate required rule".into());
            }
        }
        Ok(())
    }
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
    #[serde(default)]
    pub fasteners: Vec<Fastener>,
    #[serde(default)]
    pub materials: Vec<Material>,
    #[serde(default)]
    pub snap_fits: Vec<SnapFit>,
    #[serde(default)]
    pub connectors: Vec<Connector>,
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
    root.insert("schema_version".into(), json!(4));
    root.insert("sweeps".into(), Value::Array(sweeps));
    Ok(())
}

/// schema v4のJSONをv5へ変換する。v4はネジ固定を持たないため、空のfastenersを補う。
fn upgrade_v4(value: &mut Value) -> Result<(), String> {
    let root = value.as_object_mut().ok_or("model must be a JSON object")?;
    if root.contains_key("fasteners") {
        return Err("schema_version 4 does not define fasteners".into());
    }
    root.insert("schema_version".into(), json!(5));
    root.insert("fasteners".into(), json!([]));
    Ok(())
}

/// schema v6のJSONをv7へ変換する。v7はkeepoutの取付先を加え、keepoutを分解の障害物とする。
/// v6のkeepoutは取付先を持たないため、外部に固定されたものとしてそのまま受理する。
fn upgrade_v6(value: &mut Value) -> Result<(), String> {
    let root = value.as_object_mut().ok_or("model must be a JSON object")?;
    if let Some(keepouts) = root.get("keepouts").and_then(Value::as_array)
        && keepouts.iter().any(|k| k.get("attached_to").is_some())
    {
        return Err("schema_version 6 does not define keepout attached_to".into());
    }
    root.insert("schema_version".into(), json!(7));
    Ok(())
}

/// schema v7のJSONをv8へ変換する。v8はネジを外す状態と、ネジで締めるkeepoutを加える。
/// v7のネジはどちらも持たないため、外さないネジとしてそのまま受理する。
fn upgrade_v7(value: &mut Value) -> Result<(), String> {
    let root = value.as_object_mut().ok_or("model must be a JSON object")?;
    if let Some(fasteners) = root.get("fasteners").and_then(Value::as_array)
        && fasteners
            .iter()
            .any(|f| f.get("release").is_some() || f.get("clamp_keepouts").is_some())
    {
        return Err("schema_version 7 does not define fastener release or clamp_keepouts".into());
    }
    root.insert("schema_version".into(), json!(8));
    Ok(())
}

/// schema v8のJSONをv9へ変換する。v9はコネクタ開口を加える。v8はこれを持たないため、
/// 空のconnectorsを補う。
fn upgrade_v8(value: &mut Value) -> Result<(), String> {
    let root = value.as_object_mut().ok_or("model must be a JSON object")?;
    if root.contains_key("connectors") {
        return Err("schema_version 8 does not define connectors".into());
    }
    root.insert("schema_version".into(), json!(SCHEMA_VERSION));
    root.insert("connectors".into(), json!([]));
    Ok(())
}

/// 1版分の昇格。
type Upgrade = fn(&mut Value) -> Result<(), String>;

/// schema v5のJSONをv6へ変換する。v5は材料とsnap fitを持たないため、空の配列を補う。
/// 部品の`material`は省略可能であり、v5の部品はそのまま受理する。
fn upgrade_v5(value: &mut Value) -> Result<(), String> {
    let root = value.as_object_mut().ok_or("model must be a JSON object")?;
    for key in ["materials", "snap_fits"] {
        if root.contains_key(key) {
            return Err(format!("schema_version 5 does not define {key}"));
        }
    }
    if let Some(parts) = root.get("parts").and_then(Value::as_array)
        && parts.iter().any(|p| p.get("material").is_some())
    {
        return Err("schema_version 5 does not define part material".into());
    }
    root.insert("schema_version".into(), json!(6));
    root.insert("materials".into(), json!([]));
    root.insert("snap_fits".into(), json!([]));
    Ok(())
}

impl Model {
    pub fn from_json(json: &str) -> Result<Self, String> {
        if json.len() > 1_000_000 {
            return Err("model exceeds 1 MB limit".into());
        }
        let mut value: Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
        // 版nのJSONはUPGRADES[n-1..]を順に通してSCHEMA_VERSIONへ昇格する。
        const UPGRADES: [Upgrade; SCHEMA_VERSION as usize - 1] = [
            upgrade_v1, upgrade_v2, upgrade_v3, upgrade_v4, upgrade_v5, upgrade_v6, upgrade_v7,
            upgrade_v8,
        ];
        match value.get("schema_version").and_then(Value::as_u64) {
            Some(version) if (1..=u64::from(SCHEMA_VERSION)).contains(&version) => {
                for upgrade in &UPGRADES[version as usize - 1..] {
                    upgrade(&mut value)?;
                }
            }
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
        self.policy.validate()?;
        let pitch = self.policy.voxel_mm;
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
            if let Some(part) = &keepout.attached_to
                && !part_ids.contains(part)
            {
                return Err(format!(
                    "keepout {} is attached to unknown part {part}",
                    keepout.id
                ));
            }
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
            self.validate_sweep_state(sweep)?;
        }
        if self.fasteners.len() > MAX_FASTENERS {
            return Err(format!("at most {MAX_FASTENERS} fasteners are supported"));
        }
        let mut fastener_ids = BTreeSet::new();
        for fastener in &self.fasteners {
            if !identifier(&fastener.id) || !fastener_ids.insert(&fastener.id) {
                return Err(format!("invalid or duplicate fastener id: {}", fastener.id));
            }
            fastener.validate(&part_ids)?;
            self.validate_release(fastener)?;
        }
        if self.materials.len() > MAX_MATERIALS {
            return Err(format!("at most {MAX_MATERIALS} materials are supported"));
        }
        let mut materials = BTreeMap::new();
        for material in &self.materials {
            if !identifier(&material.id) || materials.insert(&material.id, material).is_some() {
                return Err(format!("invalid or duplicate material id: {}", material.id));
            }
            material.validate()?;
        }
        for part in &self.parts {
            if let Some(material) = &part.material
                && !materials.contains_key(material)
            {
                return Err(format!(
                    "part {} refers to unknown material {material}",
                    part.id
                ));
            }
        }
        if self.snap_fits.len() > MAX_SNAP_FITS {
            return Err(format!("at most {MAX_SNAP_FITS} snap fits are supported"));
        }
        let parts: BTreeMap<&String, &Part> = self.parts.iter().map(|p| (&p.id, p)).collect();
        let mut snap_ids = BTreeSet::new();
        for snap in &self.snap_fits {
            if !identifier(&snap.id) || !snap_ids.insert(&snap.id) {
                return Err(format!("invalid or duplicate snap fit id: {}", snap.id));
            }
            snap.validate(&parts, &materials, &self.assembly.steps)?;
        }
        if self.connectors.len() > MAX_CONNECTORS {
            return Err(format!("at most {MAX_CONNECTORS} connectors are supported"));
        }
        let mut connector_ids = BTreeSet::new();
        for connector in &self.connectors {
            if !identifier(&connector.id) || !connector_ids.insert(&connector.id) {
                return Err(format!(
                    "invalid or duplicate connector id: {}",
                    connector.id
                ));
            }
            connector.validate(&parts, &self.sweeps)?;
        }
        Ok(())
    }

    /// コネクタ開口の寸法の整合。IRの寸法だけから決まるためRust coreで判定する。
    ///
    /// 次のいずれかがあればfailとする。
    /// - 掃引の断面がプラグ断面と一致しない。
    /// - 開口の断面が、掃引の断面を各辺`clearance_mm`だけ広げた範囲を内包しない。
    /// - プラグの後端が、開口の内側の面より奥から動き始めて外側の面の外まで抜けない。
    pub fn evaluate_connectors(&self) -> Result<Vec<Check>, String> {
        self.validate()?;
        let mut checks = Vec::new();
        for connector in &self.connectors {
            let box_of = |shape: &Shape| shape.aabb();
            let opening = self
                .parts
                .iter()
                .find(|p| p.id == connector.part)
                .and_then(|p| p.features.iter().find(|f| f.id == connector.opening))
                .ok_or("validated connector opening lost")?;
            let sweep = self
                .sweeps
                .iter()
                .find(|s| s.id == connector.sweep)
                .ok_or("validated connector sweep lost")?;
            let (hole_min, hole_max) = box_of(&opening.shape);
            let (plug_min, plug_max) =
                box_of(sweep.shape.as_ref().ok_or("validated sweep shape lost")?);
            let axis = sweep.direction.axis().index();
            let plane = sweep.direction.axis().plane();
            let clearance = connector.clearance_mm;
            let mut problems = Vec::new();
            let section = plane.map(|i| plug_max[i] - plug_min[i]);
            if (0..2).any(|k| (section[k] - connector.plug_mm[k]).abs() > CONNECTOR_TOLERANCE_MM) {
                problems.push(format!(
                    "sweep {} section {:.3} x {:.3} mm differs from plug {:.3} x {:.3} mm",
                    sweep.id, section[0], section[1], connector.plug_mm[0], connector.plug_mm[1]
                ));
            }
            for i in plane {
                let low = plug_min[i] - clearance;
                let high = plug_max[i] + clearance;
                if hole_min[i] > low + CONNECTOR_TOLERANCE_MM
                    || hole_max[i] < high - CONNECTOR_TOLERANCE_MM
                {
                    problems.push(format!(
                        "opening {} spans [{:.3}, {:.3}] on {}, plug with clearance needs [{low:.3}, {high:.3}]",
                        opening.id,
                        hole_min[i],
                        hole_max[i],
                        ["x", "y", "z"][i]
                    ));
                }
            }
            let travel = match sweep.distance_mm {
                Distance::Millimetres(value) => value,
                Distance::Keyword(DistanceKeyword::Exit) => f64::INFINITY,
            };
            // プラグの後端が開口の内側の面より奥から動き始め、外側の面の外まで抜けること。
            let passes = if sweep.direction.is_positive() {
                plug_min[axis] <= hole_min[axis] + CONNECTOR_TOLERANCE_MM
                    && plug_min[axis] + travel >= hole_max[axis] - CONNECTOR_TOLERANCE_MM
            } else {
                plug_max[axis] >= hole_max[axis] - CONNECTOR_TOLERANCE_MM
                    && plug_max[axis] - travel <= hole_min[axis] + CONNECTOR_TOLERANCE_MM
            };
            if !passes {
                problems.push(format!(
                    "sweep {} does not travel through opening {} along {}",
                    sweep.id, opening.id, sweep.direction
                ));
            }
            let kind = match connector.source.kind {
                SourceKind::Datasheet => "datasheet",
                SourceKind::Measured => "measured",
                SourceKind::Other => "other",
            };
            checks.push(Check {
                rule: Rule::ConnectorFit,
                status: if problems.is_empty() {
                    Status::Pass
                } else {
                    Status::Fail
                },
                target: connector.id.clone(),
                message: if problems.is_empty() {
                    format!(
                        "plug {:.3} x {:.3} mm passes opening {} with clearance {clearance} mm (source: {kind}, {})",
                        connector.plug_mm[0],
                        connector.plug_mm[1],
                        opening.id,
                        connector.source.reference.trim()
                    )
                } else {
                    format!(
                        "{} (source: {kind}, {})",
                        problems.join("; "),
                        connector.source.reference.trim()
                    )
                },
            });
        }
        Ok(checks)
    }

    /// ネジを外す状態のstepと、締めるkeepoutが存在することを確かめる。
    fn validate_release(&self, fastener: &Fastener) -> Result<(), String> {
        if let Some(step) = fastener
            .release
            .as_ref()
            .and_then(|r| r.after_step.as_ref())
            && !self.assembly.steps.iter().any(|s| &s.id == step)
        {
            return Err(format!(
                "fastener {} is released after unknown step {step}",
                fastener.id
            ));
        }
        let mut seen = BTreeSet::new();
        for keepout in &fastener.clamp_keepouts {
            if !self.keepouts.iter().any(|k| &k.id == keepout) || !seen.insert(keepout) {
                return Err(format!(
                    "fastener {} has an invalid clamp keepout {keepout}",
                    fastener.id
                ));
            }
        }
        Ok(())
    }

    /// 部品を外すstepの番号。どのstepにも現れない部品はNone。
    fn removal_step(&self, part: &str) -> Option<usize> {
        self.assembly
            .steps
            .iter()
            .position(|s| s.parts.iter().any(|p| p == part))
    }

    /// ネジを外す順序の検査。形状を使わないためRust coreで判定する。
    ///
    /// ネジを外すより前に、base、clampの部品、締めているkeepoutのいずれかを他と別々に
    /// 動かすstepがあればfailとする。keepoutは取付先の部品と一緒に動く。締めている
    /// keepoutを、ネジを外す前の状態で取り出す掃引もfailとする。
    pub fn evaluate_fastener_releases(&self) -> Result<Vec<Check>, String> {
        self.validate()?;
        let steps = &self.assembly.steps;
        // 状態の番号。-1は組立完了、iはsteps[i]を終えた状態。
        let state_of = |step: &Option<String>| -> isize {
            step.as_ref()
                .and_then(|id| steps.iter().position(|s| &s.id == id))
                .map_or(-1, |i| i as isize)
        };
        let mut checks = Vec::new();
        for fastener in &self.fasteners {
            // ネジを外した状態。Noneは外さない。
            let released = fastener.release.as_ref().map(|r| state_of(&r.after_step));
            let mut members: Vec<(String, Option<usize>)> = std::iter::once(&fastener.base)
                .chain(&fastener.clamp)
                .map(|p| (p.clone(), self.removal_step(p)))
                .collect();
            for id in &fastener.clamp_keepouts {
                let keepout = self
                    .keepouts
                    .iter()
                    .find(|k| &k.id == id)
                    .ok_or("validated clamp keepout lost")?;
                let removal = keepout
                    .attached_to
                    .as_deref()
                    .and_then(|p| self.removal_step(p));
                members.push((format!("keepout:{id}"), removal));
            }
            let mut problems = Vec::new();
            for (index, step) in steps.iter().enumerate() {
                // steps[index]はstate index-1から始まる。ネジはその状態で外れていれば効かない。
                if released.is_some_and(|r| r < index as isize) {
                    break;
                }
                let present: Vec<&str> = members
                    .iter()
                    .filter(|(_, removal)| removal.is_none_or(|r| r >= index))
                    .map(|(name, _)| name.as_str())
                    .collect();
                let moving: Vec<&str> = members
                    .iter()
                    .filter(|(_, removal)| *removal == Some(index))
                    .map(|(name, _)| name.as_str())
                    .collect();
                if !moving.is_empty() && moving.len() != present.len() {
                    let staying: Vec<&str> = present
                        .iter()
                        .copied()
                        .filter(|name| !moving.contains(name))
                        .collect();
                    problems.push(format!(
                        "step {} moves {} away from {} while the screw holds them",
                        step.id,
                        moving.join(", "),
                        staying.join(", ")
                    ));
                }
            }
            for sweep in &self.sweeps {
                let Some(keepout) = &sweep.keepout else {
                    continue;
                };
                let state = state_of(&sweep.after_step);
                if fastener.clamp_keepouts.contains(keepout) && released.is_none_or(|r| state < r) {
                    problems.push(format!(
                        "sweep {} takes keepout {keepout} out while the screw holds it",
                        sweep.id
                    ));
                }
            }
            let released_text = match &fastener.release {
                None => "is never removed".to_string(),
                Some(Release { after_step: None }) => "is removed in the assembled state".into(),
                Some(Release {
                    after_step: Some(step),
                }) => format!("is removed after step {step}"),
            };
            checks.push(Check {
                rule: Rule::FastenerRelease,
                status: if problems.is_empty() {
                    Status::Pass
                } else {
                    Status::Fail
                },
                target: format!("{}/release", fastener.id),
                message: if problems.is_empty() {
                    format!("screw {released_text}; nothing it holds is separated before that")
                } else {
                    format!("screw {released_text}; {}", problems.join("; "))
                },
            });
        }
        Ok(checks)
    }

    /// 掃引するkeepoutが、評価する状態でまだ存在することを確かめる。
    /// 取付先の部品をafter_stepまでに外すと、keepoutも一緒に取り除かれている。
    fn validate_sweep_state(&self, sweep: &Sweep) -> Result<(), String> {
        let (Some(keepout_id), Some(after_step)) = (&sweep.keepout, &sweep.after_step) else {
            return Ok(());
        };
        let Some(part) = self
            .keepouts
            .iter()
            .find(|k| &k.id == keepout_id)
            .and_then(|k| k.attached_to.as_ref())
        else {
            return Ok(());
        };
        for step in &self.assembly.steps {
            if step.parts.contains(part) {
                return Err(format!(
                    "sweep {}: keepout {keepout_id} leaves with part {part} in step {}, before the state after {after_step}",
                    sweep.id, step.id
                ));
            }
            if &step.id == after_step {
                break;
            }
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

    /// snap fitのうち寸法だけで決まる2項目。形状との照合はbackendが行う。
    ///
    /// strain: 梁の根元のひずみが材料の許容ひずみ以下。
    /// layer: 梁の長さ方向が印刷方向と平行でない。平行だと曲げが積層面を引き離す。
    pub fn evaluate_snap_fits(&self) -> Result<Vec<Check>, String> {
        self.validate()?;
        let mut checks = Vec::new();
        for snap in &self.snap_fits {
            let part = self
                .parts
                .iter()
                .find(|p| p.id == snap.part)
                .ok_or("validated snap fit lost its part")?;
            let geometry = snap
                .geometry(part)
                .ok_or("validated snap fit lost its beam")?;
            let allowable = part
                .material
                .as_ref()
                .and_then(|id| self.materials.iter().find(|m| &m.id == id))
                .and_then(|m| m.allowable_strain)
                .ok_or("validated snap fit lost its material")?;
            let strain = snap.strain(geometry);
            checks.push(Check {
                rule: Rule::SnapFit,
                status: if strain <= allowable { Status::Pass } else { Status::Fail },
                target: format!("{}/strain", snap.id),
                message: format!(
                    "root strain 1.5·t·y/L² = {strain:.6} with t={} mm, y={} mm, L={} mm (root to hook); allowable {allowable}",
                    geometry.thickness_mm, snap.deflection_mm, geometry.arm_mm
                ),
            });
            let build = self.policy.build_direction.axis();
            let along = snap.length_direction.axis();
            checks.push(Check {
                rule: Rule::SnapFit,
                status: if along == build {
                    Status::Fail
                } else {
                    Status::Pass
                },
                target: format!("{}/layer", snap.id),
                message: format!(
                    "beam length along {}; build direction {}",
                    snap.length_direction, self.policy.build_direction
                ),
            });
        }
        Ok(checks)
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
                material: None,
            }],
            keepouts: vec![],
            sweeps: vec![],
            assembly: Assembly::default(),
            fasteners: vec![],
            materials: vec![],
            snap_fits: vec![],
            connectors: vec![],
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
            attached_to: None,
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
            attached_to: None,
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
        for version in ["0", "9", "\"2\""] {
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
            material: None,
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

    fn fastener(id: &str) -> Fastener {
        Fastener {
            id: id.into(),
            base: "base".into(),
            clamp: vec!["lid".into()],
            direction: Direction::MinusZ,
            center: [5., 5.],
            seat_mm: 4.0,
            joint_mm: 2.0,
            screw: Screw {
                length_mm: 6.0,
                major_mm: 2.0,
                head_mm: 3.8,
            },
            through_mm: 2.4,
            anchor: Anchor::SelfTapping { pilot_mm: 1.6 },
            min_engagement_mm: 3.0,
            min_boss_wall_mm: 1.2,
            release: None,
            clamp_keepouts: vec![],
        }
    }

    #[test]
    fn fasteners_round_trip_both_anchors() {
        let mut m = two_parts();
        m.fasteners = vec![
            fastener("a"),
            Fastener {
                anchor: Anchor::Insert {
                    hole_mm: 3.2,
                    length_mm: 4.0,
                },
                ..fastener("b")
            },
        ];
        assert!(m.validate().is_ok());
        let json = serde_json::to_string(&m).unwrap();
        assert!(
            json.contains(r#""anchor":{"kind":"self_tapping","pilot_mm":1.6}"#),
            "{json}"
        );
        assert!(
            json.contains(r#""anchor":{"kind":"insert","hole_mm":3.2,"length_mm":4.0}"#),
            "{json}"
        );
        let back = Model::from_json(&json).unwrap();
        assert_eq!(back.fasteners.len(), 2);
    }

    #[test]
    fn invalid_fasteners_are_rejected() {
        let screw = |length_mm, major_mm, head_mm| Screw {
            length_mm,
            major_mm,
            head_mm,
        };
        let cases: Vec<(&str, Fastener)> = vec![
            ("duplicate id", fastener("a")),
            ("invalid id", fastener("../b")),
            (
                "unknown base",
                Fastener {
                    base: "ghost".into(),
                    ..fastener("b")
                },
            ),
            (
                "unknown clamp",
                Fastener {
                    clamp: vec!["ghost".into()],
                    ..fastener("b")
                },
            ),
            (
                "clamp equals base",
                Fastener {
                    clamp: vec!["base".into()],
                    ..fastener("b")
                },
            ),
            (
                "duplicate clamp",
                Fastener {
                    clamp: vec!["lid".into(), "lid".into()],
                    ..fastener("b")
                },
            ),
            (
                "non-finite seat",
                Fastener {
                    seat_mm: f64::NAN,
                    ..fastener("b")
                },
            ),
            (
                "zero length",
                Fastener {
                    screw: screw(0.0, 2.0, 3.8),
                    ..fastener("b")
                },
            ),
            (
                "through below major",
                Fastener {
                    through_mm: 1.9,
                    ..fastener("b")
                },
            ),
            (
                "head not above through",
                Fastener {
                    screw: screw(6.0, 2.0, 2.4),
                    ..fastener("b")
                },
            ),
            (
                "pilot not below major",
                Fastener {
                    anchor: Anchor::SelfTapping { pilot_mm: 2.0 },
                    ..fastener("b")
                },
            ),
            (
                "insert hole not above major",
                Fastener {
                    anchor: Anchor::Insert {
                        hole_mm: 2.0,
                        length_mm: 4.0,
                    },
                    ..fastener("b")
                },
            ),
            (
                "zero insert length",
                Fastener {
                    anchor: Anchor::Insert {
                        hole_mm: 3.2,
                        length_mm: 0.0,
                    },
                    ..fastener("b")
                },
            ),
            (
                "joint behind the seat",
                Fastener {
                    joint_mm: 5.0,
                    ..fastener("b")
                },
            ),
            (
                "zero clamp thickness",
                Fastener {
                    joint_mm: 4.0,
                    ..fastener("b")
                },
            ),
            (
                "negative wall",
                Fastener {
                    min_boss_wall_mm: -1.0,
                    ..fastener("b")
                },
            ),
        ];
        for (name, bad) in cases {
            let mut m = two_parts();
            m.fasteners = vec![fastener("a"), bad];
            assert!(m.validate().is_err(), "{name}");
        }
    }

    #[test]
    fn fastener_without_clamp_may_seat_on_the_joint() {
        // 基板など部品として記述しない対象を締める場合、頭は境目に当たってよい。
        let mut m = two_parts();
        m.fasteners = vec![Fastener {
            clamp: vec![],
            seat_mm: 2.0,
            ..fastener("a")
        }];
        assert!(m.validate().is_ok());
    }

    #[test]
    fn schema_v4_is_upgraded_with_no_fasteners() {
        let mut value = serde_json::to_value(model()).unwrap();
        let root = value.as_object_mut().unwrap();
        for key in ["fasteners", "materials", "snap_fits", "connectors"] {
            root.remove(key);
        }
        root.insert("schema_version".into(), json!(4));
        let v4 = value.to_string();
        let model = Model::from_json(&v4).unwrap();
        assert_eq!(model.schema_version, SCHEMA_VERSION);
        assert!(model.fasteners.is_empty());
        // v4はfastenersを定義していない。昇格前の入力に現れたら拒否する。
        value
            .as_object_mut()
            .unwrap()
            .insert("fasteners".into(), json!([]));
        assert!(Model::from_json(&value.to_string()).is_err());
    }

    fn add_box(id: &str, min: [f64; 3], max: [f64; 3]) -> Feature {
        Feature {
            id: id.into(),
            role: Role::Generic,
            operation: Operation::Add,
            shape: Shape::Box { min, max },
        }
    }

    /// 蓋の上面から+Zへ立つ梁 (t=1.5 mm)。先端の-X側に1 mm張り出すフックを持つ。
    fn with_snap() -> Model {
        let mut m = two_parts();
        m.materials = vec![Material {
            id: "pla".into(),
            name: "test PLA".into(),
            source: "test fixture".into(),
            allowable_strain: Some(0.04),
        }];
        let lid = &mut m.parts[1];
        lid.material = Some("pla".into());
        lid.features
            .push(add_box("beam", [10., 4., 4.], [11.5, 6., 14.]));
        lid.features
            .push(add_box("hook", [9., 4., 12.], [10., 6., 14.]));
        m.assembly.steps = vec![step(
            "open_lid",
            &["lid"],
            vec![segment(Direction::PlusZ, EXIT)],
        )];
        m.snap_fits = vec![SnapFit {
            id: "clip".into(),
            part: "lid".into(),
            beam: "beam".into(),
            hook: "hook".into(),
            length_direction: Direction::PlusZ,
            deflection: Direction::PlusX,
            deflection_mm: 1.0,
            mate: "base".into(),
            step: "open_lid".into(),
        }];
        m
    }

    #[test]
    fn snap_fit_strain_follows_beam_theory() {
        let mut m = with_snap();
        m.policy.build_direction = Direction::PlusX;
        let checks = m.evaluate_snap_fits().unwrap();
        // L=12-4=8 mm、t=1.5 mm、y=1 mm: ε = 1.5·1.5·1/64。
        let geometry = m.snap_fits[0].geometry(&m.parts[1]).unwrap();
        assert_eq!(
            geometry,
            BeamGeometry {
                arm_mm: 8.0,
                thickness_mm: 1.5,
                width_mm: 2.0,
            }
        );
        assert_eq!(m.snap_fits[0].strain(geometry), 0.03515625);
        let statuses: Vec<(&str, Status)> = checks
            .iter()
            .map(|c| (c.target.as_str(), c.status))
            .collect();
        assert_eq!(
            statuses,
            [("clip/strain", Status::Pass), ("clip/layer", Status::Pass)]
        );
        m.snap_fits[0].deflection_mm = 1.2;
        assert_eq!(m.evaluate_snap_fits().unwrap()[0].status, Status::Fail);
    }

    #[test]
    fn snap_fit_along_the_build_direction_fails_the_layer_check() {
        let checks = with_snap().evaluate_snap_fits().unwrap();
        assert_eq!(checks[1].target, "clip/layer");
        assert_eq!(checks[1].status, Status::Fail);
    }

    #[test]
    fn snap_fit_round_trips_with_materials() {
        let json = serde_json::to_string(&with_snap()).unwrap();
        assert!(json.contains(r#""material":"pla""#), "{json}");
        assert!(json.contains(r#""allowable_strain":0.04"#), "{json}");
        let back = Model::from_json(&json).unwrap();
        assert_eq!(back.snap_fits.len(), 1);
        // materialを持たない部品はfieldを出力しない。
        let base = serde_json::to_value(&back.parts[0]).unwrap();
        assert!(base.get("material").is_none(), "{base}");
    }

    #[test]
    fn invalid_snap_fits_are_rejected() {
        type Edit = fn(&mut Model);
        let cases: Vec<(&str, Edit)> = vec![
            ("no material", |m| m.parts[1].material = None),
            ("material without strain", |m| {
                m.materials[0].allowable_strain = None
            }),
            ("strain out of range", |m| {
                m.materials[0].allowable_strain = Some(1.5)
            }),
            ("unknown material", |m| {
                m.parts[1].material = Some("abs".into())
            }),
            ("duplicate material", |m| {
                m.materials.push(m.materials[0].clone())
            }),
            ("empty material source", |m| {
                m.materials[0].source = " ".into()
            }),
            ("beam is the hook", |m| m.snap_fits[0].hook = "beam".into()),
            ("hook is not a feature", |m| {
                m.snap_fits[0].hook = "ghost".into()
            }),
            ("hook is a cut", |m| {
                m.parts[1].features[2].operation = Operation::Cut
            }),
            ("hook apart from the beam", |m| {
                m.parts[1].features[2] = add_box("hook", [8., 4., 12.], [9., 6., 14.])
            }),
            ("hook at the root", |m| {
                m.parts[1].features[2] = add_box("hook", [9., 4., 4.], [10., 6., 6.])
            }),
            ("hook beyond the tip", |m| {
                m.parts[1].features[2] = add_box("hook", [9., 4., 13.], [10., 6., 15.])
            }),
            ("deflection along the length", |m| {
                m.snap_fits[0].deflection = Direction::MinusZ
            }),
            ("zero deflection", |m| m.snap_fits[0].deflection_mm = 0.0),
            ("unknown mate", |m| m.snap_fits[0].mate = "ghost".into()),
            ("mate is the part", |m| m.snap_fits[0].mate = "lid".into()),
            ("unknown step", |m| m.snap_fits[0].step = "ghost".into()),
            ("step moves both", |m| {
                m.assembly.steps[0].parts.push("base".into())
            }),
            ("duplicate id", |m| m.snap_fits.push(m.snap_fits[0].clone())),
            ("mate removed by an earlier step", |m| {
                m.assembly.steps.insert(
                    0,
                    step(
                        "take_base",
                        &["base"],
                        vec![segment(Direction::MinusZ, EXIT)],
                    ),
                )
            }),
            ("part removed by an earlier step", |m| {
                m.assembly.steps[0].id = "lift_lid".into();
                m.assembly.steps.push(step(
                    "open_lid",
                    &["base"],
                    vec![segment(Direction::MinusZ, EXIT)],
                ))
            }),
        ];
        assert!(with_snap().validate().is_ok());
        for (name, edit) in cases {
            let mut m = with_snap();
            edit(&mut m);
            assert!(m.validate().is_err(), "{name}");
        }
    }

    #[test]
    fn schema_v5_is_upgraded_with_no_materials_or_snap_fits() {
        let mut value = serde_json::to_value(model()).unwrap();
        let root = value.as_object_mut().unwrap();
        root.remove("materials");
        root.remove("snap_fits");
        root.remove("connectors");
        root.insert("schema_version".into(), json!(5));
        let model = Model::from_json(&value.to_string()).unwrap();
        assert_eq!(model.schema_version, SCHEMA_VERSION);
        assert!(model.materials.is_empty() && model.snap_fits.is_empty());
        for (key, entry) in [("materials", json!([])), ("snap_fits", json!([]))] {
            let mut bad = value.clone();
            bad.as_object_mut().unwrap().insert(key.into(), entry);
            assert!(Model::from_json(&bad.to_string()).is_err(), "{key}");
        }
        let mut bad = value.clone();
        bad["parts"][0]["material"] = json!("pla");
        assert!(Model::from_json(&bad.to_string()).is_err());
    }

    #[test]
    fn keepout_attachment_is_validated() {
        let mut m = two_parts();
        m.keepouts = with_keepout().keepouts;
        m.keepouts[0].attached_to = Some("lid".into());
        assert!(m.validate().is_ok());
        let json = serde_json::to_string(&m).unwrap();
        assert!(json.contains(r#""attached_to":"lid""#), "{json}");
        m.keepouts[0].attached_to = Some("ghost".into());
        assert!(m.validate().is_err());
    }

    #[test]
    fn sweep_of_a_keepout_that_left_with_its_part_is_rejected() {
        let mut m = two_parts();
        m.keepouts = with_keepout().keepouts;
        m.keepouts[0].attached_to = Some("lid".into());
        m.assembly.steps = vec![
            step("open_lid", &["lid"], vec![segment(Direction::PlusZ, EXIT)]),
            step(
                "lift_base",
                &["base"],
                vec![segment(Direction::PlusZ, EXIT)],
            ),
        ];
        let after = |step_id: &str| Sweep {
            after_step: Some(step_id.into()),
            ..sweep("pcb_out")
        };
        m.sweeps = vec![after("open_lid")];
        assert!(m.validate().is_err(), "removed in the same step");
        m.sweeps = vec![after("lift_base")];
        assert!(m.validate().is_err(), "removed in an earlier step");
        // 組立完了の状態では、取付先と一緒にまだ残っている。
        m.sweeps = vec![sweep("pcb_out")];
        assert!(m.validate().is_ok());
        // 取付先が残る状態なら、他の部品を外した後でも掃引できる。
        m.keepouts[0].attached_to = Some("base".into());
        m.sweeps = vec![after("open_lid")];
        assert!(m.validate().is_ok());
    }

    #[test]
    fn schema_v6_is_upgraded_with_fixed_keepouts() {
        let mut value = serde_json::to_value(with_keepout()).unwrap();
        value.as_object_mut().unwrap().remove("connectors");
        value["schema_version"] = json!(6);
        let model = Model::from_json(&value.to_string()).unwrap();
        assert_eq!(model.schema_version, SCHEMA_VERSION);
        assert!(model.keepouts[0].attached_to.is_none());
        value["keepouts"][0]["attached_to"] = json!("base");
        assert!(Model::from_json(&value.to_string()).is_err());
    }

    /// 蓋 (lid) を基板 (base) にネジで留め、蓋を上へ外してから基板を下へ抜く。
    fn with_screwed_lid(release: Option<Release>) -> Model {
        let mut m = two_parts();
        m.keepouts = with_keepout().keepouts;
        m.assembly.steps = vec![
            step("open_lid", &["lid"], vec![segment(Direction::PlusZ, EXIT)]),
            step(
                "drop_base",
                &["base"],
                vec![segment(Direction::MinusZ, EXIT)],
            ),
        ];
        m.fasteners = vec![Fastener {
            release,
            ..fastener("corner")
        }];
        m
    }

    fn release_after(step: Option<&str>) -> Option<Release> {
        Some(Release {
            after_step: step.map(String::from),
        })
    }

    fn release_check(m: &Model) -> Check {
        let mut checks = m.evaluate_fastener_releases().unwrap();
        assert_eq!(checks.len(), 1);
        checks.remove(0)
    }

    #[test]
    fn screw_must_be_removed_before_its_parts_separate() {
        // 組立完了の状態で外せば、蓋を外すstepは妨げない。
        let passed = release_check(&with_screwed_lid(release_after(None)));
        assert_eq!(passed.status, Status::Pass, "{}", passed.message);
        // 外さないネジ、蓋を外した後で外すネジは、蓋を外すstepで効いている。
        for release in [None, release_after(Some("open_lid"))] {
            let failed = release_check(&with_screwed_lid(release));
            assert_eq!(failed.status, Status::Fail);
            assert!(
                failed
                    .message
                    .contains("step open_lid moves lid away from base"),
                "{}",
                failed.message
            );
        }
    }

    #[test]
    fn parts_removed_together_stay_held() {
        let mut m = with_screwed_lid(None);
        m.assembly.steps = vec![step(
            "lift_all",
            &["lid", "base"],
            vec![segment(Direction::PlusZ, EXIT)],
        )];
        assert_eq!(release_check(&m).status, Status::Pass);
    }

    #[test]
    fn clamped_keepout_needs_the_screw_removed_before_it_leaves() {
        // 基板 (keepout pcb) を蓋ではなくbaseへ締め、蓋を外した後に上へ抜く。
        let mut m = with_screwed_lid(release_after(None));
        m.fasteners[0].clamp = vec![];
        m.fasteners[0].seat_mm = 2.0;
        m.fasteners[0].clamp_keepouts = vec!["pcb".into()];
        m.sweeps = vec![Sweep {
            after_step: Some("open_lid".into()),
            ..sweep("pcb_out")
        }];
        m.fasteners[0].release = release_after(Some("open_lid"));
        assert_eq!(release_check(&m).status, Status::Pass);
        m.fasteners[0].release = None;
        let failed = release_check(&m);
        assert_eq!(failed.status, Status::Fail);
        assert!(
            failed.message.contains("sweep pcb_out"),
            "{}",
            failed.message
        );
        // keepoutが固定されたままbaseを下へ抜くstepも、ネジが効いていればfail。
        m.sweeps.clear();
        let failed = release_check(&m);
        assert!(
            failed
                .message
                .contains("step drop_base moves base away from keepout:pcb"),
            "{}",
            failed.message
        );
        // keepoutがbaseに取り付けられていれば、一緒に動くため引き離されない。
        m.keepouts[0].attached_to = Some("base".into());
        assert_eq!(release_check(&m).status, Status::Pass);
    }

    #[test]
    fn release_references_are_validated() {
        let mut m = with_screwed_lid(release_after(Some("ghost")));
        assert!(m.validate().is_err());
        m.fasteners[0].release = None;
        m.fasteners[0].clamp_keepouts = vec!["ghost".into()];
        assert!(m.validate().is_err());
        m.fasteners[0].clamp_keepouts = vec!["pcb".into(), "pcb".into()];
        assert!(m.validate().is_err());
    }

    #[test]
    fn release_serializes_after_step_only_when_given() {
        let mut m = with_screwed_lid(release_after(None));
        m.fasteners[0].clamp_keepouts = vec!["pcb".into()];
        let json = serde_json::to_string(&m).unwrap();
        assert!(json.contains(r#""release":{}"#), "{json}");
        assert!(json.contains(r#""clamp_keepouts":["pcb"]"#), "{json}");
        let back = Model::from_json(&json).unwrap();
        assert_eq!(back.fasteners[0].release, Some(Release::default()));
    }

    #[test]
    fn schema_v7_is_upgraded_with_screws_that_stay() {
        let mut value = serde_json::to_value(with_screwed_lid(None)).unwrap();
        value.as_object_mut().unwrap().remove("connectors");
        value["schema_version"] = json!(7);
        let model = Model::from_json(&value.to_string()).unwrap();
        assert_eq!(model.schema_version, SCHEMA_VERSION);
        assert!(model.fasteners[0].release.is_none());
        value["fasteners"][0]["release"] = json!({});
        assert!(Model::from_json(&value.to_string()).is_err());
    }

    /// x=0..2の壁にUSB程度の開口を持つ筐体。プラグは内側から-xへ抜く。
    fn with_connector() -> Model {
        let mut m = model();
        m.parts[0].features = vec![
            Feature {
                id: "wall".into(),
                role: Role::Wall,
                operation: Operation::Add,
                shape: Shape::Box {
                    min: [0., 0., 0.],
                    max: [2., 20., 10.],
                },
            },
            Feature {
                id: "port".into(),
                role: Role::Generic,
                operation: Operation::Cut,
                shape: Shape::Box {
                    min: [-1., 5., 3.],
                    max: [3., 15., 7.],
                },
            },
        ];
        m.sweeps = vec![Sweep {
            id: "plug".into(),
            shape: Some(Shape::Box {
                min: [1., 6., 4.],
                max: [10., 14., 6.],
            }),
            keepout: None,
            direction: Direction::MinusX,
            distance_mm: Distance::Keyword(DistanceKeyword::Exit),
            after_step: None,
        }];
        m.connectors = vec![Connector {
            id: "usb".into(),
            part: "base".into(),
            opening: "port".into(),
            sweep: "plug".into(),
            plug_mm: [8., 2.],
            clearance_mm: 0.5,
            source: PlugSource {
                kind: SourceKind::Measured,
                reference: "cable A, caliper".into(),
            },
        }];
        m
    }

    fn connector_check(m: &Model) -> Check {
        let mut checks = m.evaluate_connectors().unwrap();
        assert_eq!(checks.len(), 1);
        checks.remove(0)
    }

    #[test]
    fn connector_opening_with_clearance_passes() {
        let check = connector_check(&with_connector());
        assert_eq!(check.rule, Rule::ConnectorFit);
        assert_eq!(check.target, "usb");
        assert_eq!(check.status, Status::Pass, "{}", check.message);
        assert!(
            check.message.contains("measured, cable A, caliper"),
            "{}",
            check.message
        );
    }

    #[test]
    fn connector_fit_detects_each_inconsistency() {
        type Edit = fn(&mut Model);
        let cases: [(&str, Edit); 4] = [
            ("differs from plug", |m| m.connectors[0].plug_mm = [8., 2.5]),
            ("spans [3.000, 7.000] on z", |m| {
                m.connectors[0].clearance_mm = 1.5
            }),
            ("does not travel through", |m| {
                m.sweeps[0].direction = Direction::PlusX
            }),
            ("does not travel through", |m| {
                m.sweeps[0].distance_mm = Distance::Millimetres(2.0)
            }),
        ];
        for (expected, mutate) in cases {
            let mut m = with_connector();
            mutate(&mut m);
            let check = connector_check(&m);
            assert_eq!(check.status, Status::Fail, "{expected}");
            assert!(check.message.contains(expected), "{}", check.message);
        }
    }

    #[test]
    fn exact_travel_through_the_opening_passes() {
        let mut m = with_connector();
        // プラグの後端x=10から開口の外端x=-1まで、ちょうど11 mm動かす。
        m.sweeps[0].distance_mm = Distance::Millimetres(2.0 + 9.0);
        assert_eq!(connector_check(&m).status, Status::Pass);
    }

    #[test]
    fn invalid_connectors_are_rejected() {
        type Edit = fn(&mut Model);
        let cases: [(&str, Edit); 10] = [
            ("unknown part", |m| m.connectors[0].part = "ghost".into()),
            ("unknown feature", |m| {
                m.connectors[0].opening = "ghost".into()
            }),
            ("opening is additive", |m| {
                m.connectors[0].opening = "wall".into()
            }),
            ("opening is a cylinder", |m| {
                m.parts[0].features[1].shape = Shape::Cylinder {
                    axis: Axis::X,
                    center: [10., 5.],
                    radius: 2.,
                    span: [-1., 3.],
                }
            }),
            ("unknown sweep", |m| m.connectors[0].sweep = "ghost".into()),
            ("sweep uses a keepout", |m| {
                m.keepouts.push(Keepout {
                    id: "pcb".into(),
                    shape: Shape::Box {
                        min: [3., 3., 3.],
                        max: [9., 9., 5.],
                    },
                    clearance_mm: Clearance {
                        default: 0.,
                        faces: BTreeMap::new(),
                    },
                    attached_to: None,
                });
                m.sweeps[0].shape = None;
                m.sweeps[0].keepout = Some("pcb".into());
            }),
            ("zero plug", |m| m.connectors[0].plug_mm = [0., 2.]),
            ("negative clearance", |m| {
                m.connectors[0].clearance_mm = -0.1
            }),
            ("blank source", |m| {
                m.connectors[0].source.reference = "  ".into()
            }),
            ("duplicate id", |m| {
                m.connectors.push(m.connectors[0].clone())
            }),
        ];
        for (name, mutate) in cases {
            let mut m = with_connector();
            mutate(&mut m);
            assert!(m.validate().is_err(), "{name}");
        }
    }

    #[test]
    fn connector_round_trips_through_json() {
        let json = serde_json::to_string(&with_connector()).unwrap();
        assert!(json.contains(r#""kind":"measured""#), "{json}");
        let back = Model::from_json(&json).unwrap();
        assert_eq!(back.connectors.len(), 1);
        assert_eq!(back.connectors[0].plug_mm, [8., 2.]);
    }

    #[test]
    fn schema_v8_is_upgraded_with_no_connectors() {
        let mut value = serde_json::to_value(with_connector()).unwrap();
        value["schema_version"] = json!(8);
        assert!(Model::from_json(&value.to_string()).is_err());
        value.as_object_mut().unwrap().remove("connectors");
        let model = Model::from_json(&value.to_string()).unwrap();
        assert_eq!(model.schema_version, SCHEMA_VERSION);
        assert!(model.connectors.is_empty());
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
