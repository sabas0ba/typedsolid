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

use crate::multigrid::{CORNERS, Hierarchy, Level, Matrices};
use crate::voxel::Grid;
use crate::{Anchor, Check, Material, Model, Part, Rule, Status, identifier};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

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

/// 格子の節点の番号付けと、材料cellの要素。
struct Mesh<'a> {
    grid: &'a Grid,
    /// 節点の格子の各軸の数。cellの数より1多い。
    nodes: [usize; 3],
    /// 材料cellごとの8節点の番号。
    elements: Vec<[usize; 8]>,
    /// 材料cellのcell番号。`elements`と同じ順。
    cells: Vec<usize>,
    /// 材料cellに接する節点。
    active: Vec<bool>,
}

impl<'a> Mesh<'a> {
    fn new(grid: &'a Grid) -> Self {
        let [nx, ny, nz] = grid.size;
        let nodes = [nx + 1, ny + 1, nz + 1];
        let node = |x: usize, y: usize, z: usize| (z * nodes[1] + y) * nodes[0] + x;
        let mut elements = Vec::new();
        let mut cells = Vec::new();
        let mut active = vec![false; nodes[0] * nodes[1] * nodes[2]];
        for z in 0..nz {
            for y in 0..ny {
                for x in 0..nx {
                    let index = grid.index(x, y, z);
                    if !grid.occupied[index] {
                        continue;
                    }
                    let element = CORNERS.map(|c| node(x + c[0], y + c[1], z + c[2]));
                    for n in element {
                        active[n] = true;
                    }
                    elements.push(element);
                    cells.push(index);
                }
            }
        }
        Self {
            grid,
            nodes,
            elements,
            cells,
            active,
        }
    }

    fn node_count(&self) -> usize {
        self.active.len()
    }

    fn cell_positions(&self) -> Vec<[usize; 3]> {
        self.cells.iter().map(|cell| self.cell_xyz(*cell)).collect()
    }

    fn cell_xyz(&self, index: usize) -> [usize; 3] {
        let [nx, ny, _] = self.grid.size;
        [index % nx, (index / nx) % ny, index / (nx * ny)]
    }

    fn occupied(&self, xyz: [isize; 3]) -> bool {
        let size = self.grid.size;
        (0..3).all(|axis| xyz[axis] >= 0 && (xyz[axis] as usize) < size[axis])
            && self.grid.occupied[self.grid.index(
                xyz[0] as usize,
                xyz[1] as usize,
                xyz[2] as usize,
            )]
    }

    /// cellの、空のcellに面する面。(cell番号, 軸, 正の側か)。
    fn exposed_faces(&self) -> Vec<(usize, usize, bool)> {
        let mut faces = Vec::new();
        for &cell in &self.cells {
            let xyz = self.cell_xyz(cell).map(|v| v as isize);
            for axis in 0..3 {
                for positive in [false, true] {
                    let mut next = xyz;
                    next[axis] += if positive { 1 } else { -1 };
                    if !self.occupied(next) {
                        faces.push((cell, axis, positive));
                    }
                }
            }
        }
        faces
    }

    /// 面の4節点。
    fn face_nodes(&self, cell: usize, axis: usize, positive: bool) -> [usize; 4] {
        let [x, y, z] = self.cell_xyz(cell);
        let side = usize::from(positive);
        let [a, b] = match axis {
            0 => [1, 2],
            1 => [0, 2],
            _ => [0, 1],
        };
        let mut out = [0usize; 4];
        for (k, slot) in out.iter_mut().enumerate() {
            let mut offset = [0usize; 3];
            offset[axis] = side;
            offset[a] = k & 1;
            offset[b] = (k >> 1) & 1;
            *slot =
                ((z + offset[2]) * self.nodes[1] + (y + offset[1])) * self.nodes[0] + x + offset[0];
        }
        out
    }

    /// 節点を共有する要素の連結成分のうち、`anchored`の節点を1つも含まないものがあるか。
    /// そのような成分は剛体変位や温度が定まらず、方程式が特異になる。
    fn floating(&self, anchored: impl Fn(usize) -> bool) -> bool {
        let mut parent: Vec<usize> = (0..self.node_count()).collect();
        fn root(parent: &mut [usize], mut node: usize) -> usize {
            while parent[node] != node {
                parent[node] = parent[parent[node]];
                node = parent[node];
            }
            node
        }
        for element in &self.elements {
            let first = root(&mut parent, element[0]);
            for node in &element[1..] {
                let other = root(&mut parent, *node);
                parent[other] = first;
            }
        }
        let mut held = vec![false; parent.len()];
        for node in (0..parent.len()).filter(|n| self.active[*n] && anchored(*n)) {
            let r = root(&mut parent, node);
            held[r] = true;
        }
        self.elements.iter().any(|element| {
            let r = root(&mut parent, element[0]);
            !held[r]
        })
    }
}

/// 領域に含まれる露出した面。keepout、コネクタ開口、ネジ固定の領域は`reach`だけ広げる。
/// 格子間隔hと2hの解で同じ領域を使うため、`reach`は格子に依らない値とする。
fn region_faces(
    mesh: &Mesh,
    model: &Model,
    part: &Part,
    region: &Region,
    faces: &[(usize, usize, bool)],
    reach: f64,
) -> Vec<(usize, usize, bool)> {
    let pitch = mesh.grid.pitch;
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
    faces
        .iter()
        .copied()
        .filter(|(cell, axis, positive)| {
            let mut point = mesh.grid.centre_of(*cell);
            point[*axis] += if *positive { 0.5 } else { -0.5 } * pitch;
            predicate(point)
        })
        .collect()
}

/// `mask`のcellから各軸に`cells`個以内のcell。軸ごとの区間の最大で求める。
fn dilate(grid: &Grid, mask: &[bool], cells: usize) -> Vec<bool> {
    let size = grid.size;
    let mut current = mask.to_vec();
    for axis in 0..3 {
        let stride = match axis {
            0 => 1,
            1 => size[0],
            _ => size[0] * size[1],
        };
        let mut next = vec![false; current.len()];
        for (index, slot) in next.iter_mut().enumerate() {
            let position = (index / stride) % size[axis];
            let low = position.saturating_sub(cells);
            let high = (position + cells).min(size[axis] - 1);
            let base = index - position * stride;
            *slot = (low..=high).any(|k| current[base + k * stride]);
        }
        current = next;
    }
    current
}

/// 領域の合計を1とした節点ごとの割合。面は等しい面積を持ち、各面の分を4節点へ等分する。
/// 一様な表面荷重 (熱流束) を双一次要素の節点へ配ったものに当たり、節点へ等分するのと
/// 違って縁と角の節点へ荷重が集中しない。
fn surface_shares(mesh: &Mesh, faces: &[(usize, usize, bool)]) -> Vec<(usize, f64)> {
    let share = 0.25 / faces.len() as f64;
    faces
        .iter()
        .flat_map(|(cell, axis, positive)| mesh.face_nodes(*cell, *axis, *positive))
        .map(|node| (node, share))
        .collect()
}

/// 1つの格子での線形弾性の解。
struct ElasticSolution {
    /// 節点の変位の大きさの最大。単位はmm。
    deflection_mm: f64,
    /// 要素の8隅のvon Mises相当応力の最大と、cellごとの値。単位はMPa。拘束と荷重の近傍の
    /// cellを除く。
    stress_mpa: f64,
    stress: Vec<(usize, f64)>,
    /// 要素の8隅の積層方向の垂直応力 (引張) の最大と、cellごとの値。積層しない製造法では空。
    /// 拘束と荷重の近傍のcellを除く。
    interlayer_mpa: f64,
    interlayer: Vec<(usize, f64)>,
    /// 拘束と荷重の近傍のcellのvon Mises相当応力の最大。近傍が無ければNone。
    boundary_mpa: Option<f64>,
    /// 変位の大きい節点を含むcell。
    displaced: Vec<(usize, f64)>,
    iterations: usize,
}

/// 解析できない理由。checkのmessageに書く。
type Unsolved = String;

fn solve_elastic(
    model: &Model,
    part: &Part,
    case: &StructuralCase,
    material: &Material,
    pitch: f64,
    reach: f64,
    exclusion: f64,
) -> Result<Result<ElasticSolution, Unsolved>, String> {
    let grid = match Grid::rasterize_midpoint(part, pitch) {
        Ok(grid) => grid,
        Err(reason) => return Ok(Err(format!("{reason} at grid {pitch} mm"))),
    };
    let mesh = Mesh::new(&grid);
    let elastic = material.elastic.as_ref().expect("validated");
    let element = unit_element(elastic.poisson_ratio);
    let unit = &element.stiffness;
    let scale = elastic.youngs_modulus_mpa * pitch;
    let dofs = 3 * mesh.node_count();
    let faces = mesh.exposed_faces();

    let mut fixed: Vec<bool> = (0..dofs).map(|dof| !mesh.active[dof / 3]).collect();
    // 拘束と荷重の面を持つcell。応力の判定から除く近傍の起点である。
    let mut boundary = vec![false; grid.occupied.len()];
    for region in &case.supports {
        let selected = region_faces(&mesh, model, part, region, &faces, reach);
        if selected.is_empty() {
            return Ok(Err(format!(
                "support {region:?} covers no surface at grid {pitch} mm"
            )));
        }
        for (cell, axis, positive) in selected {
            boundary[cell] = true;
            for node in mesh.face_nodes(cell, axis, positive) {
                for c in 0..3 {
                    fixed[3 * node + c] = true;
                }
            }
        }
    }
    let mut rhs = vec![0.0; dofs];
    for force in &case.forces {
        let selected = region_faces(&mesh, model, part, &force.region, &faces, reach);
        if selected.is_empty() {
            return Ok(Err(format!(
                "force region {:?} covers no surface at grid {pitch} mm",
                force.region
            )));
        }
        for (cell, _, _) in &selected {
            boundary[*cell] = true;
        }
        for (node, share) in surface_shares(&mesh, &selected) {
            for c in 0..3 {
                rhs[3 * node + c] += force.newtons[c] * share;
            }
        }
    }
    if mesh.floating(|node| fixed[3 * node]) {
        return Ok(Err(format!(
            "a connected piece of the part touches no support at grid {pitch} mm"
        )));
    }
    let mut displacement = vec![0.0; dofs];
    let level = Level::new(
        3,
        mesh.nodes,
        mesh.cell_positions(),
        Matrices::Uniform(unit.iter().flatten().map(|k| scale * k).collect()),
        Vec::new(),
        fixed,
    );
    let Some(iterations) = Hierarchy::new(level).solve(&rhs, &mut displacement) else {
        return Ok(Err(format!(
            "the solver did not converge at grid {pitch} mm"
        )));
    };

    let deflection = |node: usize| {
        (0..3)
            .map(|c| displacement[3 * node + c].powi(2))
            .sum::<f64>()
            .sqrt()
    };
    let deflection_mm = (0..mesh.node_count()).map(deflection).fold(0.0, f64::max);
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
    // 近傍は各軸に`exclusion`以内のcellとする。
    let near = dilate(
        &grid,
        &boundary,
        (exclusion / pitch + 1e-9).floor() as usize,
    );
    let mut boundary_mpa: Option<f64> = None;
    let mut stress = Vec::with_capacity(mesh.elements.len());
    let mut interlayer = Vec::new();
    let mut displaced = Vec::with_capacity(mesh.elements.len());
    for (nodes, cell) in mesh.elements.iter().zip(&mesh.cells) {
        displaced.push((
            *cell,
            nodes.iter().map(|n| deflection(*n)).fold(0.0, f64::max),
        ));
        let mut local = [0.0; 24];
        for (a, node) in nodes.iter().enumerate() {
            for c in 0..3 {
                local[3 * a + c] = displacement[3 * node + c];
            }
        }
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
                (nodal + bubble) / pitch
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
        if near[*cell] {
            boundary_mpa = Some(boundary_mpa.unwrap_or(0.0).max(mises_max));
            continue;
        }
        stress.push((*cell, mises_max));
        if build.is_some() {
            interlayer.push((*cell, normal_max));
        }
    }
    let maximum = |values: &[(usize, f64)]| values.iter().map(|(_, v)| *v).fold(0.0, f64::max);
    Ok(Ok(ElasticSolution {
        deflection_mm,
        stress_mpa: maximum(&stress),
        interlayer_mpa: maximum(&interlayer),
        boundary_mpa,
        stress,
        interlayer,
        displaced,
        iterations,
    }))
}

/// 1つの格子での定常熱伝導の解。
struct ThermalSolution {
    /// 節点の温度の最大。単位は°C。
    max_c: f64,
    /// cellの8節点の温度の最大。
    cells: Vec<(usize, f64)>,
    iterations: usize,
}

fn solve_thermal(
    model: &Model,
    part: &Part,
    case: &ThermalCase,
    material: &Material,
    pitch: f64,
    reach: f64,
) -> Result<Result<ThermalSolution, Unsolved>, String> {
    let grid = match Grid::rasterize_midpoint(part, pitch) {
        Ok(grid) => grid,
        Err(reason) => return Ok(Err(format!("{reason} at grid {pitch} mm"))),
    };
    let mesh = Mesh::new(&grid);
    let unit = unit_conduction();
    // 熱伝導率はW/(m·K)で与えられ、mmの格子ではW/(mm·K)とする。
    let conductivity = material.thermal_conductivity_w_per_mk.expect("validated") / 1000.0;
    let scale = conductivity * pitch;
    let n = mesh.node_count();
    let faces = mesh.exposed_faces();
    let mut fixed: Vec<bool> = (0..n).map(|node| !mesh.active[node]).collect();
    let mut temperature = vec![0.0; n];
    // 節点に固定した温度。異なる温度の領域が節点を共有すると、記述順で解が変わるため解かない。
    let mut assigned: Vec<Option<f64>> = vec![None; n];
    for item in &case.fixed_temperatures {
        let selected = region_faces(&mesh, model, part, &item.region, &faces, reach);
        if selected.is_empty() {
            return Ok(Err(format!(
                "fixed temperature region {:?} covers no surface at grid {pitch} mm",
                item.region
            )));
        }
        for (cell, axis, positive) in selected {
            for node in mesh.face_nodes(cell, axis, positive) {
                if let Some(other) = assigned[node]
                    && other != item.celsius
                {
                    return Ok(Err(format!(
                        "fixed temperatures {other} °C and {} °C meet at the same surface at grid {pitch} mm",
                        item.celsius
                    )));
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
        let selected = region_faces(&mesh, model, part, &item.region, &faces, reach);
        if selected.is_empty() {
            return Ok(Err(format!(
                "convection region {:?} covers no surface at grid {pitch} mm",
                item.region
            )));
        }
        // 熱伝達係数はW/(m²·K)で与えられ、mm²の面ではW/(mm²·K)とする。
        let conductance = item.h_w_per_m2k * 1e-6 * pitch * pitch / 4.0;
        for face in selected {
            if !claimed.insert(face) {
                continue;
            }
            for node in mesh.face_nodes(face.0, face.1, face.2) {
                film[node] += conductance;
                rhs[node] += conductance * item.ambient_c;
            }
        }
    }
    for input in &case.heat_inputs {
        let selected = region_faces(&mesh, model, part, &input.region, &faces, reach);
        if selected.is_empty() {
            return Ok(Err(format!(
                "heat input region {:?} covers no surface at grid {pitch} mm",
                input.region
            )));
        }
        for (node, share) in surface_shares(&mesh, &selected) {
            rhs[node] += input.watts * share;
        }
    }
    if mesh.floating(|node| fixed[node] || film[node] > 0.0) {
        return Ok(Err(format!(
            "a connected piece of the part touches no convection or fixed temperature at grid {pitch} mm"
        )));
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
    let level = Level::new(
        1,
        mesh.nodes,
        mesh.cell_positions(),
        Matrices::Uniform(unit.iter().flatten().map(|k| scale * k).collect()),
        film,
        fixed,
    );
    let Some(iterations) = Hierarchy::new(level).solve(&rhs, &mut temperature) else {
        return Ok(Err(format!(
            "the solver did not converge at grid {pitch} mm"
        )));
    };
    let cells: Vec<(usize, f64)> = mesh
        .elements
        .iter()
        .zip(&mesh.cells)
        .map(|(element, cell)| {
            (
                *cell,
                element
                    .iter()
                    .map(|node| temperature[*node])
                    .fold(f64::MIN, f64::max),
            )
        })
        .collect();
    let max_c = cells.iter().map(|(_, t)| *t).fold(f64::MIN, f64::max);
    Ok(Ok(ThermalSolution {
        max_c,
        cells,
        iterations,
    }))
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

/// 格子間隔h、2h、4hの値と上限からcheckを作る。判定値が上限を超えたcellを検出箇所とし、
/// cellの値も判定値と同じ比で外挿する。
#[allow(clippy::too_many_arguments)]
fn limit_check(
    grid: &Grid,
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
    let pitch = grid.pitch;
    let summary = format!(
        "{quantity} {fine:.4} {unit} (grid {pitch} mm), {middle:.4} {unit} (grid {} mm), {coarse:.4} {unit} (grid {} mm); change{measured} {:.1}%; extrapolated {:.4} {unit}; limit {limit:.4} {unit}; {context}",
        2.0 * pitch,
        4.0 * pitch,
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
        let mut mask = vec![false; grid.occupied.len()];
        for (cell, value) in cells {
            if reference + (value - reference) * ratio > limit {
                mask[*cell] = true;
            }
        }
        let (locations, total) = grid.locations(&mask, 0.0);
        check.message.push_str(&format!("; {total} location(s)"));
        check.locations = locations;
    }
    check
}

/// 格子間隔h、2h、4hで解く。いずれかが解けなければその理由を返す。
fn solve_three<T>(
    pitch: f64,
    mut solve: impl FnMut(f64) -> Result<Result<T, Unsolved>, String>,
) -> Result<Result<[T; 3], Unsolved>, String> {
    let fine = solve(pitch)?;
    let middle = solve(2.0 * pitch)?;
    let coarse = solve(4.0 * pitch)?;
    Ok(match (fine, middle, coarse) {
        (Ok(fine), Ok(middle), Ok(coarse)) => Ok([fine, middle, coarse]),
        (Err(reason), _, _) | (_, Err(reason), _) | (_, _, Err(reason)) => Err(reason),
    })
}

/// 解析の格子間隔。ケースが与えなければpolicyの値。
fn case_pitch(model: &Model, voxel_mm: Option<f64>) -> f64 {
    voxel_mm.unwrap_or(model.policy.voxel_mm)
}

/// 宣言した解析ケースを解き、`strength`と`thermal`のcheckを返す。
pub fn evaluate(model: &Model) -> Result<Vec<Check>, String> {
    model.validate()?;
    let mut checks = Vec::new();
    for case in &model.analyses.structural {
        let part = case_part(model, &case.part, "")?;
        let material = case_material(model, part, "")?;
        let pitch = case_pitch(model, case.voxel_mm);
        // 領域は最も粗い格子の段を含む幅だけ広げ、3つの格子で同じ領域を使う。
        let reach = 2.0 * pitch;
        // 拘束と荷重の近傍は、最も粗い格子の2 cell分とする。
        let exclusion = 8.0 * pitch;
        let layered = part
            .adopted_plan()
            .is_some_and(|plan| plan.process.layering().is_some());
        let stress_target = format!("{}/stress", case.id);
        let interlayer_target = format!("{}/interlayer", case.id);
        let deflection_target = format!("{}/deflection", case.id);
        let boundary_target = format!("{}/boundary_stress", case.id);
        let solved = solve_three(pitch, |h| {
            solve_elastic(model, part, case, material, h, reach, exclusion)
        })?;
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
        let grid = Grid::rasterize_midpoint(part, pitch)?;
        let context = format!(
            "part {}; safety factor {}; {} CG iterations; stresses at element corners, excluding cells within {exclusion} mm of supports and loads",
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
                    "not evaluated: every cell lies within {exclusion} mm of supports and loads on some grid"
                ),
            )
        };
        let allowable = material.allowable_stress_mpa.expect("validated") / case.safety_factor;
        checks.push(if excluded_everywhere {
            everything_near(stress_target)
        } else {
            limit_check(
                &grid,
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
                    &grid,
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
                &grid,
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
            .map(|v| format!("; maximum von Mises stress there {v:.4} MPa at grid {pitch} mm"))
            .unwrap_or_default();
        checks.push(analysis_check(
            Rule::Strength,
            boundary_target,
            Status::NotEvaluated,
            format!(
                "not evaluated: stresses within {exclusion} mm of supports and loads depend on how the boundary conditions idealize contact{near}"
            ),
        ));
    }
    for case in &model.analyses.thermal {
        let part = case_part(model, &case.part, "")?;
        let material = case_material(model, part, "")?;
        let pitch = case_pitch(model, case.voxel_mm);
        let reach = 2.0 * pitch;
        let target = format!("{}/temperature", case.id);
        let solved = solve_three(pitch, |h| {
            solve_thermal(model, part, case, material, h, reach)
        })?;
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
        let grid = Grid::rasterize_midpoint(part, pitch)?;
        checks.push(limit_check(
            &grid,
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
    Ok(checks)
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
