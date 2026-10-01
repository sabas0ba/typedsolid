//! IRから直接rasterizeしたvoxel上で最終形状を評価する。
//!
//! backendのface/edge topologyに依存せず、boxとcylinderの内外判定だけで最終形状を
//! 得る。厚さ・接続部・空洞はいずれも同じoccupancyから導く。

use crate::{Check, Direction, Model, Operation, Part, Rule, Status};

/// 1 partあたりのcell数の上限。超える入力はvalidateで拒否する。
pub const MAX_GRID_CELLS: u64 = 200_000_000;
/// 外周を必ず空にするための余白。cell数で表す。
const PADDING_CELLS: u64 = 1;

/// rasterizeに必要なcell数。座標が有限であることは呼び出し前に検証されている。
pub fn grid_cells(part: &Part, pitch: f64) -> Option<u64> {
    let (low, high) = part.bounds()?;
    let mut cells: u64 = 1;
    for axis in 0..3 {
        let span = high[axis] - low[axis];
        if !span.is_finite() || span < 0.0 {
            return None;
        }
        let count = (span / pitch).ceil();
        if !count.is_finite() || count < 0.0 || count > u64::MAX as f64 {
            return None;
        }
        cells = cells.checked_mul(count as u64 + 1 + 2 * PADDING_CELLS)?;
    }
    Some(cells)
}

/// 軸平行の占有格子。cellはvoxel中心で内外を判定する。
pub struct Grid {
    size: [usize; 3],
    /// cell (0,0,0) の中心座標。
    origin: [f64; 3],
    pitch: f64,
    occupied: Vec<bool>,
}

impl Grid {
    fn index(&self, x: usize, y: usize, z: usize) -> usize {
        (z * self.size[1] + y) * self.size[0] + x
    }

    fn centre(&self, axis: usize, cell: usize) -> f64 {
        self.origin[axis] + cell as f64 * self.pitch
    }

    /// 座標範囲に重なるcellのindex範囲。範囲外は切り詰める。
    fn cell_range(&self, axis: usize, low: f64, high: f64) -> (usize, usize) {
        let first = ((low - self.origin[axis]) / self.pitch).floor();
        let last = ((high - self.origin[axis]) / self.pitch).ceil();
        let limit = self.size[axis] as f64 - 1.0;
        (
            first.clamp(0.0, limit) as usize,
            last.clamp(0.0, limit) as usize,
        )
    }

    pub fn size(&self) -> [usize; 3] {
        self.size
    }

    pub fn pitch(&self) -> f64 {
        self.pitch
    }

    pub fn occupied_cells(&self) -> usize {
        self.occupied.iter().filter(|cell| **cell).count()
    }

    /// 「すべてのAddの和から、すべてのCutを引く」をcell単位で構成する。
    pub fn rasterize(part: &Part, pitch: f64) -> Result<Self, String> {
        let (low, high) = part
            .bounds()
            .ok_or_else(|| format!("{} has no additive geometry", part.id))?;
        let cells =
            grid_cells(part, pitch).ok_or_else(|| format!("{} cannot be rasterized", part.id))?;
        if cells > MAX_GRID_CELLS {
            return Err(format!("{} exceeds the voxel grid limit", part.id));
        }
        let mut size = [0usize; 3];
        let mut origin = [0.0f64; 3];
        for axis in 0..3 {
            let count = ((high[axis] - low[axis]) / pitch).ceil() as u64 + 1 + 2 * PADDING_CELLS;
            size[axis] = count as usize;
            origin[axis] = low[axis] - PADDING_CELLS as f64 * pitch;
        }
        let mut grid = Self {
            size,
            origin,
            pitch,
            occupied: vec![false; cells as usize],
        };
        // featureのAABB内だけを走査する。全cellを毎回見ると無関係な領域まで数える。
        for (operation, fill) in [(Operation::Add, true), (Operation::Cut, false)] {
            for feature in part.features.iter().filter(|f| f.operation == operation) {
                let (shape_low, shape_high) = feature.shape.aabb();
                let ranges: Vec<(usize, usize)> = (0..3)
                    .map(|axis| grid.cell_range(axis, shape_low[axis], shape_high[axis]))
                    .collect();
                for z in ranges[2].0..=ranges[2].1 {
                    for y in ranges[1].0..=ranges[1].1 {
                        for x in ranges[0].0..=ranges[0].1 {
                            let point = [grid.centre(0, x), grid.centre(1, y), grid.centre(2, z)];
                            if feature.shape.contains(point) {
                                let index = grid.index(x, y, z);
                                grid.occupied[index] = fill;
                            }
                        }
                    }
                }
            }
        }
        Ok(grid)
    }

    /// 各cellから`seed`がtrueであるcellまでの距離の2乗。単位はcell。
    ///
    /// Felzenszwalb-Huttenlocherの下位包絡線法を3軸へ順に適用する。厳密な
    /// Euclidean距離であり、chamfer近似のような方向依存の誤差を持たない。
    fn distance_squared(&self, seed: &[bool]) -> Vec<f64> {
        let mut field: Vec<f64> = seed
            .iter()
            .map(|hit| if *hit { 0.0 } else { f64::INFINITY })
            .collect();
        let [nx, ny, nz] = self.size;
        let longest = nx.max(ny).max(nz);
        let mut line = vec![0.0f64; longest];
        let mut result = vec![0.0f64; longest];
        let mut vertices = vec![0usize; longest];
        let mut breaks = vec![0.0f64; longest + 1];

        for z in 0..nz {
            for y in 0..ny {
                for (x, slot) in line[..nx].iter_mut().enumerate() {
                    *slot = field[self.index(x, y, z)];
                }
                transform(&line[..nx], &mut result[..nx], &mut vertices, &mut breaks);
                for (x, value) in result[..nx].iter().enumerate() {
                    field[self.index(x, y, z)] = *value;
                }
            }
        }
        for z in 0..nz {
            for x in 0..nx {
                for (y, slot) in line[..ny].iter_mut().enumerate() {
                    *slot = field[self.index(x, y, z)];
                }
                transform(&line[..ny], &mut result[..ny], &mut vertices, &mut breaks);
                for (y, value) in result[..ny].iter().enumerate() {
                    field[self.index(x, y, z)] = *value;
                }
            }
        }
        for y in 0..ny {
            for x in 0..nx {
                for (z, slot) in line[..nz].iter_mut().enumerate() {
                    *slot = field[self.index(x, y, z)];
                }
                transform(&line[..nz], &mut result[..nz], &mut vertices, &mut breaks);
                for (z, value) in result[..nz].iter().enumerate() {
                    field[self.index(x, y, z)] = *value;
                }
            }
        }
        field
    }

    /// 半径`radius`のerosion。単位はcell。
    fn eroded(&self, radius: f64) -> Vec<bool> {
        let empty: Vec<bool> = self.occupied.iter().map(|cell| !cell).collect();
        let distance = self.distance_squared(&empty);
        let threshold = radius * radius;
        self.occupied
            .iter()
            .zip(distance.iter())
            .map(|(solid, squared)| *solid && *squared >= threshold)
            .collect()
    }

    /// 各solid cellを通る軸方向の連続長のうち最小のもの。単位はcell。
    ///
    /// 扱うprimitiveは軸平行のboxとcylinderに限るため、壁は必ずいずれかの軸に沿って
    /// 厚さを持つ。球のopeningで測ると角や稜線が必ず除去され、十分に厚い立体まで
    /// 薄肉と判定されるため採らない。
    fn axial_runs(&self) -> Vec<usize> {
        let mut runs = vec![usize::MAX; self.occupied.len()];
        let [nx, ny, nz] = self.size;
        for (axis, (length, outer, inner)) in [(nx, ny, nz), (ny, nx, nz), (nz, nx, ny)]
            .into_iter()
            .enumerate()
        {
            for a in 0..outer {
                for b in 0..inner {
                    let at = |k: usize| match axis {
                        0 => self.index(k, a, b),
                        1 => self.index(a, k, b),
                        _ => self.index(a, b, k),
                    };
                    let mut start: Option<usize> = None;
                    for k in 0..length {
                        if self.occupied[at(k)] {
                            start.get_or_insert(k);
                        } else if let Some(from) = start.take() {
                            for j in from..k {
                                let index = at(j);
                                runs[index] = runs[index].min(k - from);
                            }
                        }
                    }
                    // paddingにより末尾は空である。到達した場合も同じ扱いにする。
                    if let Some(from) = start {
                        for j in from..length {
                            let index = at(j);
                            runs[index] = runs[index].min(length - from);
                        }
                    }
                }
            }
        }
        runs
    }

    /// 6-連結の連結成分ごとのcell数。対角のみで接する部分は分かれていると扱う。
    fn component_sizes(&self, mask: &[bool]) -> Vec<usize> {
        let mut seen = vec![false; mask.len()];
        let mut stack = Vec::new();
        let mut sizes = Vec::new();
        let [nx, ny, nz] = self.size;
        for start in 0..mask.len() {
            if !mask[start] || seen[start] {
                continue;
            }
            let mut size = 0usize;
            seen[start] = true;
            stack.push(start);
            while let Some(index) = stack.pop() {
                size += 1;
                let x = index % nx;
                let y = (index / nx) % ny;
                let z = index / (nx * ny);
                for (dx, dy, dz) in NEIGHBOURS {
                    let (Some(nx_), Some(ny_), Some(nz_)) = (
                        checked_step(x, dx, nx),
                        checked_step(y, dy, ny),
                        checked_step(z, dz, nz),
                    ) else {
                        continue;
                    };
                    let neighbour = self.index(nx_, ny_, nz_);
                    if mask[neighbour] && !seen[neighbour] {
                        seen[neighbour] = true;
                        stack.push(neighbour);
                    }
                }
            }
            sizes.push(size);
        }
        sizes
    }

    /// 外周から到達できない空cellの数。padding により外周は必ず空である。
    fn enclosed_void_cells(&self) -> usize {
        let mut reached = vec![false; self.occupied.len()];
        let mut stack = Vec::new();
        let [nx, ny, nz] = self.size;
        for z in 0..nz {
            for y in 0..ny {
                for x in 0..nx {
                    let on_boundary =
                        x == 0 || y == 0 || z == 0 || x + 1 == nx || y + 1 == ny || z + 1 == nz;
                    let index = self.index(x, y, z);
                    if on_boundary && !self.occupied[index] && !reached[index] {
                        reached[index] = true;
                        stack.push(index);
                    }
                }
            }
        }
        while let Some(index) = stack.pop() {
            let x = index % nx;
            let y = (index / nx) % ny;
            let z = index / (nx * ny);
            for (dx, dy, dz) in NEIGHBOURS {
                let (Some(nx_), Some(ny_), Some(nz_)) = (
                    checked_step(x, dx, nx),
                    checked_step(y, dy, ny),
                    checked_step(z, dz, nz),
                ) else {
                    continue;
                };
                let neighbour = self.index(nx_, ny_, nz_);
                if !self.occupied[neighbour] && !reached[neighbour] {
                    reached[neighbour] = true;
                    stack.push(neighbour);
                }
            }
        }
        self.occupied
            .iter()
            .zip(reached.iter())
            .filter(|(solid, seen)| !**solid && !**seen)
            .count()
    }
}

/// 積層方向を基準にした座標系。layer 0がbuild plate側の層である。
struct Layered {
    up: usize,
    /// 層をgridの正方向へ積むか。build_directionが負向きなら層は逆順に進む。
    positive: bool,
    plane: [usize; 2],
}

impl Layered {
    fn new(direction: Direction) -> Self {
        let up = direction.axis().index();
        Self {
            up,
            positive: direction.is_positive(),
            plane: direction.axis().plane(),
        }
    }

    fn cell(&self, grid: &Grid, layer: usize, a: usize, b: usize) -> usize {
        let mut coordinate = [0usize; 3];
        coordinate[self.up] = if self.positive {
            layer
        } else {
            grid.size[self.up] - 1 - layer
        };
        coordinate[self.plane[0]] = a;
        coordinate[self.plane[1]] = b;
        grid.index(coordinate[0], coordinate[1], coordinate[2])
    }
}

/// 支持の有無と、bridgeで渡せる区間の判定結果。
struct Support {
    /// 直下に材料があるcell。
    supported: Vec<bool>,
    /// 支持は無いが、両端を支持されたbridgeで渡せるcell。
    bridged: Vec<bool>,
}

impl Grid {
    /// 層ごとに直下の材料を探し、支持の有無を求める。
    ///
    /// 直下が支持されているかは問わない。支持の要否は層ごとに独立して評価し、
    /// 1箇所のoverhangがその上の全体を未支持にすることを避ける。
    fn support(
        &self,
        direction: Direction,
        overhang_angle_deg: f64,
        bridge_max_mm: f64,
    ) -> Support {
        let frame = Layered::new(direction);
        let layers = self.size[frame.up];
        let extent = [self.size[frame.plane[0]], self.size[frame.plane[1]]];
        // 傾斜角tanが、1層あたりに許す水平方向のずれになる。tan(45°)は浮動小数点で
        // 1をわずかに下回るため、切り捨てではなく許容を持たせて比較する。
        let radius = overhang_angle_deg.to_radians().tan();
        let limit = radius * radius + 1e-9;
        let reach = radius.ceil() as isize;
        let offsets: Vec<(isize, isize)> = (-reach..=reach)
            .flat_map(|da| (-reach..=reach).map(move |db| (da, db)))
            .filter(|(da, db)| ((da * da + db * db) as f64) <= limit)
            .collect();

        let mut supported = vec![false; self.occupied.len()];
        // gridは外周にpaddingを持つため、最初の層は空である。材料が最初に現れる層が
        // build plateに接する。
        let mut material_below = false;
        for layer in 0..layers {
            let mut material_here = false;
            for a in 0..extent[0] {
                for b in 0..extent[1] {
                    let index = frame.cell(self, layer, a, b);
                    if !self.occupied[index] {
                        continue;
                    }
                    material_here = true;
                    supported[index] = !material_below
                        || offsets.iter().any(|(da, db)| {
                            let (Some(na), Some(nb)) = (
                                checked_step(a, *da as i32, extent[0]),
                                checked_step(b, *db as i32, extent[1]),
                            ) else {
                                return false;
                            };
                            self.occupied[frame.cell(self, layer - 1, na, nb)]
                        });
                }
            }
            material_below |= material_here;
        }

        // 未支持の区間は、同じ層で両端を支持された材料に挟まれていれば渡せる。
        let limit = (bridge_max_mm / self.pitch).floor() as usize;
        let mut bridged = vec![false; self.occupied.len()];
        for layer in 0..layers {
            for (axis, (length, other)) in [(extent[0], extent[1]), (extent[1], extent[0])]
                .into_iter()
                .enumerate()
            {
                for fixed in 0..other {
                    let at = |k: usize| {
                        if axis == 0 {
                            frame.cell(self, layer, k, fixed)
                        } else {
                            frame.cell(self, layer, fixed, k)
                        }
                    };
                    let mut start: Option<usize> = None;
                    for k in 0..length {
                        let index = at(k);
                        let open = self.occupied[index] && !supported[index];
                        if open {
                            start.get_or_insert(k);
                            continue;
                        }
                        if let Some(from) = start.take() {
                            let anchored_before = from > 0 && supported[at(from - 1)];
                            let anchored_after = supported[index];
                            if anchored_before && anchored_after && k - from <= limit {
                                for j in from..k {
                                    bridged[at(j)] = true;
                                }
                            }
                        }
                    }
                    // 端で終わる区間は片持ちであり、bridgeにならない。
                }
            }
        }
        Support { supported, bridged }
    }
}

const NEIGHBOURS: [(i32, i32, i32); 6] = [
    (-1, 0, 0),
    (1, 0, 0),
    (0, -1, 0),
    (0, 1, 0),
    (0, 0, -1),
    (0, 0, 1),
];

fn checked_step(value: usize, delta: i32, limit: usize) -> Option<usize> {
    let moved = value as i64 + delta as i64;
    if moved < 0 || moved >= limit as i64 {
        return None;
    }
    Some(moved as usize)
}

/// 1次元の距離変換。`f`は各位置の初期コスト、`d`に距離の2乗を書く。
fn transform(f: &[f64], d: &mut [f64], vertices: &mut [usize], breaks: &mut [f64]) {
    let n = f.len();
    if n == 0 {
        return;
    }
    let mut k = 0usize;
    vertices[0] = 0;
    breaks[0] = f64::NEG_INFINITY;
    breaks[1] = f64::INFINITY;
    for q in 1..n {
        loop {
            let v = vertices[k];
            let numerator = (f[q] + (q * q) as f64) - (f[v] + (v * v) as f64);
            let denominator = 2.0 * q as f64 - 2.0 * v as f64;
            let intersection = numerator / denominator;
            // f[q]とf[v]がともに無限大のとき交点は定まらない。より右の放物線を採る。
            if intersection <= breaks[k] && !intersection.is_nan() {
                if k == 0 {
                    vertices[0] = q;
                    breaks[0] = f64::NEG_INFINITY;
                    breaks[1] = f64::INFINITY;
                    break;
                }
                k -= 1;
                continue;
            }
            k += 1;
            vertices[k] = q;
            breaks[k] = if intersection.is_nan() {
                f64::NEG_INFINITY
            } else {
                intersection
            };
            breaks[k + 1] = f64::INFINITY;
            break;
        }
    }
    k = 0;
    for (q, slot) in d.iter_mut().enumerate().take(n) {
        while breaks[k + 1] < q as f64 {
            k += 1;
        }
        let v = vertices[k];
        let offset = q as f64 - v as f64;
        *slot = offset * offset + f[v];
    }
}

fn check(rule: Rule, target: &str, passed: bool, message: String) -> Check {
    Check {
        rule,
        status: if passed { Status::Pass } else { Status::Fail },
        target: target.into(),
        message,
    }
}

/// 最終形状に対する肉厚・接続部・空洞の検査。
///
/// 量子化の誤差は最大でgrid間隔1つ分である。要求値に間隔を加えた厚さを満たす場合のみ
/// passとし、判定を失敗側へ倒す。
pub fn evaluate(model: &Model) -> Result<Vec<Check>, String> {
    let policy = &model.policy;
    let pitch = policy.voxel_mm;
    let mut checks = Vec::new();
    for part in &model.parts {
        let grid = Grid::rasterize(part, pitch)?;
        let solid_cells = grid.occupied_cells();
        if solid_cells == 0 {
            // 空形状はvalid_solidが扱う。ここでは判定材料が無いことを明示する。
            for rule in Rule::VOXEL {
                checks.push(check(
                    rule,
                    &part.id,
                    false,
                    "rasterized geometry is empty".into(),
                ));
            }
            continue;
        }

        // 量子化の誤差を失敗側へ倒すため、要求値にgrid間隔を足した長さを求める。
        let required_run = ((policy.min_wall_mm + pitch) / pitch).ceil() as usize;
        let runs = grid.axial_runs();
        let thin: Vec<bool> = grid
            .occupied
            .iter()
            .zip(runs.iter())
            .map(|(solid, run)| *solid && *run < required_run)
            .collect();
        // 面の縁や稜線は必ず薄くなる。一辺min_wall_mmの立方体に満たない領域は
        // 形状の縁であり、壁の薄さを示さないため数えない。
        let minimum_cells = (policy.min_wall_mm.powi(3) / pitch.powi(3)).ceil() as usize;
        let regions: Vec<usize> = grid
            .component_sizes(&thin)
            .into_iter()
            .filter(|size| *size >= minimum_cells)
            .collect();
        let largest = regions.iter().copied().max().unwrap_or(0);
        checks.push(check(
            Rule::FinalWallThickness,
            &part.id,
            regions.is_empty(),
            format!(
                "{} region(s) thinner than {} mm; largest {:.3} mm³ of {:.3} mm³; grid {} mm; regions below {} mm³ are treated as edges",
                regions.len(),
                policy.min_wall_mm,
                largest as f64 * pitch.powi(3),
                solid_cells as f64 * pitch.powi(3),
                pitch,
                policy.min_wall_mm.powi(3)
            ),
        ));

        let neck_radius = (policy.min_neck_mm + pitch) / 2.0 / pitch;
        let core = grid.eroded(neck_radius);
        let core_cells = core.iter().filter(|cell| **cell).count();
        let components = grid.component_sizes(&core).len();
        checks.push(check(
            Rule::NeckSection,
            &part.id,
            components == 1,
            if core_cells == 0 {
                format!(
                    "no material survives a {} mm section; grid {} mm",
                    policy.min_neck_mm, pitch
                )
            } else {
                format!(
                    "{components} component(s) remain after eroding to a {} mm section; grid {} mm",
                    policy.min_neck_mm, pitch
                )
            },
        ));

        let enclosed = grid.enclosed_void_cells();
        checks.push(check(
            Rule::ClosedCavity,
            &part.id,
            enclosed == 0,
            format!(
                "{enclosed} enclosed void cell(s); grid {} mm; cavities reachable only through gaps below the grid size are not distinguished",
                pitch
            ),
        ));

        let support = grid.support(
            policy.build_direction,
            policy.overhang_angle_deg,
            policy.bridge_max_mm,
        );
        let unsupported = grid
            .occupied
            .iter()
            .zip(support.supported.iter())
            .zip(support.bridged.iter())
            .filter(|((solid, held), spanned)| **solid && !**held && !**spanned)
            .count();
        let spanned = support.bridged.iter().filter(|cell| **cell).count();
        checks.push(check(
            Rule::SupportFree,
            &part.id,
            unsupported == 0,
            format!(
                "{:.3} mm³ unsupported beyond {}° from {}; {:.3} mm³ carried by bridges up to {} mm; grid {} mm; slicer settings are not modelled",
                unsupported as f64 * pitch.powi(3),
                policy.overhang_angle_deg,
                policy.build_direction,
                spanned as f64 * pitch.powi(3),
                policy.bridge_max_mm,
                pitch
            ),
        ));
    }
    Ok(checks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Feature, Policy, Role, Shape, Units};

    fn part(features: Vec<Feature>) -> Part {
        Part {
            id: "block".into(),
            features,
            material: None,
        }
    }

    fn add(id: &str, shape: Shape) -> Feature {
        Feature {
            id: id.into(),
            role: Role::Generic,
            operation: Operation::Add,
            shape,
        }
    }

    fn cut(id: &str, shape: Shape) -> Feature {
        Feature {
            id: id.into(),
            role: Role::Generic,
            operation: Operation::Cut,
            shape,
        }
    }

    fn box_shape(min: [f64; 3], max: [f64; 3]) -> Shape {
        Shape::Box { min, max }
    }

    fn model_with(part: Part, policy: Policy) -> Model {
        Model {
            schema_version: crate::SCHEMA_VERSION,
            units: Units::Mm,
            parts: vec![part],
            keepouts: vec![],
            sweeps: vec![],
            assembly: crate::Assembly::default(),
            fasteners: vec![],
            materials: vec![],
            snap_fits: vec![],
            connectors: vec![],
            policy,
        }
    }

    fn policy(min_wall: f64, min_neck: f64, pitch: f64) -> Policy {
        Policy {
            min_feature_mm: 0.001,
            mesh_volume_tolerance: 0.01,
            voxel_mm: pitch,
            min_wall_mm: min_wall,
            min_neck_mm: min_neck,
            build_direction: Direction::PlusZ,
            overhang_angle_deg: 45.0,
            bridge_max_mm: 5.0,
            required: vec![],
        }
    }

    fn status(checks: &[Check], rule: Rule) -> Status {
        checks.iter().find(|c| c.rule == rule).unwrap().status
    }

    #[test]
    fn solid_block_passes_every_voxel_rule() {
        let block = part(vec![add("body", box_shape([0.; 3], [10., 10., 10.]))]);
        let checks = evaluate(&model_with(block, policy(2.0, 2.0, 0.5))).unwrap();
        for rule in Rule::VOXEL {
            assert_eq!(status(&checks, rule), Status::Pass, "{rule:?}");
        }
    }

    #[test]
    fn thin_plate_fails_wall_thickness() {
        // 厚さ0.8 mmの板に1.2 mmを要求する。
        let plate = part(vec![add("body", box_shape([0.; 3], [10., 10., 0.8]))]);
        let checks = evaluate(&model_with(plate, policy(1.2, 0.1, 0.2))).unwrap();
        assert_eq!(status(&checks, Rule::FinalWallThickness), Status::Fail);
    }

    #[test]
    fn thickness_is_measured_after_cuts() {
        // 10 mm角から内側を削り、残る壁を0.6 mmにする。primitiveはどれも厚い。
        let shell = part(vec![
            add("body", box_shape([0.; 3], [10., 10., 10.])),
            cut("pocket", box_shape([0.6, 0.6, 0.6], [9.4, 9.4, 11.])),
        ]);
        let checks = evaluate(&model_with(shell, policy(1.2, 0.1, 0.2))).unwrap();
        assert_eq!(status(&checks, Rule::FinalWallThickness), Status::Fail);
    }

    #[test]
    fn narrow_neck_fails_section() {
        // 2つの塊を1 mm角の首で繋ぐ。2 mmの断面を要求すると首で分かれる。
        let dumbbell = part(vec![
            add("left", box_shape([0., 0., 0.], [5., 5., 5.])),
            add("neck", box_shape([5., 2., 2.], [7., 3., 3.])),
            add("right", box_shape([7., 0., 0.], [12., 5., 5.])),
        ]);
        let checks = evaluate(&model_with(dumbbell, policy(0.5, 2.0, 0.25))).unwrap();
        assert_eq!(status(&checks, Rule::NeckSection), Status::Fail);
    }

    #[test]
    fn thick_neck_passes_section() {
        let bar = part(vec![
            add("left", box_shape([0., 0., 0.], [5., 5., 5.])),
            add("neck", box_shape([5., 0., 0.], [7., 5., 5.])),
            add("right", box_shape([7., 0., 0.], [12., 5., 5.])),
        ]);
        let checks = evaluate(&model_with(bar, policy(0.5, 2.0, 0.25))).unwrap();
        assert_eq!(status(&checks, Rule::NeckSection), Status::Pass);
    }

    #[test]
    fn sealed_pocket_fails_closed_cavity() {
        let sealed = part(vec![
            add("body", box_shape([0.; 3], [10., 10., 10.])),
            cut("void", box_shape([3., 3., 3.], [7., 7., 7.])),
        ]);
        let checks = evaluate(&model_with(sealed, policy(0.5, 0.5, 0.5))).unwrap();
        assert_eq!(status(&checks, Rule::ClosedCavity), Status::Fail);
    }

    #[test]
    fn open_pocket_passes_closed_cavity() {
        let open = part(vec![
            add("body", box_shape([0.; 3], [10., 10., 10.])),
            cut("void", box_shape([3., 3., 3.], [7., 7., 11.])),
        ]);
        let checks = evaluate(&model_with(open, policy(0.5, 0.5, 0.5))).unwrap();
        assert_eq!(status(&checks, Rule::ClosedCavity), Status::Pass);
    }

    #[test]
    fn cylinder_rasterizes_to_the_analytic_volume() {
        let post = part(vec![add(
            "stem",
            Shape::Cylinder {
                axis: crate::Axis::Z,
                center: [0., 0.],
                radius: 4.,
                span: [0., 8.],
            },
        )]);
        let pitch = 0.1;
        let grid = Grid::rasterize(&post, pitch).unwrap();
        let volume = grid.occupied_cells() as f64 * pitch.powi(3);
        let expected = std::f64::consts::PI * 16.0 * 8.0;
        // voxel中心での内外判定は境界を半cellだけ外へ広げる。厚さ判定はこの分を
        // 要求値に足して補っている。
        assert!(
            (volume - expected).abs() / expected < 0.03,
            "{volume} vs {expected}"
        );
    }

    #[test]
    fn distance_transform_matches_the_euclidean_distance() {
        // 1点だけをseedにし、任意のcellまでの距離が座標差の2乗和に一致することを見る。
        let block = part(vec![add("body", box_shape([0.; 3], [2., 2., 2.]))]);
        let grid = Grid::rasterize(&block, 0.5).unwrap();
        let mut seed = vec![false; grid.occupied.len()];
        seed[grid.index(1, 1, 1)] = true;
        let field = grid.distance_squared(&seed);
        for (x, y, z) in [(1, 1, 1), (4, 1, 1), (1, 3, 2), (5, 5, 5)] {
            let dx = x as f64 - 1.0;
            let dy = y as f64 - 1.0;
            let dz = z as f64 - 1.0;
            assert_eq!(
                field[grid.index(x, y, z)],
                dx * dx + dy * dy + dz * dz,
                "({x},{y},{z})"
            );
        }
    }

    /// 柱の上に片側だけ張り出した天板を載せる。張り出しがoverhangになる。
    /// 四方へ張り出すと角が対角にずれ、45°では支持と判定されない。
    fn table(overhang_mm: f64) -> Part {
        part(vec![
            add("leg", box_shape([0., 0., 0.], [4., 4., 6.])),
            add("top", box_shape([0., 0., 6.], [4. + overhang_mm, 4., 8.])),
        ])
    }

    #[test]
    fn flat_bottom_is_supported() {
        let block = part(vec![add("body", box_shape([0.; 3], [10., 10., 10.]))]);
        let checks = evaluate(&model_with(block, policy(2.0, 2.0, 0.5))).unwrap();
        assert_eq!(status(&checks, Rule::SupportFree), Status::Pass);
    }

    #[test]
    fn long_overhang_needs_support() {
        // 8 mm張り出した天板は、45°則でも5 mmのbridgeでも渡せない。
        let checks = evaluate(&model_with(table(8.0), policy(1.0, 1.0, 0.5))).unwrap();
        assert_eq!(status(&checks, Rule::SupportFree), Status::Fail);
    }

    #[test]
    fn short_overhang_is_carried_by_a_bridge() {
        // 2 mmの張り出しは両端を柱に支えられ、5 mmのbridgeで渡せる。
        let bridged = part(vec![
            add("left", box_shape([0., 0., 0.], [4., 4., 6.])),
            add("right", box_shape([6., 0., 0.], [10., 4., 6.])),
            add("deck", box_shape([0., 0., 6.], [10., 4., 8.])),
        ]);
        let checks = evaluate(&model_with(bridged, policy(1.0, 1.0, 0.5))).unwrap();
        assert_eq!(status(&checks, Rule::SupportFree), Status::Pass);
    }

    #[test]
    fn a_wide_gap_is_not_a_bridge() {
        let spanning = part(vec![
            add("left", box_shape([0., 0., 0.], [4., 4., 6.])),
            add("right", box_shape([20., 0., 0.], [24., 4., 6.])),
            add("deck", box_shape([0., 0., 6.], [24., 4., 8.])),
        ]);
        let checks = evaluate(&model_with(spanning, policy(1.0, 1.0, 0.5))).unwrap();
        assert_eq!(status(&checks, Rule::SupportFree), Status::Fail);
    }

    #[test]
    fn build_direction_changes_what_is_unsupported() {
        // 片持ちの棚。+Zに積むと棚下が未支持になり、+Xに積むと積層方向が変わる。
        let shelf = part(vec![
            add("post", box_shape([0., 0., 0.], [4., 4., 20.])),
            add("shelf", box_shape([4., 0., 14.], [16., 4., 18.])),
        ]);
        let upright = evaluate(&model_with(shelf.clone(), policy(1.0, 1.0, 0.5))).unwrap();
        assert_eq!(status(&upright, Rule::SupportFree), Status::Fail);

        let mut sideways = policy(1.0, 1.0, 0.5);
        sideways.build_direction = Direction::PlusX;
        let laid = evaluate(&model_with(shelf, sideways)).unwrap();
        assert_eq!(status(&laid, Rule::SupportFree), Status::Pass);
    }

    #[test]
    fn a_steeper_angle_accepts_more_overhang() {
        // 0.5 mmの張り出しは0.5 mm格子で1 cell分ずれる。
        let stepped = table(0.5);
        let sloped = evaluate(&model_with(stepped.clone(), policy(0.5, 0.5, 0.5))).unwrap();
        // 45°は1層あたり1 cellのずれまでを支持とみなす。
        assert_eq!(status(&sloped, Rule::SupportFree), Status::Pass);

        let mut narrow = policy(0.5, 0.5, 0.5);
        narrow.overhang_angle_deg = 0.0;
        narrow.bridge_max_mm = 0.0;
        let vertical = evaluate(&model_with(stepped, narrow)).unwrap();
        // 真下にしか支持を認めないと、同じ張り出しが未支持になる。
        assert_eq!(status(&vertical, Rule::SupportFree), Status::Fail);
    }

    #[test]
    fn grid_limit_counts_padding() {
        let block = part(vec![add("body", box_shape([0.; 3], [10., 10., 10.]))]);
        // 10 mmを1 mm刻みで覆うと、両端のpaddingを含めて13 cellになる。
        assert_eq!(grid_cells(&block, 1.0), Some(13 * 13 * 13));
        assert_eq!(Grid::rasterize(&block, 1.0).unwrap().size(), [13, 13, 13]);
    }
}
