//! 部品ごとの線形弾性と定常熱伝導の解析。
//!
//! 部品をvoxel格子にし、材料のcellを8節点の六面体要素として解く。荷重、拘束、発熱、放熱を
//! 掛ける領域は、軸平行のboxか、ネジ固定、keepout、コネクタ開口への参照で指定し、部品の
//! 表面の面へ解決する。Boolean後の面番号は使わない。
//!
//! 応力は要素の8隅で求める。平らな面では表面の応力に当たる。曲面を近似する格子の段の内角は、
//! 格子と相似な応力集中を持ち、細分しても下がらない過大な値を与える。形状の鋭い内角と拘束域の
//! 縁は特異点であり、細分するほど値が増える。格子間隔h、2h、4hで解き、判定に使う量の差が
//! 細分とともに縮まなければ評価しない。収束した量は残りの差を見積もって安全側に外挿し、上限と
//! 比べる。拘束と荷重の近傍の応力は判定せず、未評価として残す。
//!
//! 強度のケースでは、薄い部材と丸みの周りのcellを最大3段まで8分割する (`Refinement`)。3つの
//! 格子は同じ段を基準の間隔h、2h、4hに適用したもので、各所の一辺はs、2s、4sとなる。

use crate::multigrid::{CORNERS, Geometry, Hierarchy, Level};
use crate::octree::{Cell, Constraints, Locator, Tree, constraints, corner};
use crate::voxel::Grid;
use crate::{
    Anchor, Check, Location, MAX_LOCATIONS, Material, Model, Operation, Part, Rule, Shape, Status,
    identifier,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::ops::Range;

/// 1つのmodelが持てる解析ケースの上限。種類ごとである。
const MAX_CASES: usize = 100;
/// 1つの解析ケースが持てる領域の上限。
const MAX_REGIONS: usize = 100;

/// 荷重、拘束、発熱、放熱を掛ける部品表面の領域。格子の露出した面のうち、面の中心が
/// 領域に入るものを選ぶ。参照による領域は、格子の段を含めるため`2·voxel_mm`だけ広げる。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Region {
    /// 部品の表面すべて。
    Surface,
    /// 軸平行のbox内の表面。単位はmm。
    Box { min: [f64; 3], max: [f64; 3] },
    /// ネジ固定。baseの部品では下穴の壁 (下穴の径の円筒、境目から先端まで)、clampの部品では
    /// 頭の座面 (頭の外径の円板)。
    Fastener { id: String },
    /// keepoutの箱 (clearanceを含めない) に接する表面。
    Keepout { id: String },
    /// コネクタ開口の縁の表面。
    Connector { id: String },
}

/// 領域へ掛ける力。合計の力を領域の表面へ一様に分布させる。単位はN。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Force {
    pub region: Region,
    pub newtons: [f64; 3],
}

/// 部品1つの線形弾性の荷重ケース。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StructuralCase {
    pub id: String,
    pub part: String,
    /// 変位を0に拘束する領域。
    pub supports: Vec<Region>,
    pub forces: Vec<Force>,
    /// 許容応力を割る安全率。1以上。
    pub safety_factor: f64,
    /// 許す最大の変位。省略すると変位を判定しない。単位はmm。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_deflection_mm: Option<f64>,
    /// 解析の格子間隔。省略するとpolicyの`voxel_mm`。2倍と4倍の格子でも解く。単位はmm。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voxel_mm: Option<f64>,
}

/// 領域へ入る熱量。領域の表面へ一様に分布させる。単位はW。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeatInput {
    pub region: Region,
    pub watts: f64,
}

/// 領域の温度を固定する。単位は°C。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixedTemperature {
    pub region: Region,
    pub celsius: f64,
}

/// 領域の表面から周囲の空気への対流。熱伝達係数はW/(m²·K)、周囲温度は°C。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Convection {
    pub region: Region,
    pub h_w_per_m2k: f64,
    pub ambient_c: f64,
}

/// 部品1つの定常熱伝導のケース。筐体内の空気は、利用者が与える周囲温度として扱う。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThermalCase {
    pub id: String,
    pub part: String,
    #[serde(default)]
    pub heat_inputs: Vec<HeatInput>,
    #[serde(default)]
    pub fixed_temperatures: Vec<FixedTemperature>,
    #[serde(default)]
    pub convection: Vec<Convection>,
    /// 部品のどこでも超えてはならない温度。単位は°C。
    pub max_temperature_c: f64,
    /// 解析の格子間隔。省略するとpolicyの`voxel_mm`。2倍と4倍の格子でも解く。単位はmm。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voxel_mm: Option<f64>,
}

/// modelが持つ解析ケース。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Analyses {
    #[serde(default)]
    pub structural: Vec<StructuralCase>,
    #[serde(default)]
    pub thermal: Vec<ThermalCase>,
}

impl Analyses {
    pub fn is_empty(&self) -> bool {
        self.structural.is_empty() && self.thermal.is_empty()
    }

    /// 参照先、値域、材料定数の有無を確かめる。
    pub(crate) fn validate(&self, model: &Model) -> Result<(), String> {
        if self.structural.len() > MAX_CASES || self.thermal.len() > MAX_CASES {
            return Err(format!(
                "at most {MAX_CASES} structural and {MAX_CASES} thermal cases are supported"
            ));
        }
        let mut ids = BTreeSet::new();
        for case in &self.structural {
            let owner = format!("structural case {}", case.id);
            if !identifier(&case.id) || !ids.insert(&case.id) {
                return Err(format!(
                    "invalid or duplicate analysis case id: {}",
                    case.id
                ));
            }
            validate_pitch(case.voxel_mm, &owner)?;
            let part = case_part(model, &case.part, &owner)?;
            let regions: Vec<&Region> = case
                .supports
                .iter()
                .chain(case.forces.iter().map(|f| &f.region))
                .collect();
            validate_regions(model, part, &regions, &owner)?;
            if case.supports.is_empty() || case.forces.is_empty() {
                return Err(format!(
                    "{owner}: at least one support and one force are required"
                ));
            }
            for force in &case.forces {
                if force
                    .newtons
                    .iter()
                    .any(|v| !v.is_finite() || v.abs() > 1e7)
                    || force.newtons.iter().all(|v| *v == 0.0)
                {
                    return Err(format!(
                        "{owner}: force must be finite, non-zero and within 1e7 N"
                    ));
                }
            }
            if !case.safety_factor.is_finite() || !(1.0..=100.0).contains(&case.safety_factor) {
                return Err(format!("{owner}: safety_factor must be in [1, 100]"));
            }
            if let Some(limit) = case.max_deflection_mm
                && (!limit.is_finite() || limit <= 0.0 || limit > 1e3)
            {
                return Err(format!("{owner}: max_deflection_mm must be in (0, 1000]"));
            }
            let material = case_material(model, part, &owner)?;
            if material.elastic.is_none() || material.allowable_stress_mpa.is_none() {
                return Err(format!(
                    "{owner}: material {} needs elastic and allowable_stress_mpa",
                    material.id
                ));
            }
            let layered = part
                .adopted_plan()
                .is_some_and(|plan| plan.process.layering().is_some());
            if layered && material.interlayer_allowable_stress_mpa.is_none() {
                return Err(format!(
                    "{owner}: material {} needs interlayer_allowable_stress_mpa because part {} is built in layers",
                    material.id, part.id
                ));
            }
        }
        for case in &self.thermal {
            let owner = format!("thermal case {}", case.id);
            if !identifier(&case.id) || !ids.insert(&case.id) {
                return Err(format!(
                    "invalid or duplicate analysis case id: {}",
                    case.id
                ));
            }
            validate_pitch(case.voxel_mm, &owner)?;
            let part = case_part(model, &case.part, &owner)?;
            let regions: Vec<&Region> = case
                .heat_inputs
                .iter()
                .map(|h| &h.region)
                .chain(case.fixed_temperatures.iter().map(|t| &t.region))
                .chain(case.convection.iter().map(|c| &c.region))
                .collect();
            validate_regions(model, part, &regions, &owner)?;
            // 熱の出口が無ければ定常解が定まらない。
            if case.convection.is_empty() && case.fixed_temperatures.is_empty() {
                return Err(format!(
                    "{owner}: at least one convection or fixed temperature is required"
                ));
            }
            for input in &case.heat_inputs {
                if !input.watts.is_finite() || !(0.0..=1e4).contains(&input.watts) {
                    return Err(format!("{owner}: watts must be in [0, 10000]"));
                }
            }
            let temperature = |value: f64| value.is_finite() && (-273.15..=2000.0).contains(&value);
            for fixed in &case.fixed_temperatures {
                if !temperature(fixed.celsius) {
                    return Err(format!("{owner}: celsius must be in [-273.15, 2000]"));
                }
            }
            for convection in &case.convection {
                if !convection.h_w_per_m2k.is_finite()
                    || !(0.0..=1e6).contains(&convection.h_w_per_m2k)
                    || convection.h_w_per_m2k == 0.0
                    || !temperature(convection.ambient_c)
                {
                    return Err(format!(
                        "{owner}: convection needs h_w_per_m2k in (0, 1e6] and ambient_c in [-273.15, 2000]"
                    ));
                }
            }
            if !temperature(case.max_temperature_c) {
                return Err(format!(
                    "{owner}: max_temperature_c must be in [-273.15, 2000]"
                ));
            }
            let material = case_material(model, part, &owner)?;
            if material.thermal_conductivity_w_per_mk.is_none() {
                return Err(format!(
                    "{owner}: material {} needs thermal_conductivity_w_per_mk",
                    material.id
                ));
            }
        }
        Ok(())
    }
}

fn validate_pitch(voxel_mm: Option<f64>, owner: &str) -> Result<(), String> {
    match voxel_mm {
        Some(pitch) if !pitch.is_finite() || !(0.01..=10.0).contains(&pitch) => Err(format!(
            "{owner}: voxel_mm must be finite and in [0.01, 10]"
        )),
        _ => Ok(()),
    }
}

fn case_part<'a>(model: &'a Model, id: &str, owner: &str) -> Result<&'a Part, String> {
    model
        .parts
        .iter()
        .find(|p| p.id == id)
        .ok_or_else(|| format!("{owner}: unknown part {id}"))
}

/// 部品の採用した製造案の材料。製造案が材料を持たなければ部品の材料。
fn case_material<'a>(model: &'a Model, part: &Part, owner: &str) -> Result<&'a Material, String> {
    let id = part
        .material_id()
        .ok_or_else(|| format!("{owner}: part {} has no material", part.id))?;
    model
        .materials
        .iter()
        .find(|m| &m.id == id)
        .ok_or_else(|| format!("{owner}: unknown material {id}"))
}

fn validate_regions(
    model: &Model,
    part: &Part,
    regions: &[&Region],
    owner: &str,
) -> Result<(), String> {
    if regions.len() > MAX_REGIONS {
        return Err(format!(
            "{owner}: at most {MAX_REGIONS} regions are supported"
        ));
    }
    for region in regions {
        match region {
            Region::Surface => {}
            Region::Box { min, max } => {
                if (0..3).any(|axis| {
                    !min[axis].is_finite() || !max[axis].is_finite() || min[axis] >= max[axis]
                }) {
                    return Err(format!("{owner}: region box needs finite min < max"));
                }
            }
            Region::Fastener { id } => {
                let fastener = model
                    .fasteners
                    .iter()
                    .find(|f| &f.id == id)
                    .ok_or_else(|| format!("{owner}: unknown fastener {id}"))?;
                if fastener.base != part.id && !fastener.clamp.contains(&part.id) {
                    return Err(format!(
                        "{owner}: fastener {id} does not hold part {}",
                        part.id
                    ));
                }
            }
            Region::Keepout { id } => {
                if !model.keepouts.iter().any(|k| &k.id == id) {
                    return Err(format!("{owner}: unknown keepout {id}"));
                }
            }
            Region::Connector { id } => {
                let connector = model
                    .connectors
                    .iter()
                    .find(|c| &c.id == id)
                    .ok_or_else(|| format!("{owner}: unknown connector {id}"))?;
                if connector.part != part.id {
                    return Err(format!(
                        "{owner}: connector {id} is not on part {}",
                        part.id
                    ));
                }
            }
        }
    }
    Ok(())
}

/// 格子間隔hと2hの解の相対差がこれを超えれば、収束していないとして評価しない。
const CONVERGENCE_TOLERANCE: f64 = 0.2;
/// 相対差の分母の下限。上限値に対する比。上限よりずっと小さい値どうしの差は、
/// 丸め誤差であっても相対差が大きくなるため、この値を下限とする。
const CONVERGENCE_FLOOR: f64 = 1e-3;
/// これ以下の相対差は、細分で縮まなくても収束したものとみなす。丸め誤差と、格子による
/// 形状の小さな違いを許す。
const NEGLIGIBLE_CHANGE: f64 = 0.01;
/// hと2hの差の、2hと4hの差に対する比の上限。収束の次数0.5 (比2^-0.5) に当たる。鋭い内角の
/// 応力は、細分を進めると比が1を超えるが、粗い格子では1に近い値で縮んで見える。
const MAX_SHRINK_RATIO: f64 = 0.7;

/// 一辺1の立方体の要素で、点`xi` (0〜1) での形状関数の勾配。
fn shape_gradients(xi: [f64; 3]) -> [[f64; 3]; 8] {
    std::array::from_fn(|a| {
        let corner = CORNERS[a];
        let factor = |axis: usize| {
            if corner[axis] == 1 {
                xi[axis]
            } else {
                1.0 - xi[axis]
            }
        };
        let slope = |axis: usize| if corner[axis] == 1 { 1.0 } else { -1.0 };
        [
            slope(0) * factor(1) * factor(2),
            factor(0) * slope(1) * factor(2),
            factor(0) * factor(1) * slope(2),
        ]
    })
}

/// 2×2×2のGauss点。重みはいずれも1/8である。
fn gauss_points() -> [[f64; 3]; 8] {
    let a = 0.5 - 0.5 / 3f64.sqrt();
    let b = 0.5 + 0.5 / 3f64.sqrt();
    std::array::from_fn(|k| {
        [
            if k & 1 == 0 { a } else { b },
            if k & 2 == 0 { a } else { b },
            if k & 4 == 0 { a } else { b },
        ]
    })
}

/// ひずみ (Voigt表記: xx, yy, zz, yz, xz, xy) を節点変位から求める行列。一辺1の要素。
fn strain_matrix(xi: [f64; 3]) -> [[f64; 24]; 6] {
    let gradients = shape_gradients(xi);
    let mut b = [[0.0; 24]; 6];
    for (a, g) in gradients.iter().enumerate() {
        let column = 3 * a;
        b[0][column] = g[0];
        b[1][column + 1] = g[1];
        b[2][column + 2] = g[2];
        b[3][column + 1] = g[2];
        b[3][column + 2] = g[1];
        b[4][column] = g[2];
        b[4][column + 2] = g[0];
        b[5][column] = g[1];
        b[5][column + 1] = g[0];
    }
    b
}

/// ヤング率1の等方性の弾性行列。
fn elasticity(poisson: f64) -> [[f64; 6]; 6] {
    let factor = 1.0 / ((1.0 + poisson) * (1.0 - 2.0 * poisson));
    std::array::from_fn(|i| {
        std::array::from_fn(|j| match (i < 3, j < 3) {
            (true, true) if i == j => factor * (1.0 - poisson),
            (true, true) => factor * poisson,
            (false, false) if i == j => factor * (1.0 - 2.0 * poisson) / 2.0,
            _ => 0.0,
        })
    })
}

/// 非適合モードのひずみの行列。軸kの気泡関数4t(1-t)を変位の3成分に持たせた9自由度で、
/// 列は3k + 成分の順。一辺1の要素。
fn bubble_matrix(xi: [f64; 3]) -> [[f64; 9]; 6] {
    let mut g = [[0.0; 9]; 6];
    for axis in 0..3 {
        let slope = 4.0 * (1.0 - 2.0 * xi[axis]);
        let column = 3 * axis;
        let mut gradient = [0.0; 3];
        gradient[axis] = slope;
        g[0][column] = gradient[0];
        g[1][column + 1] = gradient[1];
        g[2][column + 2] = gradient[2];
        g[3][column + 1] = gradient[2];
        g[3][column + 2] = gradient[1];
        g[4][column] = gradient[2];
        g[4][column + 2] = gradient[0];
        g[5][column] = gradient[1];
        g[5][column + 1] = gradient[0];
    }
    g
}

/// ヤング率1、一辺1の要素。一辺L、ヤング率Eの要素では剛性がE·L倍となり、`internal`は変わらない。
struct UnitElement {
    /// 非適合モードを静的縮約した剛性行列。
    stiffness: Vec<[f64; 24]>,
    /// 節点変位から非適合モードの振幅を求める行列。
    internal: [[f64; 24]; 9],
}

/// `A^T D C`の2×2×2 Gauss積分。
fn integrate<const M: usize, const N: usize>(
    d: &[[f64; 6]; 6],
    left: impl Fn([f64; 3]) -> [[f64; M]; 6],
    right: impl Fn([f64; 3]) -> [[f64; N]; 6],
) -> Vec<[f64; N]> {
    let mut k = vec![[0.0; N]; M];
    for point in gauss_points() {
        let a = left(point);
        let c = right(point);
        let dc: [[f64; N]; 6] = std::array::from_fn(|i| {
            std::array::from_fn(|j| (0..6).map(|m| d[i][m] * c[m][j]).sum())
        });
        for (i, row) in k.iter_mut().enumerate() {
            for (j, value) in row.iter_mut().enumerate() {
                *value += (0..6).map(|m| a[m][i] * dc[m][j]).sum::<f64>() / 8.0;
            }
        }
    }
    k
}

/// 対称正定値の小さな行列の逆行列。Gauss-Jordan法。
fn invert<const N: usize>(matrix: &[[f64; N]]) -> [[f64; N]; N] {
    let mut a: [[f64; N]; N] = std::array::from_fn(|i| matrix[i]);
    let mut inverse: [[f64; N]; N] =
        std::array::from_fn(|i| std::array::from_fn(|j| if i == j { 1.0 } else { 0.0 }));
    for column in 0..N {
        let pivot = a[column][column];
        for j in 0..N {
            a[column][j] /= pivot;
            inverse[column][j] /= pivot;
        }
        for row in 0..N {
            if row != column {
                let factor = a[row][column];
                for j in 0..N {
                    a[row][j] -= factor * a[column][j];
                    inverse[row][j] -= factor * inverse[column][j];
                }
            }
        }
    }
    inverse
}

/// 非適合モード (Wilsonの要素) を加えた8節点六面体要素。完全積分の8節点要素は曲げで
/// 寄生的なせん断ひずみを生じて硬くなり、薄い壁を数要素で表す格子では変位を過小に見積もる。
/// 非適合モードは要素内の2次の変形を表し、立方体の要素では純曲げを厳密に表す。要素内で
/// 静的縮約するため、連立方程式の大きさは変わらない。
fn unit_element(poisson: f64) -> UnitElement {
    let d = elasticity(poisson);
    let k_uu = integrate(&d, strain_matrix, strain_matrix);
    let k_ua = integrate(&d, strain_matrix, bubble_matrix);
    let k_aa = integrate(&d, bubble_matrix, bubble_matrix);
    let inverse = invert::<9>(&k_aa);
    let internal: [[f64; 24]; 9] = std::array::from_fn(|i| {
        std::array::from_fn(|j| -(0..9).map(|m| inverse[i][m] * k_ua[j][m]).sum::<f64>())
    });
    let mut stiffness = k_uu;
    for (i, row) in stiffness.iter_mut().enumerate() {
        for (j, value) in row.iter_mut().enumerate() {
            *value += (0..9).map(|m| k_ua[i][m] * internal[m][j]).sum::<f64>();
        }
    }
    UnitElement {
        stiffness,
        internal,
    }
}

/// 熱伝導率1、一辺1の要素の伝導行列。一辺L、熱伝導率kの要素ではk·L倍となる。
fn unit_conduction() -> [[f64; 8]; 8] {
    let mut k = [[0.0; 8]; 8];
    for point in gauss_points() {
        let g = shape_gradients(point);
        for (row, gi) in k.iter_mut().zip(&g) {
            for (value, gj) in row.iter_mut().zip(&g) {
                *value += (0..3).map(|axis| gi[axis] * gj[axis]).sum::<f64>() / 8.0;
            }
        }
    }
    k
}

/// 局所的に細かくする段の上限。最も細かいcellの一辺は格子間隔の1/8。
const MAX_REFINEMENT: u32 = 3;
/// 最も粗い格子 (各所の一辺の4倍) で、部材の厚さが持つcellの数。
const CELLS_ACROSS: f64 = 3.0;
/// 最も粗い格子で、丸みの半径が持つcellの数。
const CELLS_PER_RADIUS: f64 = 1.5;
/// 1つの格子の要素の上限。弾性では約450万自由度に当たる。
const MAX_ELEMENTS: usize = 1_500_000;
/// 分割の途中で数える葉 (空のcellを含む) の上限。
const MAX_LEAVES: usize = 8 * MAX_ELEMENTS;

/// 部品が点を含むか。追加の形状のいずれかに含まれ、除去の形状のいずれにも含まれない点で、
/// 格子化 (`Grid::fill`) と同じ判定である。
fn part_contains(part: &Part, point: [f64; 3]) -> bool {
    let inside = |operation: Operation| {
        part.features
            .iter()
            .filter(|f| f.operation == operation)
            .any(|f| f.shape.contains(point))
    };
    inside(Operation::Add) && !inside(Operation::Cut)
}

/// 局所的に細かくする段。部品を格子間隔の半分の格子で測り、cellごとに段kを持つ。段kの所では
/// 要素の一辺sを格子間隔hの1/2^kとする。
///
/// 最も粗い格子 (4s) で、軸方向の厚さtが`CELLS_ACROSS`個、丸みの半径rが`CELLS_PER_RADIUS`個の
/// cellを持つよう、s ≤ t/12、s ≤ r/6とする。tは材料のcellを通る3軸の材料の区間の長さの最小で
/// ある。丸みは、その形状の外接boxを半径だけ広げた範囲の材料に適用する。
struct Refinement {
    /// 厚さを測った格子。
    grid: Grid,
    levels: Vec<u8>,
}

impl Refinement {
    /// 細かくする所が無ければNone。
    fn new(part: &Part, pitch: f64) -> Result<Option<Self>, Unsolved> {
        let grid = Grid::rasterize_midpoint(part, pitch / 2.0).map_err(|reason| {
            format!(
                "{reason} when measuring thickness at grid {} mm",
                pitch / 2.0
            )
        })?;
        let level = |target: f64| -> u8 {
            (0..=MAX_REFINEMENT)
                .find(|k| pitch / f64::from(1u32 << k) <= target * (1.0 + 1e-9))
                .unwrap_or(MAX_REFINEMENT) as u8
        };
        let [nx, ny, _] = grid.size;
        let count = grid.occupied.len();
        let mut runs = vec![u32::MAX; count];
        for axis in 0..3 {
            let stride = [1, nx, nx * ny][axis];
            let length = grid.size[axis];
            for start in (0..count).filter(|i| (i / stride) % length == 0) {
                let mut k = 0;
                while k < length {
                    if !grid.occupied[start + k * stride] {
                        k += 1;
                        continue;
                    }
                    let first = k;
                    while k < length && grid.occupied[start + k * stride] {
                        k += 1;
                    }
                    let run = (k - first) as u32;
                    for j in first..k {
                        let slot = &mut runs[start + j * stride];
                        *slot = (*slot).min(run);
                    }
                }
            }
        }
        let mut levels: Vec<u8> = runs
            .iter()
            .zip(&grid.occupied)
            .map(|(run, solid)| {
                if *solid {
                    level(f64::from(*run) * grid.pitch / (4.0 * CELLS_ACROSS))
                } else {
                    0
                }
            })
            .collect();
        for feature in &part.features {
            let Shape::Fillet { radius, .. } = feature.shape else {
                continue;
            };
            let k = level(radius / (4.0 * CELLS_PER_RADIUS));
            let (low, high) = feature.shape.aabb();
            let ranges = cell_ranges(&grid, low.map(|v| v - radius), high.map(|v| v + radius));
            for z in ranges[2].clone() {
                for y in ranges[1].clone() {
                    for x in ranges[0].clone() {
                        let index = grid.index(x, y, z);
                        if grid.occupied[index] {
                            levels[index] = levels[index].max(k);
                        }
                    }
                }
            }
        }
        if levels.iter().all(|level| *level == 0) {
            return Ok(None);
        }
        Ok(Some(Self { grid, levels }))
    }

    /// box (mm) に重なる、厚さを測ったcellの段の最大。
    fn depth(&self, low: [f64; 3], high: [f64; 3]) -> u32 {
        // 格子の境界に揃った箱が隣のcellを含まないよう、わずかに縮める。
        let margin = 1e-6 * self.grid.pitch;
        let ranges = cell_ranges(
            &self.grid,
            low.map(|v| v + margin),
            high.map(|v| v - margin),
        );
        let mut depth = 0;
        for z in ranges[2].clone() {
            for y in ranges[1].clone() {
                for x in ranges[0].clone() {
                    depth = depth.max(u32::from(self.levels[self.grid.index(x, y, z)]));
                }
            }
        }
        depth
    }
}

/// box (mm) に重なる格子のcellの、各軸の番号の範囲。
fn cell_ranges(grid: &Grid, low: [f64; 3], high: [f64; 3]) -> [Range<usize>; 3] {
    std::array::from_fn(|axis| {
        let corner = grid.origin[axis] - grid.pitch / 2.0;
        let limit = grid.size[axis] as f64;
        let first = ((low[axis] - corner) / grid.pitch)
            .floor()
            .clamp(0.0, limit) as usize;
        let last = ((high[axis] - corner) / grid.pitch)
            .ceil()
            .clamp(0.0, limit) as usize;
        first..last.max(first)
    })
}

/// 解析の格子の配置。基準の格子 (間隔`pitch`) のcellを局所的に分割した八分木の、材料のcell。
struct Layout {
    /// 格子座標の原点 (基準の格子の最初のcellの最小の隅) と単位。単位はmm。
    origin: [f64; 3],
    unit: f64,
    /// 要素のcell。最小の隅のzの順に並ぶ。
    cells: Vec<Cell>,
}

impl Layout {
    /// 要素の一辺。単位はmm。
    fn size_mm(&self, element: usize) -> f64 {
        self.cells[element].1 as f64 * self.unit
    }

    fn point(&self, lattice: [f64; 3]) -> [f64; 3] {
        std::array::from_fn(|axis| self.origin[axis] + lattice[axis] * self.unit)
    }

    fn centre(&self, element: usize) -> [f64; 3] {
        let (at, size) = self.cells[element];
        self.point(std::array::from_fn(|axis| {
            at[axis] as f64 + size as f64 / 2.0
        }))
    }

    /// 要素の一辺の最小と最大。単位はmm。
    fn pitches(&self) -> (f64, f64) {
        (0..self.cells.len())
            .map(|e| self.size_mm(e))
            .fold((f64::INFINITY, 0.0), |(lo, hi), s| (lo.min(s), hi.max(s)))
    }

    /// 格子間隔の表記。一様な格子では1つの値、細かくした格子では最小から最大まで。
    fn label(&self) -> String {
        range_label(self.pitches(), 1.0)
    }

    /// maskの要素の、面を共有して連結する成分ごとの外接box。大きい成分から`MAX_LOCATIONS`件
    /// までと、成分の総数。
    fn locations(&self, mask: &[bool]) -> (Vec<Location>, usize) {
        let locator = Locator::new(&self.cells);
        let mut parent: Vec<usize> = (0..self.cells.len()).collect();
        fn root(parent: &mut [usize], mut node: usize) -> usize {
            while parent[node] != node {
                parent[node] = parent[parent[node]];
                node = parent[node];
            }
            node
        }
        for (e, cell) in self.cells.iter().enumerate() {
            if !mask[e] {
                continue;
            }
            for (axis, positive, quarter) in face_probes() {
                if let Some(k) = locator.find(&self.cells, probe(cell, axis, positive, quarter), 4)
                    && mask[k]
                {
                    let (a, b) = (root(&mut parent, e), root(&mut parent, k));
                    parent[a.max(b)] = a.min(b);
                }
            }
        }
        // 成分は最初の要素の順に番号を付ける。
        let mut slots: HashMap<usize, usize> = HashMap::new();
        let mut boxes: Vec<([f64; 3], [f64; 3], usize)> = Vec::new();
        for (e, cell) in self.cells.iter().enumerate() {
            if !mask[e] {
                continue;
            }
            let r = root(&mut parent, e);
            let next = boxes.len();
            let slot = *slots.entry(r).or_insert(next);
            if slot == next {
                boxes.push(([f64::INFINITY; 3], [f64::NEG_INFINITY; 3], 0));
            }
            let low = self.point(cell.0.map(|v| v as f64));
            let high = self.point(cell.0.map(|v| (v + cell.1) as f64));
            let entry = &mut boxes[slot];
            for axis in 0..3 {
                entry.0[axis] = entry.0[axis].min(low[axis]);
                entry.1[axis] = entry.1[axis].max(high[axis]);
            }
            entry.2 += 1;
        }
        let total = boxes.len();
        let mut order: Vec<usize> = (0..total).collect();
        order.sort_by(|a, b| boxes[*b].2.cmp(&boxes[*a].2).then(a.cmp(b)));
        let locations = order
            .into_iter()
            .take(MAX_LOCATIONS)
            .map(|slot| Location {
                min: boxes[slot].0,
                max: boxes[slot].1,
            })
            .collect();
        (locations, total)
    }
}

/// 値の範囲の表記。`(最小, 最大)`に`factor`を掛け、単位mmを付ける。
fn range_label((low, high): (f64, f64), factor: f64) -> String {
    if low == high {
        format!("{} mm", factor * low)
    } else {
        format!("{}–{} mm", factor * low, factor * high)
    }
}

/// 面の法線の軸に対する面内の2軸。
fn face_axes(axis: usize) -> [usize; 2] {
    match axis {
        0 => [1, 2],
        1 => [0, 2],
        _ => [0, 1],
    }
}

/// 6面の、1/4ずつの位置。(法線の軸, 正の側か, 1/4の位置)。
fn face_probes() -> impl Iterator<Item = (usize, bool, usize)> {
    (0..3).flat_map(|axis| {
        [false, true]
            .into_iter()
            .flat_map(move |positive| (0..4).map(move |quarter| (axis, positive, quarter)))
    })
}

/// cellの面の1/4の中心から、面の外へわずかに出た点。格子座標の4倍で表す。
fn probe(cell: &Cell, axis: usize, positive: bool, quarter: usize) -> [i64; 3] {
    let [a, b] = face_axes(axis);
    let (at, size) = *cell;
    let mut point = [0i64; 3];
    point[axis] = if positive {
        4 * (at[axis] + size) + 1
    } else {
        4 * at[axis] - 1
    };
    point[a] = 4 * at[a] + size * (2 * (quarter & 1) as i64 + 1);
    point[b] = 4 * at[b] + size * (2 * ((quarter >> 1) & 1) as i64 + 1);
    point
}

/// 露出した面。要素の面全体か、隣の小さいcellに一部を覆われた面の1/4。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Patch {
    element: usize,
    axis: usize,
    positive: bool,
    /// 面内の2軸の側 (ビット0とビット1) で表した1/4の位置。面全体ではNone。
    quarter: Option<usize>,
}

impl Patch {
    /// 面の4隅の、要素内の隅の番号 (`CORNERS`の位置)。kのビット0とビット1は面内の2軸の側。
    fn corner(&self, k: usize) -> usize {
        let [a, b] = face_axes(self.axis);
        let mut offset = [0usize; 3];
        offset[self.axis] = usize::from(self.positive);
        offset[a] = k & 1;
        offset[b] = (k >> 1) & 1;
        CORNERS.iter().position(|c| *c == offset).expect("a corner")
    }

    /// 面の4隅への一様な分布の配分 (合計1) と、隅が面の範囲に含まれるか。隅の番号は要素内の番号。
    /// 双一次の形状関数の面の範囲での積分を、範囲の面積で割った値である。
    fn shares(&self) -> [(usize, f64, bool); 4] {
        std::array::from_fn(|k| match self.quarter {
            None => (self.corner(k), 0.25, true),
            Some(q) => {
                let side = |bit: usize| {
                    if (k >> bit) & 1 == (q >> bit) & 1 {
                        0.75
                    } else {
                        0.25
                    }
                };
                (self.corner(k), side(0) * side(1), k == q)
            }
        })
    }

    fn area(&self, layout: &Layout) -> f64 {
        let size = layout.size_mm(self.element);
        if self.quarter.is_some() {
            size * size / 4.0
        } else {
            size * size
        }
    }

    fn centre(&self, layout: &Layout) -> [f64; 3] {
        let size = layout.size_mm(self.element);
        let mut point = layout.centre(self.element);
        point[self.axis] += if self.positive { 0.5 } else { -0.5 } * size;
        if let Some(q) = self.quarter {
            let [a, b] = face_axes(self.axis);
            point[a] += ((q & 1) as f64 - 0.5) * size / 2.0;
            point[b] += (((q >> 1) & 1) as f64 - 0.5) * size / 2.0;
        }
        point
    }
}

/// 材料のcellに覆われていない面。
fn exposed(layout: &Layout, locator: &Locator) -> Vec<Patch> {
    let mut patches = Vec::new();
    for (element, cell) in layout.cells.iter().enumerate() {
        for axis in 0..3 {
            for positive in [false, true] {
                let open: Vec<usize> = (0..4)
                    .filter(|quarter| {
                        locator
                            .find(&layout.cells, probe(cell, axis, positive, *quarter), 4)
                            .is_none()
                    })
                    .collect();
                let patch = |quarter| Patch {
                    element,
                    axis,
                    positive,
                    quarter,
                };
                if open.len() == 4 {
                    patches.push(patch(None));
                } else {
                    patches.extend(open.into_iter().map(|q| patch(Some(q))));
                }
            }
        }
    }
    patches
}

/// 解析の格子を作る。基準の格子のcellを`refinement`の段まで分割し、中心が部品に含まれる葉を
/// 要素とする。分割しない基準のcellは、従来の格子と同じく`Grid::rasterize_midpoint`で判定する。
fn mesh(
    part: &Part,
    pitch: f64,
    refinement: Option<&Refinement>,
    limit: usize,
) -> Result<(Layout, Geometry, Locator), Unsolved> {
    let grid = Grid::rasterize_midpoint(part, pitch)
        .map_err(|reason| format!("{reason} at grid {pitch} mm"))?;
    let base = 1i64 << MAX_REFINEMENT;
    let unit = pitch / base as f64;
    let origin = grid.origin.map(|v| v - pitch / 2.0);
    let point = |lattice: [i64; 3]| -> [f64; 3] {
        std::array::from_fn(|axis| origin[axis] + lattice[axis] as f64 * unit)
    };
    let depth = |cell: &Cell| {
        refinement.map_or(0, |r| {
            r.depth(point(cell.0), point(cell.0.map(|v| v + cell.1)))
        })
    };
    let tree = Tree::new(grid.size, base, depth, MAX_LEAVES.max(8 * limit)).map_err(|count| {
        format!("local refinement needs more than {count} cells at grid {pitch} mm")
    })?;
    let [nx, ny, nz] = grid.size;
    let mut cells: Vec<Cell> = Vec::new();
    for z in 0..nz {
        for y in 0..ny {
            for x in 0..nx {
                let index = [x, y, z].map(|v| v as i64);
                if tree.is_whole(index) && grid.occupied[grid.index(x, y, z)] {
                    cells.push((index.map(|v| v * base), base));
                }
            }
        }
    }
    for cell in tree.refined_leaves() {
        let centre: [f64; 3] = std::array::from_fn(|axis| {
            origin[axis] + (cell.0[axis] as f64 + cell.1 as f64 / 2.0) * unit
        });
        if part_contains(part, centre) {
            cells.push(cell);
        }
    }
    if cells.len() > limit {
        return Err(format!(
            "local refinement needs {} cells at grid {pitch} mm, above the limit of {limit}",
            cells.len()
        ));
    }
    cells.sort_unstable_by_key(|cell| [cell.0[2], cell.0[1], cell.0[0]]);
    let mut positions: Vec<[i64; 3]> = cells
        .iter()
        .flat_map(|cell| (0..8).map(move |a| corner(cell, a)))
        .collect();
    positions.sort_unstable_by_key(|p| [p[2], p[1], p[0]]);
    positions.dedup();
    let index: HashMap<[i64; 3], usize> =
        positions.iter().enumerate().map(|(k, p)| (*p, k)).collect();
    let elements: Vec<[usize; 8]> = cells
        .iter()
        .map(|cell| std::array::from_fn(|a| index[&corner(cell, a)]))
        .collect();
    let locator = Locator::new(&cells);
    let constraints = if tree.refined() {
        constraints(&positions, &cells, &elements, &locator)
    } else {
        Constraints::none(positions.len())
    };
    let layout = Layout {
        origin,
        unit,
        cells: cells.clone(),
    };
    let geometry = Geometry {
        positions,
        cells,
        elements,
        constraints,
    };
    Ok((layout, geometry, locator))
}

/// 節点を共有する要素の連結成分のうち、`anchored`の節点を1つも含まないものがあるか。
/// そのような成分は剛体変位や温度が定まらず、方程式が特異になる。従属する節点は主節点と
/// つながる。
fn floating(geometry: &Geometry, anchored: impl Fn(usize) -> bool) -> bool {
    let mut parent: Vec<usize> = (0..geometry.positions.len()).collect();
    fn root(parent: &mut [usize], mut node: usize) -> usize {
        while parent[node] != node {
            parent[node] = parent[parent[node]];
            node = parent[node];
        }
        node
    }
    let join = |parent: &mut Vec<usize>, a: usize, b: usize| {
        let (ra, rb) = (root(parent, a), root(parent, b));
        parent[rb] = ra;
    };
    for element in &geometry.elements {
        for node in &element[1..] {
            join(&mut parent, element[0], *node);
        }
    }
    for node in 0..geometry.positions.len() {
        for (master, _) in geometry.constraints.masters(node) {
            join(&mut parent, node, *master);
        }
    }
    let mut held = vec![false; parent.len()];
    for node in (0..parent.len()).filter(|n| !geometry.constraints.is_dependent(*n) && anchored(*n))
    {
        let r = root(&mut parent, node);
        held[r] = true;
    }
    geometry.elements.iter().any(|element| {
        let r = root(&mut parent, element[0]);
        !held[r]
    })
}

/// 領域に含まれる露出した面。keepout、コネクタ開口、ネジ固定の領域は`reach`だけ広げる。
/// 格子間隔hと2hの解で同じ領域を使うため、`reach`は格子に依らない値とする。
fn region_patches(
    layout: &Layout,
    model: &Model,
    part: &Part,
    region: &Region,
    patches: &[Patch],
    reach: f64,
) -> Vec<Patch> {
    let within = |point: [f64; 3], low: [f64; 3], high: [f64; 3]| {
        (0..3).all(|axis| point[axis] >= low[axis] - 1e-9 && point[axis] <= high[axis] + 1e-9)
    };
    let grown = |low: [f64; 3], high: [f64; 3]| (low.map(|v| v - reach), high.map(|v| v + reach));
    let predicate: Box<dyn Fn([f64; 3]) -> bool> = match region {
        Region::Surface => Box::new(|_| true),
        Region::Box { min, max } => {
            let (min, max) = (*min, *max);
            Box::new(move |point| within(point, min, max))
        }
        Region::Keepout { id } => {
            let keepout = model
                .keepouts
                .iter()
                .find(|k| &k.id == id)
                .expect("validated");
            let (low, high) = grown(keepout.shape.aabb().0, keepout.shape.aabb().1);
            Box::new(move |point| within(point, low, high))
        }
        Region::Connector { id } => {
            let connector = model
                .connectors
                .iter()
                .find(|c| &c.id == id)
                .expect("validated");
            let opening = part.features.iter().find(|f| f.id == connector.opening);
            match opening {
                Some(feature) => {
                    let (low, high) = feature.shape.aabb();
                    let (low, high) = grown(low, high);
                    Box::new(move |point| within(point, low, high))
                }
                None => Box::new(|_| false),
            }
        }
        Region::Fastener { id } => {
            let fastener = model
                .fasteners
                .iter()
                .find(|f| &f.id == id)
                .expect("validated");
            let axis = fastener.direction.axis();
            let along = axis.index();
            let [first, second] = axis.plane();
            let sign = if fastener.direction.is_positive() {
                1.0
            } else {
                -1.0
            };
            let center = fastener.center;
            let (radius, span) = if fastener.base == part.id {
                // 受け側の部品では、下穴の壁がネジの荷重を受ける。
                let bore = match fastener.anchor {
                    Anchor::SelfTapping { pilot_mm } => pilot_mm,
                    Anchor::Insert { hole_mm, .. } => hole_mm,
                };
                let tip = fastener.seat_mm + sign * fastener.screw.length_mm;
                (
                    bore / 2.0 + reach,
                    (fastener.joint_mm.min(tip), fastener.joint_mm.max(tip)),
                )
            } else {
                // 締め付けられる部品では、頭の座面が荷重を受ける。
                (
                    fastener.screw.head_mm / 2.0 + reach,
                    (fastener.seat_mm - reach, fastener.seat_mm + reach),
                )
            };
            Box::new(move |point| {
                let radial = (point[first] - center[0]).hypot(point[second] - center[1]);
                radial <= radius && point[along] >= span.0 && point[along] <= span.1
            })
        }
    };
    // 面の中心で判定する。cellの中心で判定すると、端のcellの側面まで領域に含まれる。
    patches
        .iter()
        .copied()
        .filter(|patch| predicate(patch.centre(layout)))
        .collect()
}

/// 領域の合計を1とした主節点ごとの割合。面ごとに面積に比例して配り、面の中では双一次の
/// 形状関数の積分で4隅へ配る。一様な表面荷重 (熱流束) を節点へ配ったものに当たり、節点へ
/// 等分するのと違って縁と角の節点へ荷重が集中しない。従属する隅の分は主節点へ配る。
fn surface_shares(layout: &Layout, geometry: &Geometry, patches: &[Patch]) -> Vec<(usize, f64)> {
    let total: f64 = patches.iter().map(|p| p.area(layout)).sum();
    let mut out = Vec::new();
    for patch in patches {
        let fraction = patch.area(layout) / total;
        for (corner, share, _) in patch.shares() {
            let node = geometry.elements[patch.element][corner];
            for (master, weight) in geometry.constraints.expand(node) {
                out.push((master, fraction * share * weight));
            }
        }
    }
    out
}

/// 面の範囲に含まれる隅の主節点。拘束や固定温度を与える節点である。従属する隅は主節点で
/// 値が決まるため除く。
fn patch_nodes<'a>(geometry: &'a Geometry, patch: &Patch) -> impl Iterator<Item = usize> + 'a {
    let element = geometry.elements[patch.element];
    patch
        .shares()
        .into_iter()
        .filter(|(_, _, inside)| *inside)
        .map(move |(corner, _, _)| element[corner])
        .filter(|node| !geometry.constraints.is_dependent(*node))
}

/// 拘束と荷重の要素から、各軸に`factor`·(要素の一辺)以内に中心がある要素。
fn near_boundary(layout: &Layout, boundary: &[bool], factor: f64) -> Vec<bool> {
    let count = layout.cells.len();
    let reach: Vec<f64> = (0..count).map(|e| factor * layout.size_mm(e)).collect();
    let bucket = reach.iter().copied().fold(0.0, f64::max);
    if bucket <= 0.0 {
        return boundary.to_vec();
    }
    let key = |point: [f64; 3]| point.map(|v| (v / bucket).floor() as i64);
    let mut buckets: HashMap<[i64; 3], Vec<[f64; 3]>> = HashMap::new();
    for e in (0..count).filter(|e| boundary[*e]) {
        let centre = layout.centre(e);
        buckets.entry(key(centre)).or_default().push(centre);
    }
    (0..count)
        .map(|e| {
            let centre = layout.centre(e);
            let limit = reach[e] + 1e-9 * bucket;
            let low = key(centre.map(|v| v - limit));
            let high = key(centre.map(|v| v + limit));
            (low[2]..=high[2]).any(|z| {
                (low[1]..=high[1]).any(|y| {
                    (low[0]..=high[0]).any(|x| {
                        buckets.get(&[x, y, z]).is_some_and(|points| {
                            points
                                .iter()
                                .any(|p| (0..3).all(|axis| (p[axis] - centre[axis]).abs() <= limit))
                        })
                    })
                })
            })
        })
        .collect()
}

/// 1つの格子での線形弾性の解。
struct ElasticSolution {
    layout: Layout,
    /// 節点の変位の大きさの最大。単位はmm。
    deflection_mm: f64,
    /// 要素の8隅のvon Mises相当応力の最大と、要素ごとの値。単位はMPa。拘束と荷重の近傍の
    /// 要素を除く。
    stress_mpa: f64,
    stress: Vec<(usize, f64)>,
    /// 要素の8隅の積層方向の垂直応力 (引張) の最大と、要素ごとの値。積層しない製造法では空。
    /// 拘束と荷重の近傍の要素を除く。
    interlayer_mpa: f64,
    interlayer: Vec<(usize, f64)>,
    /// 拘束と荷重の近傍の要素のvon Mises相当応力の最大。近傍が無ければNone。
    boundary_mpa: Option<f64>,
    /// 変位の大きい節点を含む要素。
    displaced: Vec<(usize, f64)>,
    /// 表面の要素の値。分布の表示に使う。
    surface: Vec<SurfaceCell>,
    iterations: usize,
}

/// 表面の1要素の、要素の8隅の応力の最大、層間の引張の最大、8節点の変位の大きさの最大。
struct SurfaceCell {
    element: usize,
    stress: f64,
    interlayer: f64,
    deflection: f64,
    /// 拘束と荷重の近傍で、応力を判定しない。
    excluded: bool,
}

/// 露出した面を持つ要素。
fn surface_mask(length: usize, patches: &[Patch]) -> Vec<bool> {
    let mut mask = vec![false; length];
    for patch in patches {
        mask[patch.element] = true;
    }
    mask
}

/// 解析できない理由。checkのmessageに書く。
type Unsolved = String;

/// 格子間隔`pitch`の格子で解く。`exclusion`は、拘束と荷重の近傍として応力を判定しない範囲を
/// 要素の一辺に対する倍数で与える。
#[allow(clippy::too_many_arguments)]
fn solve_elastic(
    model: &Model,
    part: &Part,
    case: &StructuralCase,
    material: &Material,
    pitch: f64,
    refinement: Option<&Refinement>,
    reach: f64,
    exclusion: f64,
) -> Result<ElasticSolution, Unsolved> {
    let (layout, geometry, locator) = mesh(part, pitch, refinement, MAX_ELEMENTS)?;
    let elastic = material.elastic.as_ref().expect("validated");
    let element = unit_element(elastic.poisson_ratio);
    let nodes = geometry.positions.len();
    let dofs = 3 * nodes;
    let patches = exposed(&layout, &locator);
    let count = layout.cells.len();

    let mut fixed = vec![false; dofs];
    // 拘束と荷重の面を持つ要素。応力の判定から除く近傍の起点である。
    let mut boundary = vec![false; count];
    for region in &case.supports {
        let selected = region_patches(&layout, model, part, region, &patches, reach);
        if selected.is_empty() {
            return Err(format!(
                "support {region:?} covers no surface at grid {pitch} mm"
            ));
        }
        for patch in &selected {
            boundary[patch.element] = true;
            for node in patch_nodes(&geometry, patch) {
                for c in 0..3 {
                    fixed[3 * node + c] = true;
                }
            }
        }
    }
    let mut rhs = vec![0.0; dofs];
    for force in &case.forces {
        let selected = region_patches(&layout, model, part, &force.region, &patches, reach);
        if selected.is_empty() {
            return Err(format!(
                "force region {:?} covers no surface at grid {pitch} mm",
                force.region
            ));
        }
        for patch in &selected {
            boundary[patch.element] = true;
        }
        for (node, share) in surface_shares(&layout, &geometry, &selected) {
            for c in 0..3 {
                rhs[3 * node + c] += force.newtons[c] * share;
            }
        }
    }
    if floating(&geometry, |node| fixed[3 * node]) {
        return Err(format!(
            "a connected piece of the part touches no support at grid {pitch} mm"
        ));
    }
    let mut displacement = vec![0.0; dofs];
    let table = vec![
        element
            .stiffness
            .iter()
            .flatten()
            .map(|k| elastic.youngs_modulus_mpa * k)
            .collect(),
    ];
    // 一辺Lの要素の剛性は、一辺1の要素のL倍である。
    let assigned = (0..count).map(|e| (0, layout.size_mm(e))).collect();
    let hierarchy = Hierarchy::new(Level::new(3, geometry, table, assigned, Vec::new(), fixed));
    let Some(iterations) = hierarchy.solve(&rhs, &mut displacement) else {
        return Err(format!("the solver did not converge at grid {pitch} mm"));
    };
    let geometry = hierarchy.fine().geometry();

    let deflection = |node: usize| {
        (0..3)
            .map(|c| displacement[3 * node + c].powi(2))
            .sum::<f64>()
            .sqrt()
    };
    let deflection_mm = (0..nodes).map(deflection).fold(0.0, f64::max);
    let d = elasticity(elastic.poisson_ratio);
    // 要素の8隅で応力を求める。表面の隅の値は、平らな面の曲げで表面の応力に当たる。
    let corners = CORNERS.map(|c| {
        let point = c.map(|v| v as f64);
        (strain_matrix(point), bubble_matrix(point))
    });
    let build = part
        .adopted_plan()
        .filter(|plan| plan.process.layering().is_some())
        .map(|plan| plan.orientation.build_vector());
    let near = near_boundary(&layout, &boundary, exclusion);
    let mut boundary_mpa: Option<f64> = None;
    let mut stress = Vec::with_capacity(count);
    let mut interlayer = Vec::new();
    let mut displaced = Vec::with_capacity(count);
    let on_surface = surface_mask(count, &patches);
    let mut surface = Vec::new();
    for (e, nodes) in geometry.elements.iter().enumerate() {
        let moved = nodes.iter().map(|n| deflection(*n)).fold(0.0, f64::max);
        displaced.push((e, moved));
        let mut local = [0.0; 24];
        for (a, node) in nodes.iter().enumerate() {
            for c in 0..3 {
                local[3 * a + c] = displacement[3 * node + c];
            }
        }
        let size = layout.size_mm(e);
        let mut mises_max = 0.0f64;
        let mut normal_max = 0.0f64;
        let amplitude: [f64; 9] = std::array::from_fn(|m| {
            element.internal[m]
                .iter()
                .zip(local.iter())
                .map(|(r, u)| r * u)
                .sum()
        });
        for (b, g) in &corners {
            // 一辺Lの要素のひずみは、一辺1の行列の値を1/L倍したものである。
            let strain: [f64; 6] = std::array::from_fn(|i| {
                let nodal: f64 = b[i].iter().zip(local.iter()).map(|(b, u)| b * u).sum();
                let bubble: f64 = g[i].iter().zip(amplitude.iter()).map(|(g, a)| g * a).sum();
                (nodal + bubble) / size
            });
            let sigma: [f64; 6] = std::array::from_fn(|i| {
                elastic.youngs_modulus_mpa * (0..6).map(|m| d[i][m] * strain[m]).sum::<f64>()
            });
            let [sx, sy, sz, tyz, txz, txy] = sigma;
            let mises = (0.5 * ((sx - sy).powi(2) + (sy - sz).powi(2) + (sz - sx).powi(2))
                + 3.0 * (tyz * tyz + txz * txz + txy * txy))
                .sqrt();
            mises_max = mises_max.max(mises);
            if let Some(n) = build {
                let tensor = [[sx, txy, txz], [txy, sy, tyz], [txz, tyz, sz]];
                let normal: f64 = (0..3)
                    .map(|i| (0..3).map(|j| n[i] * tensor[i][j] * n[j]).sum::<f64>())
                    .sum();
                normal_max = normal_max.max(normal);
            }
        }
        if on_surface[e] {
            surface.push(SurfaceCell {
                element: e,
                stress: mises_max,
                interlayer: normal_max,
                deflection: moved,
                excluded: near[e],
            });
        }
        if near[e] {
            boundary_mpa = Some(boundary_mpa.unwrap_or(0.0).max(mises_max));
            continue;
        }
        stress.push((e, mises_max));
        if build.is_some() {
            interlayer.push((e, normal_max));
        }
    }
    let maximum = |values: &[(usize, f64)]| values.iter().map(|(_, v)| *v).fold(0.0, f64::max);
    Ok(ElasticSolution {
        layout,
        deflection_mm,
        stress_mpa: maximum(&stress),
        interlayer_mpa: maximum(&interlayer),
        boundary_mpa,
        stress,
        interlayer,
        displaced,
        surface,
        iterations,
    })
}

/// 1つの格子での定常熱伝導の解。
struct ThermalSolution {
    layout: Layout,
    /// 節点の温度の最大。単位は°C。
    max_c: f64,
    /// 要素の8節点の温度の最大。
    cells: Vec<(usize, f64)>,
    /// 表面の要素の、8節点の温度の最大。分布の表示に使う。
    surface: Vec<(usize, f64)>,
    iterations: usize,
}

fn solve_thermal(
    model: &Model,
    part: &Part,
    case: &ThermalCase,
    material: &Material,
    pitch: f64,
    reach: f64,
) -> Result<ThermalSolution, Unsolved> {
    let (layout, geometry, locator) = mesh(part, pitch, None, MAX_ELEMENTS)?;
    let unit = unit_conduction();
    // 熱伝導率はW/(m·K)で与えられ、mmの格子ではW/(mm·K)とする。
    let conductivity = material.thermal_conductivity_w_per_mk.expect("validated") / 1000.0;
    let n = geometry.positions.len();
    let count = layout.cells.len();
    let patches = exposed(&layout, &locator);
    let mut fixed = vec![false; n];
    let mut temperature = vec![0.0; n];
    // 節点に固定した温度。異なる温度の領域が節点を共有すると、記述順で解が変わるため解かない。
    let mut assigned: Vec<Option<f64>> = vec![None; n];
    for item in &case.fixed_temperatures {
        let selected = region_patches(&layout, model, part, &item.region, &patches, reach);
        if selected.is_empty() {
            return Err(format!(
                "fixed temperature region {:?} covers no surface at grid {pitch} mm",
                item.region
            ));
        }
        for patch in &selected {
            for node in patch_nodes(&geometry, patch) {
                if let Some(other) = assigned[node]
                    && other != item.celsius
                {
                    return Err(format!(
                        "fixed temperatures {other} °C and {} °C meet at the same surface at grid {pitch} mm",
                        item.celsius
                    ));
                }
                assigned[node] = Some(item.celsius);
                fixed[node] = true;
                temperature[node] = item.celsius;
            }
        }
    }
    let mut rhs = vec![0.0; n];
    let mut film = vec![0.0; n];
    // 1つの面は、最初に一致した対流の領域にだけ属する。
    let mut claimed = BTreeSet::new();
    for item in &case.convection {
        let selected = region_patches(&layout, model, part, &item.region, &patches, reach);
        if selected.is_empty() {
            return Err(format!(
                "convection region {:?} covers no surface at grid {pitch} mm",
                item.region
            ));
        }
        for patch in selected {
            if !claimed.insert(patch) {
                continue;
            }
            // 熱伝達係数はW/(m²·K)で与えられ、mm²の面ではW/(mm²·K)とする。膜の項は4隅へ
            // 集中させる。従属する隅の分は主節点へ配る。
            let conductance = item.h_w_per_m2k * 1e-6 * patch.area(&layout);
            for (corner, share, _) in patch.shares() {
                let node = geometry.elements[patch.element][corner];
                for (master, weight) in geometry.constraints.expand(node) {
                    film[master] += conductance * share * weight;
                    rhs[master] += conductance * share * weight * item.ambient_c;
                }
            }
        }
    }
    for input in &case.heat_inputs {
        let selected = region_patches(&layout, model, part, &input.region, &patches, reach);
        if selected.is_empty() {
            return Err(format!(
                "heat input region {:?} covers no surface at grid {pitch} mm",
                input.region
            ));
        }
        for (node, share) in surface_shares(&layout, &geometry, &selected) {
            rhs[node] += input.watts * share;
        }
    }
    if floating(&geometry, |node| fixed[node] || film[node] > 0.0) {
        return Err(format!(
            "a connected piece of the part touches no convection or fixed temperature at grid {pitch} mm"
        ));
    }
    // 対流も固定温度も無い節点の初期値は、周囲温度の平均とする。収束を速めるだけである。
    let start = case
        .convection
        .iter()
        .map(|c| c.ambient_c)
        .chain(case.fixed_temperatures.iter().map(|t| t.celsius))
        .sum::<f64>()
        / (case.convection.len() + case.fixed_temperatures.len()) as f64;
    for node in 0..n {
        if !fixed[node] {
            temperature[node] = start;
        }
    }
    let table = vec![unit.iter().flatten().map(|k| conductivity * k).collect()];
    // 一辺Lの要素の伝導行列は、一辺1の要素のL倍である。
    let scales = (0..count).map(|e| (0, layout.size_mm(e))).collect();
    let hierarchy = Hierarchy::new(Level::new(1, geometry, table, scales, film, fixed));
    let Some(iterations) = hierarchy.solve(&rhs, &mut temperature) else {
        return Err(format!("the solver did not converge at grid {pitch} mm"));
    };
    let geometry = hierarchy.fine().geometry();
    let cells: Vec<(usize, f64)> = geometry
        .elements
        .iter()
        .enumerate()
        .map(|(e, element)| {
            (
                e,
                element
                    .iter()
                    .map(|node| temperature[*node])
                    .fold(f64::MIN, f64::max),
            )
        })
        .collect();
    let max_c = cells.iter().map(|(_, t)| *t).fold(f64::MIN, f64::max);
    let on_surface = surface_mask(count, &patches);
    let surface = cells
        .iter()
        .copied()
        .filter(|(e, _)| on_surface[*e])
        .collect();
    Ok(ThermalSolution {
        layout,
        max_c,
        cells,
        surface,
        iterations,
    })
}

/// 格子間隔h、2h、4hの値から求めた収束の判定。
struct Convergence {
    /// hと2hの値の相対差。
    change: f64,
    /// 上限と比べる値。
    judged: f64,
    /// 収束していない理由。
    unconverged: Option<String>,
}

/// 格子間隔h、2h、4hの値`values`の収束を判定し、安全側に外挿する。値は`reference`からの差で
/// 扱う。相対差の分母は3つの値の絶対値の最大とし、`limit`の`CONVERGENCE_FLOOR`倍を下限とする。
///
/// hと2hの相対差が`CONVERGENCE_TOLERANCE`を超えるか、hと2hの差が2hと4hの差の
/// `MAX_SHRINK_RATIO`倍より縮まなければ収束していないとする。ただし相対差が
/// `NEGLIGIBLE_CHANGE`以下なら縮まなくてもよい。特異点の応力は細分するほど増え、差は縮まない。
///
/// 外挿は差が等比で縮むとみなした残りの和`|d1|·r/(1-r)` (rは差の縮小率) とし、1次の収束に
/// 当たる`|d1|`を下限とする。細分で値が減る場合は外挿せず、判定を緩める向きには外挿しない。
fn converge(values: [f64; 3], reference: f64, limit: f64) -> Convergence {
    let [fine, middle, coarse] = values.map(|v| v - reference);
    let base = fine
        .abs()
        .max(middle.abs())
        .max(coarse.abs())
        .max(CONVERGENCE_FLOOR * (limit - reference).abs());
    let (first, second) = (fine - middle, middle - coarse);
    let change = if base == 0.0 { 0.0 } else { first.abs() / base };
    let negligible = change <= NEGLIGIBLE_CHANGE;
    let shrinking = first.abs() <= MAX_SHRINK_RATIO * second.abs();
    let unconverged = if change > CONVERGENCE_TOLERANCE {
        Some(format!(
            "changes by more than {:.0}% between grids h and 2h",
            100.0 * CONVERGENCE_TOLERANCE
        ))
    } else if !negligible && !shrinking {
        Some(format!(
            "the change between grids h and 2h is not below {MAX_SHRINK_RATIO} times the change between 2h and 4h"
        ))
    } else {
        None
    };
    let ratio = if shrinking {
        first.abs() / second.abs()
    } else {
        0.0
    };
    let tail = first.abs() * (ratio / (1.0 - ratio)).max(1.0);
    let judged = reference + fine + if first > 0.0 { tail } else { 0.0 };
    Convergence {
        change,
        judged,
        unconverged,
    }
}

fn analysis_check(rule: Rule, target: String, status: Status, message: String) -> Check {
    Check {
        plan: None,
        adopted: true,
        locations: Vec::new(),
        rule,
        status,
        target,
        message,
    }
}

/// 格子間隔h、2h、4hの値と上限からcheckを作る。判定値が上限を超えた要素を検出箇所とし、
/// 要素の値も判定値と同じ比で外挿する。`labels`は3つの格子の間隔の表記、`layout`は最も
/// 細かい格子である。
#[allow(clippy::too_many_arguments)]
fn limit_check(
    layout: &Layout,
    labels: &[String; 3],
    rule: Rule,
    target: String,
    quantity: &str,
    unit: &str,
    values: [f64; 3],
    reference: f64,
    limit: f64,
    cells: &[(usize, f64)],
    context: &str,
) -> Check {
    let convergence = converge(values, reference, limit);
    let measured = if reference == 0.0 {
        String::new()
    } else {
        format!(" of the rise above {reference} {unit}")
    };
    let [fine, middle, coarse] = values;
    let summary = format!(
        "{quantity} {fine:.4} {unit} (grid {}), {middle:.4} {unit} (grid {}), {coarse:.4} {unit} (grid {}); change{measured} {:.1}%; extrapolated {:.4} {unit}; limit {limit:.4} {unit}; {context}",
        labels[0],
        labels[1],
        labels[2],
        100.0 * convergence.change,
        convergence.judged,
    );
    if let Some(reason) = convergence.unconverged {
        return analysis_check(
            rule,
            target,
            Status::NotEvaluated,
            format!("not evaluated: {quantity} {reason}; {summary}"),
        );
    }
    let passed = convergence.judged <= limit;
    let mut check = analysis_check(
        rule,
        target,
        if passed { Status::Pass } else { Status::Fail },
        summary,
    );
    if !passed {
        let rise = fine - reference;
        let ratio = if rise > 0.0 {
            (convergence.judged - reference) / rise
        } else {
            1.0
        };
        let mut mask = vec![false; layout.cells.len()];
        for (element, value) in cells {
            if reference + (value - reference) * ratio > limit {
                mask[*element] = true;
            }
        }
        let (locations, total) = layout.locations(&mask);
        check.message.push_str(&format!("; {total} location(s)"));
        check.locations = locations;
    }
    check
}

/// 格子間隔h、2h、4hで解く。`solve`は格子の段 (0、1、2) を受け取る。いずれかが解けなければ
/// その理由を返す。
fn solve_three<T>(mut solve: impl FnMut(u32) -> Result<T, Unsolved>) -> Result<[T; 3], Unsolved> {
    Ok([solve(0)?, solve(1)?, solve(2)?])
}

/// 解析の格子間隔。ケースが与えなければpolicyの値。
fn case_pitch(model: &Model, voxel_mm: Option<f64>) -> f64 {
    voxel_mm.unwrap_or(model.policy.voxel_mm)
}

/// 分布として表示する量。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Quantity {
    /// von Mises相当応力。
    Stress,
    /// 積層方向の垂直応力の引張。
    Interlayer,
    /// 変位の大きさ。
    Deflection,
    Temperature,
}

/// 部品の表面のcellに置いた解析の量。最も細かい格子の解の値 (外挿前) であり、判定には使わない。
/// viewerが部品の表面を色分けするために使う。cellの位置は格子座標で、点`origin + lattice·座標`
/// に当たる。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Field {
    pub case: String,
    pub quantity: Quantity,
    pub part: String,
    pub unit: String,
    /// 判定の上限。変位の上限を与えないケースではNone。
    pub limit: Option<f64>,
    /// 値の基準。温度では周囲温度と固定温度の最小、他は0。
    pub reference: f64,
    /// cellの一辺の最小。単位はmm。
    pub pitch: f64,
    /// cellの一辺の最大。一様な格子ではNone。単位はmm。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_pitch: Option<f64>,
    /// 格子座標の原点と単位。単位はmm。
    pub origin: [f64; 3],
    pub lattice: f64,
    /// 表面のcellの最小の隅の格子座標と一辺、および値。
    pub cells: Vec<[i64; 4]>,
    pub values: Vec<f32>,
    /// 拘束と荷重の近傍で応力を判定しないcell。`cells`と同じ順。応力以外では空。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub excluded: Vec<bool>,
    /// 判定しない近傍の、拘束と荷重の領域からの距離の最大。応力以外ではNone。単位はmm。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exclusion_mm: Option<f64>,
    /// 判定しない近傍の距離の最小。距離が一様な場合はNone。単位はmm。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_exclusion_mm: Option<f64>,
}

/// 分布の表面のcellを、一辺ごとに最小の隅で引く索引。
struct Sampler<'a> {
    field: &'a Field,
    /// 一辺 (大きい順) と、最小の隅から`cells`の位置。
    index: Vec<(i64, HashMap<[i64; 3], usize>)>,
}

impl<'a> Sampler<'a> {
    fn new(field: &'a Field) -> Self {
        let mut index: Vec<(i64, HashMap<[i64; 3], usize>)> = Vec::new();
        for (k, cell) in field.cells.iter().enumerate() {
            let size = cell[3];
            let slot = match index.iter().position(|(s, _)| *s == size) {
                Some(slot) => slot,
                None => {
                    index.push((size, HashMap::new()));
                    index.len() - 1
                }
            };
            index[slot].1.insert([cell[0], cell[1], cell[2]], k);
        }
        index.sort_by_key(|entry| std::cmp::Reverse(entry.0));
        Self { field, index }
    }

    /// 点に最も近い表面のcellの番号 (`cells`の位置)。中心がそのcellの一辺の2倍以内に無ければ
    /// 候補としない。
    fn nearest(&self, point: [f32; 3]) -> Option<usize> {
        let field = self.field;
        let position: [f64; 3] = std::array::from_fn(|axis| {
            (f64::from(point[axis]) - field.origin[axis]) / field.lattice
        });
        let mut best: Option<(f64, usize)> = None;
        for (size, cells) in &self.index {
            let s = *size as f64;
            // 中心 (i + 1/2)·sが点から2s以内にあるcellの番号i。
            let range: [(i64, i64); 3] = std::array::from_fn(|axis| {
                (
                    ((position[axis] - 2.5 * s) / s).ceil() as i64,
                    ((position[axis] + 1.5 * s) / s).floor() as i64,
                )
            });
            for z in range[2].0..=range[2].1 {
                for y in range[1].0..=range[1].1 {
                    for x in range[0].0..=range[0].1 {
                        let Some(k) = cells.get(&[x * size, y * size, z * size]) else {
                            continue;
                        };
                        let distance: f64 = [x, y, z]
                            .iter()
                            .zip(position)
                            .map(|(i, p)| ((*i as f64 + 0.5) * s - p).powi(2))
                            .sum();
                        if distance <= 4.0 * s * s && best.is_none_or(|(d, _)| distance < d) {
                            best = Some((distance, *k));
                        }
                    }
                }
            }
        }
        best.map(|(_, k)| k)
    }

    fn value(&self, point: [f32; 3]) -> f32 {
        self.nearest(point)
            .map_or(f32::NAN, |k| self.field.values[k])
    }
}

/// 点ごとの、最も近い表面のcellの値と、そのcellが判定しない近傍にあるか。cellの一辺の2倍以内に
/// 表面のcellが無い点の値はNaNとする。点はviewerの部品meshの頂点で、組立座標である。
pub fn sample(field: &Field, points: &[[f32; 3]]) -> (Vec<f32>, Vec<bool>) {
    let sampler = Sampler::new(field);
    points
        .iter()
        .map(|point| match sampler.nearest(*point) {
            Some(k) => (
                field.values[k],
                field.excluded.get(k).copied().unwrap_or(false),
            ),
            None => (f32::NAN, false),
        })
        .unzip()
}

/// 分布を色で描くための三角形。辺が`max_edge`を超え、かついずれかの分布の値を3頂点からの線形
/// 補間で表すと、内側の点で分布の値の範囲の`tolerance`倍を超えて外れる三角形を、最長の辺の中点で
/// 2つに分けることを繰り返す。値が一様な所の三角形は大きいまま残す。内側の点は間隔`max_edge`で
/// 取る。三角形は頂点を共有せず、3頂点ずつ並べて返す。
pub fn refine(
    triangles: &[[[f32; 3]; 3]],
    fields: &[Field],
    max_edge: f64,
    tolerance: f64,
) -> Vec<[[f32; 3]; 3]> {
    let ranges: Vec<f64> = fields
        .iter()
        .map(|field| {
            let (low, high) = field
                .values
                .iter()
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| {
                    (lo.min(f64::from(*v)), hi.max(f64::from(*v)))
                });
            (high - low).max(1e-12)
        })
        .collect();
    let samplers: Vec<Sampler> = fields.iter().map(Sampler::new).collect();
    let values_at = |points: &[[f32; 3]]| -> Vec<Vec<f32>> {
        samplers
            .iter()
            .map(|sampler| points.iter().map(|p| sampler.value(*p)).collect())
            .collect()
    };
    let length = |a: [f32; 3], b: [f32; 3]| -> f64 {
        (0..3)
            .map(|k| f64::from(a[k] - b[k]).powi(2))
            .sum::<f64>()
            .sqrt()
    };
    // 3頂点の重心座標で表した内側の点。間隔max_edgeの格子点 (辺の上を除く)。
    let probes = |triangle: &[[f32; 3]; 3], longest: f64| -> Vec<([f32; 3], [f64; 3])> {
        let n = (longest / max_edge).ceil().max(2.0) as usize;
        let mut out = Vec::new();
        for i in 1..n {
            for j in 1..n - i {
                let (u, v) = (i as f64 / n as f64, j as f64 / n as f64);
                let w = [1.0 - u - v, u, v];
                let point = std::array::from_fn(|k| {
                    (0..3)
                        .map(|c| w[c] * f64::from(triangle[c][k]))
                        .sum::<f64>() as f32
                });
                out.push((point, w));
            }
        }
        // 最長の辺の中点も確かめる。
        for e in 0..3 {
            let mut w = [0.0; 3];
            w[e] = 0.5;
            w[(e + 1) % 3] = 0.5;
            let point = std::array::from_fn(|k| (triangle[e][k] + triangle[(e + 1) % 3][k]) / 2.0);
            out.push((point, w));
        }
        out
    };
    let mut out = Vec::new();
    let mut stack: Vec<[[f32; 3]; 3]> = triangles.to_vec();
    while let Some(triangle) = stack.pop() {
        let lengths = [0, 1, 2].map(|e| length(triangle[e], triangle[(e + 1) % 3]));
        let e = (0..3)
            .max_by(|a, b| lengths[*a].total_cmp(&lengths[*b]))
            .expect("three edges");
        if lengths[e] <= max_edge || fields.is_empty() {
            out.push(triangle);
            continue;
        }
        let corners = values_at(&triangle);
        let inside = probes(&triangle, lengths[e]);
        let points: Vec<[f32; 3]> = inside.iter().map(|(p, _)| *p).collect();
        let sampled = values_at(&points);
        let linear = fields.iter().enumerate().all(|(f, _)| {
            inside.iter().enumerate().all(|(k, (_, w))| {
                let actual = f64::from(sampled[f][k]);
                let corner: Vec<f64> = corners[f].iter().map(|v| f64::from(*v)).collect();
                if actual.is_nan() || corner.iter().any(|v| v.is_nan()) {
                    // 値の無い点は、その周りを細かく描く必要がない。値が混ざる場合は分ける。
                    return actual.is_nan() && corner.iter().all(|v| v.is_nan());
                }
                let interpolated: f64 = (0..3).map(|c| w[c] * corner[c]).sum();
                (actual - interpolated).abs() <= tolerance * ranges[f]
            })
        });
        if linear {
            out.push(triangle);
            continue;
        }
        let (a, b, c) = (triangle[e], triangle[(e + 1) % 3], triangle[(e + 2) % 3]);
        let middle = std::array::from_fn(|k| (a[k] + b[k]) / 2.0);
        stack.push([a, middle, c]);
        stack.push([middle, b, c]);
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn field(
    case: &str,
    quantity: Quantity,
    part: &Part,
    unit: &str,
    limit: Option<f64>,
    reference: f64,
    layout: &Layout,
    cells: impl Iterator<Item = (usize, f64, bool)>,
) -> Field {
    let (low, high) = layout.pitches();
    let mut field = Field {
        case: case.into(),
        quantity,
        part: part.id.clone(),
        unit: unit.into(),
        limit,
        reference,
        pitch: low,
        max_pitch: (high > low).then_some(high),
        origin: layout.origin,
        lattice: layout.unit,
        cells: Vec::new(),
        values: Vec::new(),
        excluded: Vec::new(),
        exclusion_mm: None,
        min_exclusion_mm: None,
    };
    let mut any_excluded = false;
    for (element, value, excluded) in cells {
        let (at, size) = layout.cells[element];
        field.cells.push([at[0], at[1], at[2], size]);
        field.values.push(value as f32);
        field.excluded.push(excluded);
        any_excluded |= excluded;
    }
    if !any_excluded {
        field.excluded.clear();
    }
    field
}

/// 応力の分布に、判定しない近傍の距離を記す。
fn with_exclusion(mut field: Field, (low, high): (f64, f64)) -> Field {
    field.exclusion_mm = Some(high);
    field.min_exclusion_mm = (high > low).then_some(low);
    field
}

/// 宣言した解析ケースを解き、`strength`と`thermal`のcheckを返す。
pub fn evaluate(model: &Model) -> Result<Vec<Check>, String> {
    evaluate_with_fields(model).map(|(checks, _)| checks)
}

/// `evaluate`のcheckに加え、3つの格子で解けたケースの分布を返す。
pub fn evaluate_with_fields(model: &Model) -> Result<(Vec<Check>, Vec<Field>), String> {
    model.validate()?;
    let mut checks = Vec::new();
    let mut fields = Vec::new();
    for case in &model.analyses.structural {
        let part = case_part(model, &case.part, "")?;
        let material = case_material(model, part, "")?;
        let pitch = case_pitch(model, case.voxel_mm);
        // 領域は最も粗い格子の段を含む幅だけ広げ、3つの格子で同じ領域を使う。
        let reach = 2.0 * pitch;
        let layered = part
            .adopted_plan()
            .is_some_and(|plan| plan.process.layering().is_some());
        let stress_target = format!("{}/stress", case.id);
        let interlayer_target = format!("{}/interlayer", case.id);
        let deflection_target = format!("{}/deflection", case.id);
        let boundary_target = format!("{}/boundary_stress", case.id);
        // 厚さを測れない場合は細かくせず、一様な格子で解く。収束の判定は変わらない。
        let (refinement, skipped) = match Refinement::new(part, pitch) {
            Ok(refinement) => (refinement, String::new()),
            Err(reason) => (None, format!("; not refined locally: {reason}")),
        };
        // 拘束と荷重の近傍は、各所の最も粗い格子の2 cell分 (最も細かい格子の8 cell分) とする。
        let solved = solve_three(|level| {
            let scale = f64::from(1u32 << level);
            solve_elastic(
                model,
                part,
                case,
                material,
                scale * pitch,
                refinement.as_ref(),
                reach,
                8.0 / scale,
            )
        });
        let [fine, middle, coarse] = match solved {
            Ok(solutions) => solutions,
            Err(reason) => {
                // ケースが判定するはずの項目をすべて未評価として残す。
                let targets = [
                    Some(stress_target),
                    layered.then_some(interlayer_target),
                    case.max_deflection_mm.map(|_| deflection_target),
                    Some(boundary_target),
                ];
                for target in targets.into_iter().flatten() {
                    checks.push(analysis_check(
                        Rule::Strength,
                        target,
                        Status::NotEvaluated,
                        format!("not evaluated: {reason}"),
                    ));
                }
                continue;
            }
        };
        let layout = &fine.layout;
        let pitches = layout.pitches();
        let exclusion = (8.0 * pitches.0, 8.0 * pitches.1);
        let within = if exclusion.0 == exclusion.1 {
            range_label(exclusion, 1.0)
        } else {
            format!(
                "{} (8 cells of the local finest grid)",
                range_label(exclusion, 1.0)
            )
        };
        let labels = [
            fine.layout.label(),
            middle.layout.label(),
            coarse.layout.label(),
        ];
        let allowable = material.allowable_stress_mpa.expect("validated") / case.safety_factor;
        let surface = || fine.surface.iter();
        fields.push(with_exclusion(
            field(
                &case.id,
                Quantity::Stress,
                part,
                "MPa",
                Some(allowable),
                0.0,
                layout,
                surface().map(|s| (s.element, s.stress, s.excluded)),
            ),
            exclusion,
        ));
        if layered {
            let interlayer = material.interlayer_allowable_stress_mpa.expect("validated");
            fields.push(with_exclusion(
                field(
                    &case.id,
                    Quantity::Interlayer,
                    part,
                    "MPa",
                    Some(interlayer / case.safety_factor),
                    0.0,
                    layout,
                    surface().map(|s| (s.element, s.interlayer.max(0.0), s.excluded)),
                ),
                exclusion,
            ));
        }
        fields.push(field(
            &case.id,
            Quantity::Deflection,
            part,
            "mm",
            case.max_deflection_mm,
            0.0,
            layout,
            surface().map(|s| (s.element, s.deflection, false)),
        ));
        let context = format!(
            "part {}; safety factor {}; {} CG iterations; stresses at element corners, excluding cells within {within} of supports and loads{skipped}",
            part.id, case.safety_factor, fine.iterations
        );
        let solutions = [&fine, &middle, &coarse];
        // 近傍の外にcellが無ければ、応力を判定できない。
        let excluded_everywhere = solutions.iter().any(|s| s.stress.is_empty());
        let everything_near = |target: String| {
            analysis_check(
                Rule::Strength,
                target,
                Status::NotEvaluated,
                format!(
                    "not evaluated: every cell lies within {within} of supports and loads on some grid"
                ),
            )
        };
        checks.push(if excluded_everywhere {
            everything_near(stress_target)
        } else {
            limit_check(
                layout,
                &labels,
                Rule::Strength,
                stress_target,
                "von Mises stress",
                "MPa",
                solutions.map(|s| s.stress_mpa),
                0.0,
                allowable,
                &fine.stress,
                &context,
            )
        });
        if layered {
            let interlayer = material.interlayer_allowable_stress_mpa.expect("validated");
            checks.push(if excluded_everywhere {
                everything_near(interlayer_target)
            } else {
                limit_check(
                    layout,
                    &labels,
                    Rule::Strength,
                    interlayer_target,
                    "tension across layers",
                    "MPa",
                    solutions.map(|s| s.interlayer_mpa),
                    0.0,
                    interlayer / case.safety_factor,
                    &fine.interlayer,
                    &context,
                )
            });
        }
        if let Some(limit) = case.max_deflection_mm {
            checks.push(limit_check(
                layout,
                &labels,
                Rule::Strength,
                deflection_target,
                "deflection",
                "mm",
                solutions.map(|s| s.deflection_mm),
                0.0,
                limit,
                &fine.displaced,
                &context,
            ));
        }
        // 拘束と荷重の近傍は、理想化した境界条件による応力の特異点を含み、判定できない。
        let near = fine
            .boundary_mpa
            .map(|v| {
                format!(
                    "; maximum von Mises stress there {v:.4} MPa at grid {}",
                    labels[0]
                )
            })
            .unwrap_or_default();
        checks.push(analysis_check(
            Rule::Strength,
            boundary_target,
            Status::NotEvaluated,
            format!(
                "not evaluated: stresses within {within} of supports and loads depend on how the boundary conditions idealize contact{near}"
            ),
        ));
    }
    for case in &model.analyses.thermal {
        let part = case_part(model, &case.part, "")?;
        let material = case_material(model, part, "")?;
        let pitch = case_pitch(model, case.voxel_mm);
        let reach = 2.0 * pitch;
        let target = format!("{}/temperature", case.id);
        let solved = solve_three(|level| {
            let h = f64::from(1u32 << level) * pitch;
            solve_thermal(model, part, case, material, h, reach)
        });
        let [fine, middle, coarse] = match solved {
            Ok(solutions) => solutions,
            Err(reason) => {
                checks.push(analysis_check(
                    Rule::Thermal,
                    target,
                    Status::NotEvaluated,
                    format!("not evaluated: {reason}"),
                ));
                continue;
            }
        };
        // 周囲温度や固定温度からの上昇で収束を比べる。絶対温度では差が小さく見える。
        let reference = case
            .convection
            .iter()
            .map(|c| c.ambient_c)
            .chain(case.fixed_temperatures.iter().map(|t| t.celsius))
            .fold(f64::MAX, f64::min);
        let labels = [
            fine.layout.label(),
            middle.layout.label(),
            coarse.layout.label(),
        ];
        fields.push(field(
            &case.id,
            Quantity::Temperature,
            part,
            "°C",
            Some(case.max_temperature_c),
            reference,
            &fine.layout,
            fine.surface
                .iter()
                .map(|(element, value)| (*element, *value, false)),
        ));
        checks.push(limit_check(
            &fine.layout,
            &labels,
            Rule::Thermal,
            target,
            "maximum temperature",
            "°C",
            [fine.max_c, middle.max_c, coarse.max_c],
            reference,
            case.max_temperature_c,
            &fine.cells,
            &format!(
                "part {}; {} CG iterations; air inside is the given ambient",
                part.id, fine.iterations
            ),
        ));
    }
    Ok((checks, fields))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Assembly, Direction, Elastic, Feature, ManufacturingPlan, Operation, Orientation, Policy,
        Process, Role, Shape, Units,
    };

    const E_MPA: f64 = 1000.0;

    fn material() -> Material {
        Material {
            id: "resin".into(),
            name: "test resin".into(),
            source: "test".into(),
            elastic: Some(Elastic {
                youngs_modulus_mpa: E_MPA,
                poisson_ratio: 0.0,
            }),
            allowable_stress_mpa: Some(100.0),
            interlayer_allowable_stress_mpa: Some(50.0),
            thermal_conductivity_w_per_mk: Some(1.0),
            ..Material::default()
        }
    }

    fn molding() -> ManufacturingPlan {
        ManufacturingPlan {
            id: "molded".into(),
            material: None,
            process: Process::Molding {
                min_wall_mm: 0.5,
                max_wall_mm: 100.0,
            },
            orientation: Orientation::facing(Direction::PlusZ),
            source: "test".into(),
        }
    }

    fn fdm(up: Direction) -> ManufacturingPlan {
        ManufacturingPlan {
            id: "printed".into(),
            material: None,
            process: Process::Fdm {
                min_wall_mm: 0.5,
                overhang_angle_deg: 45.0,
                bridge_max_mm: 5.0,
            },
            orientation: Orientation::facing(up),
            source: "test".into(),
        }
    }

    /// 原点から+x方向へ伸びる角棒。
    fn bar(length: f64, width: f64, height: f64, plan: ManufacturingPlan) -> Model {
        Model {
            schema_version: crate::SCHEMA_VERSION,
            units: Units::Mm,
            parts: vec![Part {
                id: "bar".into(),
                features: vec![Feature {
                    id: "body".into(),
                    role: Role::Generic,
                    operation: Operation::Add,
                    shape: Shape::Box {
                        min: [0.0; 3],
                        max: [length, width, height],
                    },
                }],
                material: Some("resin".into()),
                adopted: plan.id.clone(),
                manufacturing: vec![plan],
            }],
            keepouts: vec![],
            sweeps: vec![],
            assembly: Assembly::default(),
            fasteners: vec![],
            materials: vec![material()],
            snap_fits: vec![],
            connectors: vec![],
            analyses: Analyses::default(),
            policy: Policy {
                min_feature_mm: 0.001,
                mesh_volume_tolerance: 0.01,
                voxel_mm: 0.5,
                min_neck_mm: 0.5,
                required: vec![],
            },
        }
    }

    /// x = `at`の端面を選ぶbox。
    fn end(at: f64) -> Region {
        Region::Box {
            min: [at - 0.1, -1.0, -1.0],
            max: [at + 0.1, 100.0, 100.0],
        }
    }

    fn cantilever(id: &str, length: f64, newtons: [f64; 3]) -> StructuralCase {
        StructuralCase {
            id: id.into(),
            part: "bar".into(),
            supports: vec![end(0.0)],
            forces: vec![Force {
                region: end(length),
                newtons,
            }],
            safety_factor: 1.0,
            max_deflection_mm: Some(10.0),
            voxel_mm: None,
        }
    }

    fn find<'a>(checks: &'a [Check], target: &str) -> &'a Check {
        checks
            .iter()
            .find(|c| c.target == target)
            .unwrap_or_else(|| panic!("no check {target}: {checks:?}"))
    }

    /// messageから、量の名前に続く最初の数値 (格子間隔hの値) を読む。
    fn value(check: &Check, quantity: &str) -> f64 {
        check
            .message
            .split(&format!("{quantity} "))
            .skip(1)
            .find_map(|rest| rest.split(' ').next()?.parse().ok())
            .unwrap_or_else(|| panic!("no value of {quantity} in {}", check.message))
    }

    #[test]
    fn tension_bar_matches_hookes_law() {
        // 長さ20 mm、断面6×6 mmの棒を360 Nで引く。σ = F/A = 10 MPa、δ = FL/(EA) = 0.2 mm。
        let mut model = bar(20.0, 6.0, 6.0, molding());
        model
            .analyses
            .structural
            .push(cantilever("pull", 20.0, [360.0, 0.0, 0.0]));
        let checks = evaluate(&model).unwrap();
        let stress = find(&checks, "pull/stress");
        assert_eq!(stress.status, Status::Pass, "{}", stress.message);
        let sigma = value(stress, "von Mises stress");
        assert!((sigma - 10.0).abs() < 0.01, "{}", stress.message);
        let deflection = find(&checks, "pull/deflection");
        assert_eq!(deflection.status, Status::Pass, "{}", deflection.message);
        let delta = value(deflection, "deflection");
        assert!((delta - 0.2).abs() < 1e-3, "{}", deflection.message);
        // 積層しない製造法では層間を評価しない。
        assert!(checks.iter().all(|c| c.target != "pull/interlayer"));
    }

    #[test]
    fn cantilever_matches_beam_theory() {
        // 長さ30 mm、断面6×6 mmの片持ち梁の先端に1 Nを掛ける。曲げのたわみFL³/(3EI)に
        // せん断のたわみFL/(κGA) (κ = 5/6、ν = 0でG = E/2) を加えた値と比べる。
        let mut model = bar(30.0, 6.0, 6.0, molding());
        model
            .analyses
            .structural
            .push(cantilever("bend", 30.0, [0.0, 0.0, -1.0]));
        let checks = evaluate(&model).unwrap();
        let deflection = find(&checks, "bend/deflection");
        assert_eq!(deflection.status, Status::Pass, "{}", deflection.message);
        let inertia = 6f64.powi(4) / 12.0;
        let expected =
            30f64.powi(3) / (3.0 * E_MPA * inertia) + 30.0 / (5.0 / 6.0 * E_MPA / 2.0 * 36.0);
        let delta = value(deflection, "deflection");
        assert!(
            (delta - expected).abs() / expected < 0.03,
            "expected {expected}: {}",
            deflection.message
        );
        // 拘束から4 mm (格子0.5 mmの8 cell分) までのcellを除くため、最大の曲げ応力は
        // x = 4.5 mmの断面の上下の面の値F(L - x)c/Iとなる。
        let stress = find(&checks, "bend/stress");
        assert_eq!(stress.status, Status::Pass, "{}", stress.message);
        let sigma = (30.0 - 4.5) * 3.0 / inertia;
        let root = value(stress, "von Mises stress");
        assert!(
            (root - sigma).abs() / sigma < 0.1,
            "expected {sigma}: {}",
            stress.message
        );
    }

    #[test]
    fn stress_over_the_allowable_fails_with_locations() {
        let mut model = bar(20.0, 6.0, 6.0, molding());
        model.materials[0].allowable_stress_mpa = Some(5.0);
        model
            .analyses
            .structural
            .push(cantilever("pull", 20.0, [360.0, 0.0, 0.0]));
        let checks = evaluate(&model).unwrap();
        let stress = find(&checks, "pull/stress");
        assert_eq!(stress.status, Status::Fail, "{}", stress.message);
        assert!(!stress.locations.is_empty());
    }

    #[test]
    fn interlayer_tension_follows_the_build_direction() {
        // 引張の向きに積層すると、層間に軸方向の応力10 MPaが掛かる。
        let mut model = bar(20.0, 6.0, 6.0, fdm(Direction::PlusX));
        model
            .analyses
            .structural
            .push(cantilever("pull", 20.0, [360.0, 0.0, 0.0]));
        let checks = evaluate(&model).unwrap();
        let along = value(find(&checks, "pull/interlayer"), "tension across layers");
        assert!((along - 10.0).abs() < 0.01, "{along}");

        // 引張と直交する向きに積層すると、層間の引張は生じない。
        model.parts[0].manufacturing = vec![fdm(Direction::PlusZ)];
        let checks = evaluate(&model).unwrap();
        let check = find(&checks, "pull/interlayer");
        assert_eq!(check.status, Status::Pass, "{}", check.message);
        let across = value(check, "tension across layers");
        assert!(across < 1e-6, "{across}");
    }

    #[test]
    fn fields_hold_surface_values_and_sample_mesh_vertices() {
        // 引張棒の表面の応力は一様に10 MPa、両端から4 mmの表面は判定しない近傍である。
        let mut model = bar(20.0, 6.0, 6.0, molding());
        model
            .analyses
            .structural
            .push(cantilever("pull", 20.0, [360.0, 0.0, 0.0]));
        let (_, fields) = evaluate_with_fields(&model).unwrap();
        let quantities: Vec<Quantity> = fields.iter().map(|f| f.quantity).collect();
        assert_eq!(quantities, [Quantity::Stress, Quantity::Deflection]);
        let stress = &fields[0];
        assert_eq!(stress.limit, Some(100.0));
        assert_eq!(stress.exclusion_mm, Some(4.0));
        assert_eq!((stress.pitch, stress.max_pitch), (0.5, None));
        assert_eq!(stress.cells.len(), stress.values.len());
        assert_eq!(stress.excluded.len(), stress.cells.len());
        assert!(stress.values.iter().all(|v| (v - 10.0).abs() < 1e-3));

        // 中央、拘束した端面、部品から離れた点。
        let (values, excluded) = sample(
            stress,
            &[[10.0, 0.0, 3.0], [0.0, 3.0, 3.0], [50.0, 3.0, 3.0]],
        );
        assert!((values[0] - 10.0).abs() < 1e-3);
        assert_eq!(excluded[..2], [false, true]);
        assert!(values[2].is_nan());

        // 隅のcellから1軸に1.9 pitch離れた点は値を持ち、3軸に1.9 pitch (約3.3 pitch) 離れた点は持たない。
        let corner = stress
            .cells
            .iter()
            .map(|cell| {
                std::array::from_fn::<f32, 3, _>(|axis| {
                    let centre = cell[axis] as f64 + cell[3] as f64 / 2.0;
                    (stress.origin[axis] + centre * stress.lattice) as f32
                })
            })
            .max_by(|a, b| a.iter().sum::<f32>().total_cmp(&b.iter().sum::<f32>()))
            .unwrap();
        let away = 1.9 * stress.pitch as f32;
        let (values, _) = sample(
            stress,
            &[
                [corner[0] + away, corner[1], corner[2]],
                corner.map(|v| v + away),
            ],
        );
        assert!(!values[0].is_nan() && values[1].is_nan(), "{values:?}");

        // 変位は拘束端で0、引く端で0.2 mm。
        let (values, _) = sample(&fields[1], &[[0.0, 3.0, 3.0], [20.0, 3.0, 3.0]]);
        assert!(
            values[0] < 0.02 && (values[1] - 0.2).abs() < 0.02,
            "{values:?}"
        );

        // 熱のケースは温度の分布を持ち、基準は固定温度。
        let (_, fields) = evaluate_with_fields(&rod(|case| {
            case.fixed_temperatures.push(FixedTemperature {
                region: end(20.0),
                celsius: 20.0,
            })
        }))
        .unwrap();
        assert_eq!(fields[0].quantity, Quantity::Temperature);
        assert_eq!(fields[0].reference, 20.0);
        assert!(fields[0].excluded.is_empty());
    }

    #[test]
    fn piece_without_support_or_heat_exit_is_not_evaluated() {
        // 棒から離れた箱は拘束も放熱も持たず、剛体変位と温度が定まらない。
        let mut model = bar(20.0, 6.0, 6.0, molding());
        let mut island = model.parts[0].features[0].clone();
        island.id = "island".into();
        island.shape = Shape::Box {
            min: [30.0, 0.0, 0.0],
            max: [36.0, 6.0, 6.0],
        };
        model.parts[0].features.push(island);
        model
            .analyses
            .structural
            .push(cantilever("pull", 20.0, [360.0, 0.0, 0.0]));
        model.analyses.thermal.push(ThermalCase {
            id: "heat".into(),
            part: "bar".into(),
            heat_inputs: vec![],
            fixed_temperatures: vec![FixedTemperature {
                region: end(0.0),
                celsius: 20.0,
            }],
            convection: vec![],
            max_temperature_c: 100.0,
            voxel_mm: None,
        });
        let checks = evaluate(&model).unwrap();
        for (target, reason) in [
            ("pull/stress", "touches no support"),
            ("pull/deflection", "touches no support"),
            (
                "heat/temperature",
                "touches no convection or fixed temperature",
            ),
        ] {
            let check = find(&checks, target);
            assert_eq!(check.status, Status::NotEvaluated, "{}", check.message);
            assert!(check.message.contains(reason), "{}", check.message);
        }
    }

    /// 縦の脚 (4 mm角、高さ20 mm) の上端から腕 (長さ20 mm、厚さ4 mm) が伸びるL字を`scale`倍した
    /// もの。脚の下端を拘束し、腕の先端を押す。`fillet`を与えると腕の付け根の内角を丸める。
    fn bracket(scale: f64, fillet: Option<f64>) -> Model {
        let mut model = bar(4.0 * scale, 6.0 * scale, 20.0 * scale, molding());
        let mut arm = model.parts[0].features[0].clone();
        arm.id = "arm".into();
        arm.shape = Shape::Box {
            min: [0.0, 0.0, 16.0 * scale],
            max: [20.0 * scale, 6.0 * scale, 20.0 * scale],
        };
        model.parts[0].features.push(arm);
        if let Some(radius) = fillet {
            let mut round = model.parts[0].features[0].clone();
            round.id = "corner".into();
            round.shape = Shape::Fillet {
                axis: crate::Axis::Y,
                edge: [4.0 * scale, 16.0 * scale],
                radius,
                span: [0.0, 6.0 * scale],
                toward: [Direction::PlusX, Direction::MinusZ],
            };
            model.parts[0].features.push(round);
        }
        model.analyses.structural.push(StructuralCase {
            id: "bracket".into(),
            part: "bar".into(),
            supports: vec![Region::Box {
                min: [-1.0, -1.0, -0.1],
                max: [4.0 * scale + 1.0, 6.0 * scale + 1.0, 0.1],
            }],
            forces: vec![Force {
                region: end(20.0 * scale),
                newtons: [0.0, 0.0, -1.0],
            }],
            safety_factor: 1.0,
            max_deflection_mm: Some(10.0),
            voxel_mm: None,
        });
        model
    }

    #[test]
    fn sharp_inner_corner_does_not_converge() {
        // L字の腕の付け根の内角は応力の特異点であり、細分するほど応力が増える。変位は収束する。
        let checks = evaluate(&bracket(1.0, None)).unwrap();
        let stress = find(&checks, "bracket/stress");
        assert_eq!(stress.status, Status::NotEvaluated, "{}", stress.message);
        let deflection = find(&checks, "bracket/deflection");
        assert_eq!(deflection.status, Status::Pass, "{}", deflection.message);
    }

    #[test]
    fn small_round_is_refined_locally() {
        // 厚さ12 mmのL字の丸み (半径3 mm) は、格子1 mmでは最も粗い格子 (4 mm) で1 cellに満たない。
        // 丸みの周りだけ0.5 mmに細かくすると、3つの格子で収束する。
        let mut model = bracket(3.0, Some(3.0));
        model.analyses.structural[0].voxel_mm = Some(1.0);
        let (checks, fields) = evaluate_with_fields(&model).unwrap();
        let stress = find(&checks, "bracket/stress");
        assert_eq!(stress.status, Status::Pass, "{}", stress.message);
        assert!(
            stress.message.contains("(grid 0.5–1 mm)") && stress.message.contains("(grid 2–4 mm)"),
            "{}",
            stress.message
        );
        let nominal = 6.0 * 1.0 * (60.0 - 12.0) / (18.0 * 12.0 * 12.0);
        assert!(
            value(stress, "von Mises stress") > nominal,
            "{}",
            stress.message
        );
        // 判定しない近傍の距離は、各所の最も細かい格子の8 cell分である。
        assert!(
            stress.message.contains("within 4–8 mm"),
            "{}",
            stress.message
        );
        assert_eq!((fields[0].pitch, fields[0].max_pitch), (0.5, Some(1.0)));
        assert_eq!(
            (fields[0].min_exclusion_mm, fields[0].exclusion_mm),
            (Some(4.0), Some(8.0))
        );
        // 細かくしたcellも分布の標本化に使う。丸みの面の点は値を持つ。
        let at = 12.0 + 3.0 * (1.0 - 0.5f64.sqrt());
        let (values, _) = sample(
            &fields[0],
            &[[at as f32, 9.0, (48.0 - 3.0 * (1.0 - 0.5f64.sqrt())) as f32]],
        );
        assert!(values[0] > 0.0, "{values:?}");
    }

    #[test]
    fn thin_plate_is_refined_to_match_beam_theory() {
        // 長さ15 mm、幅3 mm、厚さ1.5 mmの板の片持ち梁。格子0.5 mmでは最も粗い格子で厚さが1 cell
        // に満たないため、厚さから一辺0.125 mmに細かくする (最も粗い格子で厚さ3 cell)。
        let mut model = bar(15.0, 3.0, 1.5, molding());
        // 端面を選ぶboxは、端のcell (一辺0.125 mm) の側面の中心を含まない厚さとする。
        let face = |at: f64| Region::Box {
            min: [at - 0.01, -1.0, -1.0],
            max: [at + 0.01, 100.0, 100.0],
        };
        let mut case = cantilever("bend", 15.0, [0.0, 0.0, -0.01]);
        case.supports = vec![face(0.0)];
        case.forces[0].region = face(15.0);
        model.analyses.structural.push(case);
        let checks = evaluate(&model).unwrap();
        let deflection = find(&checks, "bend/deflection");
        assert_eq!(deflection.status, Status::Pass, "{}", deflection.message);
        assert!(
            deflection.message.contains("(grid 0.125 mm)"),
            "{}",
            deflection.message
        );
        let inertia = 3.0 * 1.5f64.powi(3) / 12.0;
        let expected = 0.01 * 15f64.powi(3) / (3.0 * E_MPA * inertia)
            + 0.01 * 15.0 / (5.0 / 6.0 * E_MPA / 2.0 * 4.5);
        let delta = value(deflection, "deflection");
        assert!(
            (delta - expected).abs() / expected < 0.03,
            "expected {expected}: {}",
            deflection.message
        );
        // 拘束から1 mm (0.125 mmの8 cell分) の外の、最大の曲げ応力F(L - x)c/I。
        let stress = find(&checks, "bend/stress");
        assert_eq!(stress.status, Status::Pass, "{}", stress.message);
        let sigma = 0.01 * (15.0 - 1.0) * 0.75 / inertia;
        let root = value(stress, "von Mises stress");
        assert!(
            (root - sigma).abs() / sigma < 0.1,
            "expected {sigma}: {}",
            stress.message
        );
    }

    #[test]
    fn refinement_over_the_cell_limit_is_not_solved() {
        let model = bar(15.0, 3.0, 1.5, molding());
        let part = &model.parts[0];
        let refinement = Refinement::new(part, 0.5).unwrap().expect("thin plate");
        let Err(reason) = mesh(part, 0.5, Some(&refinement), 1000) else {
            panic!("the mesh exceeds the limit");
        };
        assert!(reason.contains("above the limit of 1000"), "{reason}");
        // 細かくしない格子は上限に収まる。
        assert!(mesh(part, 0.5, None, 1000).is_ok());
    }

    #[test]
    fn rounded_inner_corner_converges() {
        // 部材の厚さ12 mmを格子1 mmで解く。最も粗い格子 (4 mm) でも厚さは3 cellある。鋭い内角の
        // 応力は細分で増え続けるが、半径6 mmで丸めると3つの格子で収束する。
        let at_one_millimetre = |mut model: Model| {
            model.analyses.structural[0].voxel_mm = Some(1.0);
            evaluate(&model).unwrap()
        };
        let sharp = at_one_millimetre(bracket(3.0, None));
        let stress = find(&sharp, "bracket/stress");
        assert_eq!(stress.status, Status::NotEvaluated, "{}", stress.message);

        let rounded = at_one_millimetre(bracket(3.0, Some(6.0)));
        let stress = find(&rounded, "bracket/stress");
        assert_eq!(stress.status, Status::Pass, "{}", stress.message);
        // 付け根の公称の曲げ応力6FL/(bt²)を下回らない。
        let nominal = 6.0 * 1.0 * (60.0 - 12.0) / (18.0 * 12.0 * 12.0);
        assert!(
            value(stress, "von Mises stress") > nominal,
            "{}",
            stress.message
        );
        let deflection = find(&rounded, "bracket/deflection");
        assert_eq!(deflection.status, Status::Pass, "{}", deflection.message);
    }

    #[test]
    fn support_outside_the_part_is_not_evaluated() {
        let mut model = bar(20.0, 6.0, 6.0, molding());
        let mut case = cantilever("pull", 20.0, [360.0, 0.0, 0.0]);
        case.supports = vec![Region::Box {
            min: [100.0; 3],
            max: [101.0; 3],
        }];
        model.analyses.structural.push(case);
        let checks = evaluate(&model).unwrap();
        let stress = find(&checks, "pull/stress");
        assert_eq!(stress.status, Status::NotEvaluated);
        assert!(stress.message.contains("covers no surface"));
    }

    fn rod(exit: impl FnOnce(&mut ThermalCase)) -> Model {
        let mut model = bar(20.0, 4.0, 4.0, molding());
        let mut case = ThermalCase {
            id: "heat".into(),
            part: "bar".into(),
            heat_inputs: vec![HeatInput {
                region: end(0.0),
                watts: 0.1,
            }],
            fixed_temperatures: vec![],
            convection: vec![],
            max_temperature_c: 200.0,
            voxel_mm: None,
        };
        exit(&mut case);
        model.analyses.thermal.push(case);
        model
    }

    #[test]
    fn rod_with_fixed_end_matches_one_dimensional_conduction() {
        // 側面は断熱。温度は長さ方向に線形で、双一次要素で厳密に表せる。T(0) = T(L) + QL/(kA) = 20 + 0.1·20/(1e-3·16) = 145 °C。
        let model = rod(|case| {
            case.fixed_temperatures.push(FixedTemperature {
                region: end(20.0),
                celsius: 20.0,
            })
        });
        let checks = evaluate(&model).unwrap();
        let check = find(&checks, "heat/temperature");
        assert_eq!(check.status, Status::Pass, "{}", check.message);
        let maximum = value(check, "maximum temperature");
        assert!((maximum - 145.0).abs() < 0.01, "{}", check.message);
    }

    #[test]
    fn rod_with_convective_end_adds_film_resistance() {
        // h = 1e4 W/(m²·K)の端面で、膜の抵抗による上昇Q/(hA) = 0.625 Kが加わる。
        let model = rod(|case| {
            case.convection.push(Convection {
                region: end(20.0),
                h_w_per_m2k: 1e4,
                ambient_c: 20.0,
            });
            case.max_temperature_c = 100.0;
        });
        let checks = evaluate(&model).unwrap();
        let check = find(&checks, "heat/temperature");
        assert_eq!(check.status, Status::Fail, "{}", check.message);
        assert!(!check.locations.is_empty());
        let maximum = value(check, "maximum temperature");
        assert!((maximum - 145.625).abs() < 0.01, "{}", check.message);
    }

    #[test]
    fn conflicting_fixed_temperatures_are_not_evaluated() {
        // 両端の領域が中央の断面で重なり、20 °Cと80 °Cが同じ節点に掛かる。順序で解が変わるため解かない。
        for order in [[20.0, 80.0], [80.0, 20.0]] {
            let model = rod(|case| {
                case.fixed_temperatures = vec![
                    FixedTemperature {
                        region: Region::Box {
                            min: [-1.0, -1.0, -1.0],
                            max: [10.0, 100.0, 100.0],
                        },
                        celsius: order[0],
                    },
                    FixedTemperature {
                        region: Region::Box {
                            min: [10.0, -1.0, -1.0],
                            max: [21.0, 100.0, 100.0],
                        },
                        celsius: order[1],
                    },
                ];
            });
            let checks = evaluate(&model).unwrap();
            let check = find(&checks, "heat/temperature");
            assert_eq!(check.status, Status::NotEvaluated, "{}", check.message);
            assert!(
                check.message.contains("meet at the same surface"),
                "{}",
                check.message
            );
        }
    }

    #[test]
    fn validation_requires_material_constants_and_known_references() {
        let base = || {
            let mut model = bar(20.0, 6.0, 6.0, molding());
            model
                .analyses
                .structural
                .push(cantilever("pull", 20.0, [1.0, 0.0, 0.0]));
            model
        };
        assert!(base().validate().is_ok());

        let mut model = base();
        model.materials[0].elastic = None;
        assert!(model.validate().unwrap_err().contains("needs elastic"));

        let mut model = base();
        model.parts[0].manufacturing = vec![fdm(Direction::PlusZ)];
        model.parts[0].adopted = "printed".into();
        model.materials[0].interlayer_allowable_stress_mpa = None;
        assert!(model.validate().unwrap_err().contains("interlayer"));

        let mut model = base();
        model.analyses.structural[0].supports = vec![Region::Fastener { id: "m3".into() }];
        assert!(model.validate().unwrap_err().contains("unknown fastener"));

        let mut model = base();
        model.analyses.structural[0].supports.clear();
        assert!(
            model
                .validate()
                .unwrap_err()
                .contains("at least one support")
        );

        let mut model = base();
        model.analyses.structural[0].part = "lid".into();
        assert!(model.validate().unwrap_err().contains("unknown part"));

        let mut model = base();
        let duplicate = model.analyses.structural[0].clone();
        model.analyses.structural.push(duplicate);
        assert!(model.validate().unwrap_err().contains("duplicate"));

        let mut model = rod(|_| {});
        assert!(
            model
                .validate()
                .unwrap_err()
                .contains("convection or fixed")
        );
        model.analyses.thermal[0].convection.push(Convection {
            region: Region::Surface,
            h_w_per_m2k: 10.0,
            ambient_c: 25.0,
        });
        model.materials[0].thermal_conductivity_w_per_mk = None;
        assert!(
            model
                .validate()
                .unwrap_err()
                .contains("thermal_conductivity")
        );
    }

    #[test]
    fn schema_12_input_cannot_declare_analyses() {
        let mut model = bar(20.0, 6.0, 6.0, molding());
        model.materials[0] = Material {
            id: "resin".into(),
            name: "test resin".into(),
            source: "test".into(),
            ..Material::default()
        };
        let mut value = serde_json::to_value(&model).unwrap();
        value["schema_version"] = 12.into();
        let upgraded = Model::from_json(&value.to_string()).unwrap();
        assert_eq!(upgraded.schema_version, crate::SCHEMA_VERSION);

        let mut with_cases = value.clone();
        with_cases["analyses"] = serde_json::json!({"structural": [], "thermal": []});
        assert!(Model::from_json(&with_cases.to_string()).is_err());

        let mut with_constants = value;
        with_constants["materials"][0]["thermal_conductivity_w_per_mk"] = 1.0.into();
        assert!(Model::from_json(&with_constants.to_string()).is_err());
    }
}
