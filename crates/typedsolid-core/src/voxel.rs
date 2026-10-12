//! IRから直接rasterizeしたvoxel上で最終形状を評価する。
//!
//! backendのface/edge topologyに依存せず、boxとcylinderの内外判定だけで最終形状を
//! 得る。厚さ・接続部・空洞はいずれも同じoccupancyから導く。

use crate::{
    Axis, Check, Direction, Location, MAX_LOCATIONS, ManufacturingPlan, Model, Operation, Part,
    Policy, Process, Rule, Status, mesh,
};
use serde::{Deserialize, Serialize};

/// 1 partあたりのcell数の上限。超える入力はvalidateで拒否する。
pub const MAX_GRID_CELLS: u64 = 200_000_000;
/// 切削の判定で、工具を平面上で走査する作業量の上限。段取り1つあたりである。
pub const MAX_MILLING_WORK: u64 = 2_000_000_000;
/// 外周を必ず空にするための余白。cell数で表す。
const PADDING_CELLS: u64 = 1;

/// rasterizeに必要な各軸のcell数。座標が有限であることは呼び出し前に検証されている。
pub fn grid_size(part: &Part, pitch: f64) -> Option<[u64; 3]> {
    let (low, high) = part.bounds()?;
    let mut size = [0u64; 3];
    for axis in 0..3 {
        let span = high[axis] - low[axis];
        if !span.is_finite() || span < 0.0 {
            return None;
        }
        let count = (span / pitch).ceil();
        if !count.is_finite() || count < 0.0 || count > u64::MAX as f64 {
            return None;
        }
        size[axis] = (count as u64).checked_add(1 + 2 * PADDING_CELLS)?;
    }
    Some(size)
}

/// rasterizeに必要なcell数。
pub fn grid_cells(part: &Part, pitch: f64) -> Option<u64> {
    grid_size(part, pitch)?
        .into_iter()
        .try_fold(1u64, |cells, count| cells.checked_mul(count))
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

    /// 閉じた三角形meshの内部をcell単位で求める。
    ///
    /// 各(x, y) cell中心から+z方向の直線とmeshの交点を求め、winding numberが正の区間に
    /// あるcell中心を内部とする。外向きの法線が-z成分を持つ面を通ると+1 (入る)、
    /// +z成分を持つ面を通ると-1 (出る) とする。偶奇則と異なり、重なった複数のsolidは
    /// 和として、内向きの面で囲んだ空洞は空洞として扱える。xy平面へ投影した三角形の
    /// 内外判定は、辺上の点を辺の向きで一方の三角形だけに割り当てるため、隣り合う
    /// 三角形の共有辺で交点を二重に数えない。格子はmeshの外接boxから半cellずらし、
    /// 軸平行な面がcell中心を通らないようにする。winding numberが負になるか0に戻らない
    /// 列があれば、meshが閉じていないか向きが不整合であるとして拒否する。
    pub fn from_triangles(triangles: &[[[f64; 3]; 3]], pitch: f64) -> Result<Self, String> {
        if triangles.is_empty() {
            return Err("mesh has no triangles".into());
        }
        let mut low = [f64::INFINITY; 3];
        let mut high = [f64::NEG_INFINITY; 3];
        for vertex in triangles.iter().flatten() {
            for axis in 0..3 {
                if !vertex[axis].is_finite() || vertex[axis].abs() > crate::COORDINATE_LIMIT_MM {
                    return Err("mesh coordinates must be finite and within ±1000000 mm".into());
                }
                low[axis] = low[axis].min(vertex[axis]);
                high[axis] = high[axis].max(vertex[axis]);
            }
        }
        let mut size = [0usize; 3];
        let mut origin = [0.0f64; 3];
        let mut cells: u64 = 1;
        for axis in 0..3 {
            let count = ((high[axis] - low[axis]) / pitch).ceil() as u64 + 1 + 2 * PADDING_CELLS;
            cells = cells
                .checked_mul(count)
                .filter(|c| *c <= MAX_GRID_CELLS)
                .ok_or_else(|| {
                    format!(
                        "mesh exceeds the voxel grid limit of {MAX_GRID_CELLS} cells at voxel_mm={pitch}"
                    )
                })?;
            size[axis] = count as usize;
            origin[axis] = low[axis] - (PADDING_CELLS as f64 - 0.5) * pitch;
        }
        let mut grid = Self {
            size,
            origin,
            pitch,
            occupied: vec![false; cells as usize],
        };
        let [nx, ny, _] = size;
        // 交点のzとwinding numberの増分。
        let mut columns: Vec<Vec<(f64, i32)>> = vec![Vec::new(); nx * ny];
        for triangle in triangles {
            let [a, mut b, mut c] = *triangle;
            let mut area = cross(a, b, [c[0], c[1]]);
            if area == 0.0 {
                // xy平面へ投影すると線分になる三角形は+z方向の直線と交わらない。
                continue;
            }
            // 投影が反時計回りなら外向きの法線は+z成分を持ち、上へ抜けると外に出る。
            let delta = if area > 0.0 { -1 } else { 1 };
            if area < 0.0 {
                std::mem::swap(&mut b, &mut c);
                area = -area;
            }
            // 以降、頂点a, b, cはxy平面で反時計回りに並ぶ。
            let (x0, x1) = grid.cell_range(0, a[0].min(b[0]).min(c[0]), a[0].max(b[0]).max(c[0]));
            let (y0, y1) = grid.cell_range(1, a[1].min(b[1]).min(c[1]), a[1].max(b[1]).max(c[1]));
            for y in y0..=y1 {
                for x in x0..=x1 {
                    let point = [grid.centre(0, x), grid.centre(1, y)];
                    let weights = [
                        edge_weight(b, c, point),
                        edge_weight(c, a, point),
                        edge_weight(a, b, point),
                    ];
                    let edges = [(b, c), (c, a), (a, b)];
                    let inside = weights
                        .iter()
                        .zip(edges.iter())
                        .all(|(w, (u, v))| *w > 0.0 || (*w == 0.0 && owns_edge(*u, *v)));
                    if inside {
                        let z = (weights[0] * a[2] + weights[1] * b[2] + weights[2] * c[2]) / area;
                        columns[y * nx + x].push((z, delta));
                    }
                }
            }
        }
        let mut broken_columns = 0usize;
        for y in 0..ny {
            for x in 0..nx {
                let hits = &mut columns[y * nx + x];
                // 同じzの交点は入る側を先に数え、接する2つのsolidの境で外に出ないようにする。
                hits.sort_by(|p, q| p.0.total_cmp(&q.0).then(q.1.cmp(&p.1)));
                let mut winding = 0i32;
                let mut entered = 0.0f64;
                let mut broken = false;
                for &(z, delta) in hits.iter() {
                    let before = winding;
                    winding += delta;
                    if winding < 0 {
                        broken = true;
                        break;
                    }
                    if before == 0 && winding > 0 {
                        entered = z;
                    } else if before > 0 && winding == 0 {
                        // 中心がentered <= z < 出た位置にあるcellを内部とする。
                        let first = ((entered - grid.origin[2]) / pitch).ceil().max(0.0) as usize;
                        let last = ((z - grid.origin[2]) / pitch).ceil().max(0.0) as usize;
                        for cell in first..last.min(grid.size[2]) {
                            let index = grid.index(x, y, cell);
                            grid.occupied[index] = true;
                        }
                    }
                }
                if broken || winding != 0 {
                    broken_columns += 1;
                }
            }
        }
        if broken_columns > 0 {
            return Err(format!(
                "mesh is not closed or its orientation is inconsistent: winding number is negative or does not return to 0 in {broken_columns} column(s)"
            ));
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

    /// 外周から到達できない空cell。padding により外周は必ず空である。
    fn enclosed_void_mask(&self) -> Vec<bool> {
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
            .map(|(solid, seen)| !*solid && !*seen)
            .collect()
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

/// xy平面での(v - u)×(p - u)。uからvへの辺の左にpがあれば正。
fn cross(u: [f64; 3], v: [f64; 3], p: [f64; 2]) -> f64 {
    (v[0] - u[0]) * (p[1] - u[1]) - (v[1] - u[1]) * (p[0] - u[0])
}

/// 辺uvに対する点pの符号付き面積。端点を座標の辞書順に並べてから計算し、向きは符号で
/// 表す。共有辺を逆向きに持つ2つの三角形はビット単位で符号だけが異なる値を得るため、
/// 丸め誤差で両方が内側と判定することはない。
fn edge_weight(u: [f64; 3], v: [f64; 3], p: [f64; 2]) -> f64 {
    if (u[0], u[1]) <= (v[0], v[1]) {
        cross(u, v, p)
    } else {
        -cross(v, u, p)
    }
}

/// 辺上の点をこの辺を持つ三角形へ割り当てるか。隣り合う2つの反時計回りの三角形は
/// 共有辺を逆向きに持つため、ちょうど一方だけがtrueとなる。
fn owns_edge(u: [f64; 3], v: [f64; 3]) -> bool {
    let (dx, dy) = (v[0] - u[0], v[1] - u[1]);
    dy > 0.0 || (dy == 0.0 && dx < 0.0)
}

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
    planned(rule, target, None, true, passed, message)
}

/// 製造案に依るcheck。planは製造案のid、adoptedは採用した製造案か。
fn planned(
    rule: Rule,
    target: &str,
    plan: Option<&str>,
    adopted: bool,
    passed: bool,
    message: String,
) -> Check {
    Check {
        plan: plan.map(Into::into),
        adopted,
        locations: Vec::new(),
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
///
/// 製造案に依るrule (肉厚、支持、UV樹脂) は部品の製造案ごとに評価し、checkに製造案の
/// idと採用の有無を持たせる。形状だけで決まるrule (接続部、閉空洞) は部品ごとに1回である。
pub fn evaluate(model: &Model) -> Result<Vec<Check>, String> {
    let mut checks = Vec::new();
    for part in &model.parts {
        let grid = Grid::rasterize(part, model.policy.voxel_mm)?;
        for plan in &part.manufacturing {
            check_milling_work(grid.size, model.policy.voxel_mm, plan, &part.id)?;
        }
        let plans: Vec<(&ManufacturingPlan, bool)> = part
            .manufacturing
            .iter()
            .map(|plan| (plan, plan.id == part.adopted))
            .collect();
        checks.extend(evaluate_grid(&grid, &part.id, &model.policy, &plans));
    }
    Ok(checks)
}

/// 外部のSTL (binary又はASCII) を読み、IRを介さずに最終形状のruleを評価する。
/// 座標の単位はmmとみなす。meshは閉じている必要があり、閉じていなければ拒否する。
/// 製造案は1つで、採用したものとして扱う。
pub fn evaluate_stl(
    bytes: &[u8],
    target: &str,
    policy: &Policy,
    plan: &ManufacturingPlan,
) -> Result<Vec<Check>, String> {
    plan.validate()?;
    let grid = stl_grid(bytes, policy)?;
    check_milling_work(grid.size, policy.voxel_mm, plan, target)?;
    Ok(evaluate_grid(&grid, target, policy, &[(plan, true)]))
}

/// STLを読み、閉じていることを確かめてから格子にする。
fn stl_grid(bytes: &[u8], policy: &Policy) -> Result<Grid, String> {
    policy.validate()?;
    let parsed = mesh::parse_stl(bytes)?;
    // +z方向のwinding numberは、z方向から見て面積0の面の穴や裏返りを検出できない。
    let unpaired = mesh::unpaired_edges(&parsed);
    if unpaired > 0 {
        return Err(format!(
            "mesh is not closed or not consistently oriented: {unpaired} edge(s) are not paired with an opposite-direction edge"
        ));
    }
    let triangles: Vec<[[f64; 3]; 3]> = parsed.iter().map(mesh::Triangle::vertices).collect();
    Grid::from_triangles(&triangles, policy.voxel_mm)
}

/// 断面を描く平面。`axis`に垂直で、座標`coordinate`を通る。単位はmm。
/// `coordinate`を省くと格子の中央を通る。
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plane {
    pub axis: Axis,
    #[serde(default)]
    pub coordinate: Option<f64>,
}

/// 断面1枚のcell。
///
/// `rows[j][i]`は、平面内の2軸 (`Axis::plane`の順) でi番目とj番目のcellを表す。
/// 各文字はbitの和を32進 ("0"〜"9"、"a"〜"v") で書いたもので、bitは1が材料、
/// 2が薄肉、4が接続部、8が未支持、16が閉空洞である。`origin`はcell (0, 0) の中心、
/// `plane_coordinate`は実際に切ったcell中心の座標である。
#[derive(Debug, Clone, Serialize)]
pub struct Section {
    pub axis: Axis,
    pub plane_coordinate: f64,
    pub origin: [f64; 2],
    pub pitch: f64,
    pub rows: Vec<String>,
}

pub const SECTION_SOLID: u8 = 1;
pub const SECTION_THIN: u8 = 2;
pub const SECTION_NECK: u8 = 4;
pub const SECTION_UNSUPPORTED: u8 = 8;
pub const SECTION_VOID: u8 = 16;
/// 1回の呼び出しで求める断面の上限。
const MAX_SECTIONS: usize = 256;

impl Grid {
    /// 解析結果を断面ごとに切り出す。平面の座標は最も近いcell中心へ丸める。
    pub fn sections(&self, analysis: &Analysis, planes: &[Plane]) -> Result<Vec<Section>, String> {
        if planes.len() > MAX_SECTIONS {
            return Err(format!("at most {MAX_SECTIONS} sections are supported"));
        }
        const DIGITS: &[u8; 32] = b"0123456789abcdefghijklmnopqrstuv";
        planes
            .iter()
            .map(|plane| {
                let normal = plane.axis.index();
                let [first, second] = plane.axis.plane();
                let layer = match plane.coordinate {
                    None => self.size[normal] / 2,
                    Some(value) if value.is_finite() => {
                        let limit = self.size[normal] as f64 - 1.0;
                        ((value - self.origin[normal]) / self.pitch)
                            .round()
                            .clamp(0.0, limit) as usize
                    }
                    Some(_) => return Err("section coordinate must be finite".into()),
                };
                let rows = (0..self.size[second])
                    .map(|j| {
                        let bytes: Vec<u8> = (0..self.size[first])
                            .map(|i| {
                                let mut cell = [0usize; 3];
                                cell[normal] = layer;
                                cell[first] = i;
                                cell[second] = j;
                                let index = self.index(cell[0], cell[1], cell[2]);
                                let bits = [
                                    (self.occupied[index], SECTION_SOLID),
                                    (analysis.manufacture.thin[index], SECTION_THIN),
                                    (analysis.structure.neck[index], SECTION_NECK),
                                    (
                                        marked(&analysis.manufacture.unsupported, index),
                                        SECTION_UNSUPPORTED,
                                    ),
                                    (analysis.structure.void[index], SECTION_VOID),
                                ]
                                .into_iter()
                                .filter(|(set, _)| *set)
                                .fold(0u8, |acc, (_, bit)| acc | bit);
                                DIGITS[bits as usize]
                            })
                            .collect();
                        String::from_utf8(bytes).expect("digits are ASCII")
                    })
                    .collect();
                Ok(Section {
                    axis: plane.axis,
                    plane_coordinate: self.centre(normal, layer),
                    origin: [self.origin[first], self.origin[second]],
                    pitch: self.pitch,
                    rows,
                })
            })
            .collect()
    }
}

/// IRの1部品を格子にし、指定した製造案で判定した断面を返す。planを省くと採用した製造案とする。
pub fn sections_of_part(
    model: &Model,
    part_id: &str,
    plan_id: Option<&str>,
    planes: &[Plane],
) -> Result<Vec<Section>, String> {
    model.validate()?;
    let part = model
        .parts
        .iter()
        .find(|p| p.id == part_id)
        .ok_or_else(|| format!("unknown part {part_id}"))?;
    let wanted = plan_id.unwrap_or(&part.adopted);
    let plan = part
        .manufacturing
        .iter()
        .find(|plan| plan.id == wanted)
        .ok_or_else(|| format!("part {part_id} has no manufacturing plan {wanted}"))?;
    let grid = Grid::rasterize(part, model.policy.voxel_mm)?;
    grid.sections(&grid.analyse(&model.policy, &Limits::of(plan)), planes)
}

/// 外部のSTLを格子にし、指定した製造案で判定した断面を返す。
pub fn sections_of_stl(
    bytes: &[u8],
    policy: &Policy,
    plan: &ManufacturingPlan,
    planes: &[Plane],
) -> Result<Vec<Section>, String> {
    plan.validate()?;
    let grid = stl_grid(bytes, policy)?;
    check_milling_work(grid.size, policy.voxel_mm, plan, "mesh")?;
    grid.sections(&grid.analyse(policy, &Limits::of(plan)), planes)
}

/// 製造案から決まる判定の値。IRの部品と外部形状で共有する。
#[derive(Debug, Clone)]
pub struct Limits {
    pub min_wall_mm: f64,
    /// 製造案の`orientation.up`。積層方向、工具を下ろす側、型を開く軸の基準である。
    pub up: Direction,
    pub process: Process,
    /// 段取りごとの`up`。切削以外は`up`だけである。
    pub setups: Vec<Direction>,
}

impl Limits {
    pub fn of(plan: &ManufacturingPlan) -> Self {
        Self {
            min_wall_mm: plan.process.min_wall_mm(),
            up: plan.orientation.up,
            process: plan.process.clone(),
            setups: plan.setups().map(|orientation| orientation.up).collect(),
        }
    }
}

/// 製造法に当てはまらないmaskは空である。断面の塗り分けでは空のmaskを偽として読む。
fn marked(mask: &[bool], index: usize) -> bool {
    mask.get(index).copied().unwrap_or(false)
}

/// 工具の半径をcell数で表したもの。量子化の誤差を失敗側へ倒すため、半cellを加える。
fn tool_radius_cells(tool_diameter_mm: f64, pitch: f64) -> f64 {
    (tool_diameter_mm / 2.0 + pitch / 2.0) / pitch
}

/// 円板の行`offset`に含まれるcellの、行の中心からの最大のずれ。`disc_offsets`と同じ円板である。
fn half_width(radius: f64, offset: isize) -> usize {
    let mut width = (radius * radius - (offset * offset) as f64).max(0.0).sqrt() as usize;
    while ((offset * offset) as f64) + ((width + 1) * (width + 1)) as f64 <= radius * radius {
        width += 1;
    }
    while width > 0 && ((offset * offset) as f64) + (width * width) as f64 > radius * radius {
        width -= 1;
    }
    width
}

#[derive(Clone, Copy, PartialEq)]
enum Extreme {
    Max,
    Min,
}

/// 平面 (`dims`は行数と列数) の各点で、半径`radius` cellの円板に含まれる値の最大か最小。
/// 平面の外は除く。円板を行ごとの区間に分け、各行で区間の極値を単調な待ち行列で求めるため、
/// 計算量は平面の点数と円板の行数の積である。円板の全cellを数えると半径の2乗に比例する。
fn disc_extreme(values: &[u32], dims: [usize; 2], radius: f64, extreme: Extreme) -> Vec<u32> {
    let [rows, columns] = dims;
    let reach = radius.floor() as isize;
    let mut result = vec![
        match extreme {
            Extreme::Max => 0,
            Extreme::Min => u32::MAX,
        };
        values.len()
    ];
    let mut window = vec![0u32; columns];
    let mut queue = std::collections::VecDeque::with_capacity(columns);
    let better = |x: u32, y: u32| match extreme {
        Extreme::Max => x >= y,
        Extreme::Min => x <= y,
    };
    for offset in -reach..=reach {
        let half = half_width(radius, offset);
        for row in 0..rows {
            let Some(source) = row.checked_add_signed(offset).filter(|r| *r < rows) else {
                continue;
            };
            let line = &values[source * columns..(source + 1) * columns];
            queue.clear();
            let mut next = 0usize;
            for (column, slot) in window.iter_mut().enumerate() {
                let last = (column + half).min(columns - 1);
                while next <= last {
                    while queue
                        .back()
                        .is_some_and(|back| better(line[next], line[*back]))
                    {
                        queue.pop_back();
                    }
                    queue.push_back(next);
                    next += 1;
                }
                let first = column.saturating_sub(half);
                while queue.front().is_some_and(|front| *front < first) {
                    queue.pop_front();
                }
                *slot = line[*queue.front().expect("the window holds its own column")];
            }
            for (target, value) in result[row * columns..(row + 1) * columns]
                .iter_mut()
                .zip(window.iter())
            {
                if better(*value, *target) {
                    *target = *value;
                }
            }
        }
    }
    result
}

/// 切削の判定に要する作業量。段取りごとの、工具の半径だけ広げた平面の点数と、
/// 平面を走査する円板の行数の積の最大である。平面の点数も返す。切削でなければ0とする。
pub fn milling_work(size: [usize; 3], pitch: f64, plan: &ManufacturingPlan) -> Option<(u64, u64)> {
    let Process::Milling {
        tool_diameter_mm, ..
    } = &plan.process
    else {
        return Some((0, 0));
    };
    let margin = tool_radius_cells(*tool_diameter_mm, pitch).floor() as u64;
    let mut work = (0u64, 0u64);
    for orientation in plan.setups() {
        let [first, second] = orientation.up.axis().plane();
        let plane = (size[first] as u64)
            .checked_add(margin.checked_mul(2)?)?
            .checked_mul((size[second] as u64).checked_add(margin.checked_mul(2)?)?)?;
        let total = plane.checked_mul(margin.checked_mul(2)?.checked_add(1)?)?;
        work = (work.0.max(total), work.1.max(plane));
    }
    Some(work)
}

/// 切削の判定に要する作業量が上限以内か。工具の径が格子に対して大きすぎる入力を、
/// 評価で記憶を使い尽くす前に拒否する。
pub fn check_milling_work(
    size: [usize; 3],
    pitch: f64,
    plan: &ManufacturingPlan,
    owner: &str,
) -> Result<(), String> {
    match milling_work(size, pitch, plan) {
        Some((work, plane)) if work <= MAX_MILLING_WORK && plane <= MAX_GRID_CELLS => Ok(()),
        _ => Err(format!(
            "{owner}: manufacturing plan {} needs more than {MAX_MILLING_WORK} steps or {MAX_GRID_CELLS} plane cells to trace the tool at voxel_mm={pitch}; use a coarser grid or a smaller tool",
            plan.id
        )),
    }
}

/// 半径`radius` cellの円板に含まれる平面上のずれ。
fn disc_offsets(radius: f64) -> Vec<(isize, isize)> {
    let reach = radius.floor() as isize;
    (-reach..=reach)
        .flat_map(|da| (-reach..=reach).map(move |db| (da, db)))
        .filter(|(da, db)| ((da * da + db * db) as f64) <= radius * radius)
        .collect()
}

/// 形状だけで決まる判定材料。製造案に依らず、部品ごとに1回求める。
pub struct Structure {
    /// erosionで分かれた成分どうしを繋ぐ細い接続部。
    pub neck: Vec<bool>,
    /// 外部に通じない空cell。
    pub void: Vec<bool>,
    core_cells: usize,
    core_components: usize,
    enclosed: usize,
}

/// 製造案に依る判定材料。製造法に当てはまらないmaskは空である。
pub struct Manufacture {
    /// 一辺`min_wall_mm`の立方体以上の大きさを持つ薄肉領域。
    pub thin: Vec<bool>,
    /// FDMとUV樹脂: bridgeでも支持されないcell。
    pub unsupported: Vec<bool>,
    /// UV樹脂: 細い通路の奥にあり、樹脂が抜けない空間。
    pub trapped: Vec<bool>,
    /// UV樹脂: 造形中に造形板の側だけが閉じた椀となる空間。
    pub suction: Vec<bool>,
    /// 切削: どの段取りからも細い工具が届かない素材内の空間。
    pub unreachable: Vec<bool>,
    /// 切削: 細い工具なら届くが、工具の径では削れない素材内の空間。
    pub corner: Vec<bool>,
    /// 射出成形: 型を開く軸のどちら側へも抜けない空間。
    pub undercut: Vec<bool>,
    /// 射出成形: 内接球の直径が最大肉厚を超える材料の芯。
    pub thick: Vec<bool>,
    thin_regions: usize,
    largest_thin: usize,
    unsupported_cells: usize,
    spanned_cells: usize,
    trapped_regions: usize,
    suction_regions: usize,
    unreachable_cells: usize,
    corner_cells: usize,
    undercut_cells: usize,
    /// 最も厚い箇所の内接球の直径の上界。単位はcell。
    thickest_cells: f64,
}

/// 最終形状のruleが検出したcellのmaskと、messageに書く集計値。
///
/// 判定と図示が同じmaskを使い、図に描いた箇所と判定の根拠を一致させる。
pub struct Analysis {
    pub structure: Structure,
    pub manufacture: Manufacture,
}

impl Grid {
    /// 1つの製造案での判定材料を求める。断面図はこれを描く。
    pub fn analyse(&self, policy: &Policy, limits: &Limits) -> Analysis {
        let structure = self.structure(policy);
        let manufacture = self.manufacture(limits, &structure.void);
        Analysis {
            structure,
            manufacture,
        }
    }

    /// 細い接続部と閉空洞。
    pub fn structure(&self, policy: &Policy) -> Structure {
        let pitch = self.pitch;
        let neck_radius = (policy.min_neck_mm + pitch) / 2.0 / pitch;
        let core = self.eroded(neck_radius);
        let core_cells = core.iter().filter(|cell| **cell).count();
        let (core_labels, core_sizes) = self.labels(&core);
        let neck = if core_cells == 0 {
            // 断面を保つ材料が残らない。部品全体を接続部不足として示す。
            self.occupied.clone()
        } else if core_sizes.len() > 1 {
            let meeting = self.meeting_cells(&core_labels);
            if meeting.iter().any(|cell| *cell) {
                meeting
            } else {
                // 材料で繋がっていない成分どうしは前線が出会わない。最大の成分以外を
                // 切り離された部分として示す。
                let largest = (0..core_sizes.len())
                    .max_by_key(|slot| core_sizes[*slot])
                    .map_or(0, |slot| slot as u32 + 1);
                core_labels
                    .iter()
                    .map(|label| *label > 0 && *label != largest)
                    .collect()
            }
        } else {
            vec![false; self.occupied.len()]
        };
        let void = self.enclosed_void_mask();
        let enclosed = void.iter().filter(|cell| **cell).count();
        Structure {
            neck,
            void,
            core_cells,
            core_components: core_sizes.len(),
            enclosed,
        }
    }

    /// 肉厚、支持、UV樹脂の排出と吸着。
    pub fn manufacture(&self, limits: &Limits, void: &[bool]) -> Manufacture {
        let pitch = self.pitch;
        // 量子化の誤差を失敗側へ倒すため、要求値にgrid間隔を足した長さを求める。
        let required_run = ((limits.min_wall_mm + pitch) / pitch).ceil() as usize;
        let runs = self.axial_runs();
        let thin_cells: Vec<bool> = self
            .occupied
            .iter()
            .zip(runs.iter())
            .map(|(solid, run)| *solid && *run < required_run)
            .collect();
        // 面の縁や稜線は必ず薄くなる。一辺min_wall_mmの立方体に満たない領域は
        // 形状の縁であり、壁の薄さを示さないため数えない。
        let minimum_cells = (limits.min_wall_mm.powi(3) / pitch.powi(3)).ceil() as usize;
        let (labels, sizes) = self.labels(&thin_cells);
        let thin: Vec<bool> = labels
            .iter()
            .map(|label| *label > 0 && sizes[*label as usize - 1] >= minimum_cells)
            .collect();
        let kept: Vec<usize> = sizes
            .iter()
            .copied()
            .filter(|size| *size >= minimum_cells)
            .collect();

        let count = self.occupied.len();
        let (unsupported, unsupported_cells, spanned_cells) = match limits.process.layering() {
            Some((overhang, bridge)) => {
                let support = self.support(limits.up, overhang, bridge);
                let unsupported: Vec<bool> = self
                    .occupied
                    .iter()
                    .zip(support.supported.iter())
                    .zip(support.bridged.iter())
                    .map(|((solid, held), spanned)| *solid && !*held && !*spanned)
                    .collect();
                let cells = unsupported.iter().filter(|cell| **cell).count();
                let spanned = support.bridged.iter().filter(|cell| **cell).count();
                (unsupported, cells, spanned)
            }
            None => (Vec::new(), 0, 0),
        };

        let (trapped, suction) = match limits.process.min_drain_mm() {
            Some(drain) => (
                self.trapped_resin(drain, void),
                self.suction_cups(limits.up, drain),
            ),
            None => (Vec::new(), Vec::new()),
        };
        let (unreachable, corner) = match &limits.process {
            Process::Milling {
                tool_diameter_mm,
                tool_length_mm,
                ..
            } => self.milling_leftovers(&limits.setups, *tool_diameter_mm, *tool_length_mm),
            _ => (Vec::new(), Vec::new()),
        };
        let (undercut, thick, thickest_cells) = match &limits.process {
            Process::Molding { max_wall_mm, .. } => {
                let (thick, thickest) = self.thick_sections(*max_wall_mm);
                (self.undercuts(limits.up), thick, thickest)
            }
            _ => (Vec::new(), Vec::new(), 0.0),
        };
        let cells = |mask: &[bool]| mask.iter().filter(|cell| **cell).count();
        debug_assert!(
            [&unreachable, &corner, &undercut, &thick]
                .iter()
                .all(|mask| mask.is_empty() || mask.len() == count)
        );
        Manufacture {
            thin,
            unsupported,
            trapped_regions: self.labels(&trapped).1.len(),
            suction_regions: self.labels(&suction).1.len(),
            trapped,
            suction,
            unreachable_cells: cells(&unreachable),
            corner_cells: cells(&corner),
            undercut_cells: cells(&undercut),
            unreachable,
            corner,
            undercut,
            thick,
            thin_regions: kept.len(),
            largest_thin: kept.iter().copied().max().unwrap_or(0),
            unsupported_cells,
            spanned_cells,
            thickest_cells,
        }
    }

    /// 材料のcellを含む最小のindex範囲。材料が無ければNone。
    fn material_bounds(&self) -> Option<([usize; 3], [usize; 3])> {
        let mut low = [usize::MAX; 3];
        let mut high = [0usize; 3];
        let [nx, ny, _] = self.size;
        for (index, solid) in self.occupied.iter().enumerate() {
            if *solid {
                let cell = [index % nx, (index / nx) % ny, index / (nx * ny)];
                for axis in 0..3 {
                    low[axis] = low[axis].min(cell[axis]);
                    high[axis] = high[axis].max(cell[axis]);
                }
            }
        }
        (low[0] != usize::MAX).then_some((low, high))
    }

    /// 3軸の切削で削り残す素材内の空間。素材は材料の外接boxとする。
    ///
    /// 返り値は、どの段取りからも径0の工具が届かない空間 (undercutと届かない深さ) と、
    /// 径0の工具なら届くが`tool_diameter_mm`の工具では削れない空間 (内角の丸みと細い溝) である。
    /// 工具は平端の円柱で、段取りの`up`の側から下ろす。先端の高さは、工具の円板の下にある
    /// 材料の最上面より上に限る。素材の上面から`tool_length_mm`より深くは届かない。
    /// 量子化の誤差を失敗側へ倒すため、工具の半径に半cellを加え、届く深さは切り捨てる。
    /// 一方、工具軸に垂直な層で8-連結の2 cell以下の削り残しは、格子化した円弧と円板の差で
    /// 生じるため、削れたものとみなす。工具の半径が1.5 cellを超えれば、鋭い内角は層内で
    /// 3 cell以上を削り残す。
    fn milling_leftovers(
        &self,
        setups: &[Direction],
        tool_diameter_mm: f64,
        tool_length_mm: f64,
    ) -> (Vec<bool>, Vec<bool>) {
        let count = self.occupied.len();
        let Some((stock_low, stock_high)) = self.material_bounds() else {
            return (vec![false; count], vec![false; count]);
        };
        let [nx, ny, _] = self.size;
        let in_stock = |index: usize| {
            let cell = [index % nx, (index / nx) % ny, index / (nx * ny)];
            (0..3).all(|axis| (stock_low[axis]..=stock_high[axis]).contains(&cell[axis]))
        };
        let radius = tool_radius_cells(tool_diameter_mm, self.pitch);
        let around = disc_offsets(1.5);
        let margin = radius.floor() as usize;
        let reach = (tool_length_mm / self.pitch).floor() as usize;
        let mut thin_reach = vec![false; count];
        let mut tool_reach = vec![false; count];
        let mut reached = vec![false; count];
        for direction in setups {
            let frame = Layered::new(*direction);
            let layers = self.size[frame.up];
            let extent = [self.size[frame.plane[0]], self.size[frame.plane[1]]];
            // 工具の中心は格子の外にも置ける。円板の半径だけ広げた平面で扱う。
            let wide = [extent[0] + 2 * margin, extent[1] + 2 * margin];
            let at = |a: usize, b: usize| a * wide[1] + b;
            // 各列で、材料の最上cellの1つ上の層。材料が無い列は0。
            let mut above = vec![0u32; wide[0] * wide[1]];
            for a in 0..extent[0] {
                for b in 0..extent[1] {
                    let top = (0..layers)
                        .rev()
                        .find(|layer| self.occupied[frame.cell(self, *layer, a, b)]);
                    above[at(a + margin, b + margin)] = top.map_or(0, |layer| layer as u32 + 1);
                }
            }
            let surface = above.iter().copied().max().unwrap_or(0);
            let deepest = surface.saturating_sub(reach as u32);
            // 中心qに置いた工具の先端が下りられる層。円板の下の材料より上で、届く深さまで。
            let mut tip = disc_extreme(&above, wide, radius, Extreme::Max);
            for value in &mut tip {
                *value = (*value).max(deepest);
            }
            // 列を覆う工具の位置のうち、最も深く下りられるもの。
            let floor = disc_extreme(&tip, wide, radius, Extreme::Min);
            drop(tip);
            for a in 0..extent[0] {
                for b in 0..extent[1] {
                    let (ca, cb) = (a + margin, b + margin);
                    let thin_floor = above[at(ca, cb)].max(deepest) as usize;
                    let tool_floor = floor[at(ca, cb)] as usize;
                    for layer in thin_floor..layers {
                        thin_reach[frame.cell(self, layer, a, b)] = true;
                    }
                    for layer in tool_floor.min(layers)..layers {
                        reached[frame.cell(self, layer, a, b)] = true;
                    }
                }
            }
            let left = |layer: usize, a: usize, b: usize| {
                let index = frame.cell(self, layer, a, b);
                !self.occupied[index] && !reached[index] && in_stock(index)
            };
            // 層内で8-連結の削り残しのうち、2 cell以下のものは格子化の差として削れたものとする。
            let mut seen = vec![false; extent[0] * extent[1]];
            let mut component = Vec::new();
            for layer in 0..layers {
                seen.fill(false);
                for a in 0..extent[0] {
                    for b in 0..extent[1] {
                        let index = frame.cell(self, layer, a, b);
                        tool_reach[index] |= reached[index];
                        if seen[a * extent[1] + b] || !left(layer, a, b) {
                            continue;
                        }
                        component.clear();
                        let mut stack = vec![(a, b)];
                        seen[a * extent[1] + b] = true;
                        while let Some((ca, cb)) = stack.pop() {
                            component.push((ca, cb));
                            for (da, db) in &around {
                                let (Some(na), Some(nb)) = (
                                    ca.checked_add_signed(*da).filter(|v| *v < extent[0]),
                                    cb.checked_add_signed(*db).filter(|v| *v < extent[1]),
                                ) else {
                                    continue;
                                };
                                if !seen[na * extent[1] + nb] && left(layer, na, nb) {
                                    seen[na * extent[1] + nb] = true;
                                    stack.push((na, nb));
                                }
                            }
                        }
                        if component.len() <= 2 {
                            for (ca, cb) in &component {
                                tool_reach[frame.cell(self, layer, *ca, *cb)] = true;
                            }
                        }
                    }
                }
            }
            reached.fill(false);
        }
        let mut unreachable = vec![false; count];
        let mut corner = vec![false; count];
        for index in 0..count {
            if self.occupied[index] || !in_stock(index) {
                continue;
            }
            unreachable[index] = !thin_reach[index];
            corner[index] = thin_reach[index] && !tool_reach[index];
        }
        (unreachable, corner)
    }

    /// 2枚型で、`direction`の軸のどちら側へも抜けない空cell。列の最初と最後の材料の間にある。
    fn undercuts(&self, direction: Direction) -> Vec<bool> {
        let frame = Layered::new(direction);
        let layers = self.size[frame.up];
        let extent = [self.size[frame.plane[0]], self.size[frame.plane[1]]];
        let mut undercut = vec![false; self.occupied.len()];
        for a in 0..extent[0] {
            for b in 0..extent[1] {
                let solid = |layer: usize| self.occupied[frame.cell(self, layer, a, b)];
                let (Some(first), Some(last)) = (
                    (0..layers).find(|layer| solid(*layer)),
                    (0..layers).rev().find(|layer| solid(*layer)),
                ) else {
                    continue;
                };
                for layer in first..last {
                    if !solid(layer) {
                        undercut[frame.cell(self, layer, a, b)] = true;
                    }
                }
            }
        }
        undercut
    }

    /// 内接球の直径が`max_wall_mm`を超える材料の芯と、最大の直径の上界 (cell)。
    ///
    /// 空cellの中心までの距離がd cellの材料cellを中心とする内接球の直径は、表面までの
    /// 半cellを除いた(2d - 1) cellと見積もれる。量子化の誤差を失敗側へ倒すため、これに
    /// grid間隔を足した2d cellが最大肉厚を超えれば厚肉とする。軸方向の連続長と異なり、
    /// 壁の角を厚肉と誤らない。
    fn thick_sections(&self, max_wall_mm: f64) -> (Vec<bool>, f64) {
        let empty: Vec<bool> = self.occupied.iter().map(|cell| !cell).collect();
        let distance = self.distance_squared(&empty);
        let limit = max_wall_mm / 2.0 / self.pitch;
        let thick = self
            .occupied
            .iter()
            .zip(distance.iter())
            .map(|(solid, squared)| *solid && *squared > limit * limit)
            .collect();
        let deepest = self
            .occupied
            .iter()
            .zip(distance.iter())
            .filter(|(solid, _)| **solid)
            .map(|(_, squared)| *squared)
            .fold(0.0f64, f64::max);
        (thick, 2.0 * deepest.sqrt())
    }

    /// 幅`min_drain_mm`の球が外から入れない空間のうち、その球が収まる部分。
    ///
    /// 材料から球の半径より遠い空cellを、外周から辿る。辿れないcellは、通路が排出に
    /// 必要な幅より細い奥にある。閉空洞はclosed_cavityが扱うため除く。通路の幅を
    /// 失敗側へ倒すため、要求値にgrid間隔を足した幅で判定する。
    fn trapped_resin(&self, min_drain_mm: f64, void: &[bool]) -> Vec<bool> {
        let radius = (min_drain_mm + self.pitch) / 2.0 / self.pitch;
        let distance = self.distance_squared(&self.occupied);
        let wide: Vec<bool> = self
            .occupied
            .iter()
            .zip(distance.iter())
            .map(|(solid, squared)| !*solid && *squared >= radius * radius)
            .collect();
        let reached = self.reachable_from_boundary(&wide);
        wide.iter()
            .zip(reached.iter())
            .zip(void.iter())
            .map(|((open, seen), enclosed)| *open && !*seen && !*enclosed)
            .collect()
    }

    /// 造形中のある層で、外周へ通じず造形板の側が閉じている空間 (椀)。
    ///
    /// 層を造形板の側から順に加え、それまでの層の空cellを連結する。加えた層の空cellの
    /// 成分が外周に触れていなければ、その層の硬化時に槽の底との間で閉じた空間となる。
    /// 材料が最初に現れる層が造形板に接し、それより前の層は数えない。幅`min_drain_mm`の
    /// 球が収まらない細い椀は除く。
    fn suction_cups(&self, direction: Direction, min_drain_mm: f64) -> Vec<bool> {
        let frame = Layered::new(direction);
        let layers = self.size[frame.up];
        let extent = [self.size[frame.plane[0]], self.size[frame.plane[1]]];
        let count = self.occupied.len();
        let mut parent: Vec<usize> = (0..count).collect();
        let mut open = vec![false; count];
        let mut cup = vec![false; count];

        fn find(parent: &mut [usize], mut index: usize) -> usize {
            while parent[index] != index {
                parent[index] = parent[parent[index]];
                index = parent[index];
            }
            index
        }
        fn union(parent: &mut [usize], open: &mut [bool], a: usize, b: usize) {
            let (ra, rb) = (find(parent, a), find(parent, b));
            if ra != rb {
                parent[rb] = ra;
                open[ra] = open[ra] || open[rb];
            }
        }

        let has_material = |layer: usize| {
            (0..extent[0])
                .any(|a| (0..extent[1]).any(|b| self.occupied[frame.cell(self, layer, a, b)]))
        };
        let Some(first) = (0..layers).find(|layer| has_material(*layer)) else {
            return cup;
        };
        for layer in first..layers {
            for a in 0..extent[0] {
                for b in 0..extent[1] {
                    let index = frame.cell(self, layer, a, b);
                    if self.occupied[index] {
                        continue;
                    }
                    if a == 0 || b == 0 || a + 1 == extent[0] || b + 1 == extent[1] {
                        open[index] = true;
                    }
                    if layer > first {
                        let below = frame.cell(self, layer - 1, a, b);
                        if !self.occupied[below] {
                            union(&mut parent, &mut open, below, index);
                        }
                    }
                    if a > 0 {
                        let left = frame.cell(self, layer, a - 1, b);
                        if !self.occupied[left] {
                            union(&mut parent, &mut open, left, index);
                        }
                    }
                    if b > 0 {
                        let back = frame.cell(self, layer, a, b - 1);
                        if !self.occupied[back] {
                            union(&mut parent, &mut open, back, index);
                        }
                    }
                }
            }
            for a in 0..extent[0] {
                for b in 0..extent[1] {
                    let index = frame.cell(self, layer, a, b);
                    if !self.occupied[index] {
                        let root = find(&mut parent, index);
                        cup[index] = !open[root];
                    }
                }
            }
        }
        // 幅`min_drain_mm`の球が収まる椀だけを数える。距離はcell中心から材料のcell中心までで
        // あり、材料の表面までの距離より半cell長いため、その分を要求に加える。
        let radius = (min_drain_mm + self.pitch) / 2.0 / self.pitch;
        let distance = self.distance_squared(&self.occupied);
        let (labels, sizes) = self.labels(&cup);
        let mut wide_component = vec![false; sizes.len()];
        for (label, squared) in labels.iter().zip(distance.iter()) {
            if *label > 0 && *squared >= radius * radius {
                wide_component[*label as usize - 1] = true;
            }
        }
        labels
            .iter()
            .map(|label| *label > 0 && wide_component[*label as usize - 1])
            .collect()
    }

    /// 外周の`mask`のcellから`mask`内を6-連結で辿れるcell。
    fn reachable_from_boundary(&self, mask: &[bool]) -> Vec<bool> {
        let mut reached = vec![false; mask.len()];
        let mut stack = Vec::new();
        let [nx, ny, nz] = self.size;
        for z in 0..nz {
            for y in 0..ny {
                for x in 0..nx {
                    let on_boundary =
                        x == 0 || y == 0 || z == 0 || x + 1 == nx || y + 1 == ny || z + 1 == nz;
                    let index = self.index(x, y, z);
                    if on_boundary && mask[index] && !reached[index] {
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
                if mask[neighbour] && !reached[neighbour] {
                    reached[neighbour] = true;
                    stack.push(neighbour);
                }
            }
        }
        reached
    }

    /// 6-連結の連結成分のlabel (0は対象外、kは1始まり) と、各成分のcell数。
    fn labels(&self, mask: &[bool]) -> (Vec<u32>, Vec<usize>) {
        let mut labels = vec![0u32; mask.len()];
        let mut stack = Vec::new();
        let mut sizes = Vec::new();
        let [nx, ny, nz] = self.size;
        for start in 0..mask.len() {
            if !mask[start] || labels[start] != 0 {
                continue;
            }
            let label = sizes.len() as u32 + 1;
            let mut size = 0usize;
            labels[start] = label;
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
                    if mask[neighbour] && labels[neighbour] == 0 {
                        labels[neighbour] = label;
                        stack.push(neighbour);
                    }
                }
            }
            sizes.push(size);
        }
        (labels, sizes)
    }

    /// erosionで残った成分から材料の内部を幅優先で同時に広げ、異なる成分から来た
    /// 前線が接するcellを返す。前線は細い接続部で出会うため、この集合が接続部となる。
    fn meeting_cells(&self, core_labels: &[u32]) -> Vec<bool> {
        let mut owner = core_labels.to_vec();
        let mut queue: std::collections::VecDeque<usize> = owner
            .iter()
            .enumerate()
            .filter(|(_, label)| **label > 0)
            .map(|(index, _)| index)
            .collect();
        let [nx, ny, nz] = self.size;
        let neighbours = |index: usize| {
            let x = index % nx;
            let y = (index / nx) % ny;
            let z = index / (nx * ny);
            NEIGHBOURS.into_iter().filter_map(move |(dx, dy, dz)| {
                Some(
                    (checked_step(z, dz, nz)? * ny + checked_step(y, dy, ny)?) * nx
                        + checked_step(x, dx, nx)?,
                )
            })
        };
        while let Some(index) = queue.pop_front() {
            for neighbour in neighbours(index) {
                if self.occupied[neighbour] && owner[neighbour] == 0 {
                    owner[neighbour] = owner[index];
                    queue.push_back(neighbour);
                }
            }
        }
        (0..owner.len())
            .map(|index| {
                owner[index] > 0
                    && neighbours(index).any(|n| owner[n] > 0 && owner[n] != owner[index])
            })
            .collect()
    }

    /// maskの連結成分ごとの外接box。大きい成分から`MAX_LOCATIONS`件までと、成分の総数。
    /// `margin_mm`だけ各方向へ広げる。cellは中心から半pitchの範囲を占める。
    pub fn locations(&self, mask: &[bool], margin_mm: f64) -> (Vec<Location>, usize) {
        let (labels, sizes) = self.labels(mask);
        let mut low = vec![[usize::MAX; 3]; sizes.len()];
        let mut high = vec![[0usize; 3]; sizes.len()];
        let [nx, ny, _] = self.size;
        for (index, label) in labels.iter().enumerate() {
            if *label == 0 {
                continue;
            }
            let cell = [index % nx, (index / nx) % ny, index / (nx * ny)];
            let slot = *label as usize - 1;
            for axis in 0..3 {
                low[slot][axis] = low[slot][axis].min(cell[axis]);
                high[slot][axis] = high[slot][axis].max(cell[axis]);
            }
        }
        let mut order: Vec<usize> = (0..sizes.len()).collect();
        order.sort_by(|a, b| sizes[*b].cmp(&sizes[*a]).then(a.cmp(b)));
        let half = self.pitch / 2.0 + margin_mm;
        let boxes = order
            .into_iter()
            .take(MAX_LOCATIONS)
            .map(|slot| Location {
                min: std::array::from_fn(|axis| self.centre(axis, low[slot][axis]) - half),
                max: std::array::from_fn(|axis| self.centre(axis, high[slot][axis]) + half),
            })
            .collect();
        (boxes, sizes.len())
    }
}

/// 1つの占有格子に対する最終形状のrule。plansは製造案と、採用したものかの組である。
fn evaluate_grid(
    grid: &Grid,
    target: &str,
    policy: &Policy,
    plans: &[(&ManufacturingPlan, bool)],
) -> Vec<Check> {
    let pitch = grid.pitch;
    let solid_cells = grid.occupied_cells();
    if solid_cells == 0 {
        // 空形状はvalid_solidが扱う。ここでは判定材料が無いことを明示する。
        return Rule::VOXEL
            .into_iter()
            .map(|rule| check(rule, target, false, "rasterized geometry is empty".into()))
            .collect();
    }
    let cell_mm3 = pitch.powi(3);
    let located = |mut result: Check, mask: &[bool], margin: f64| {
        if result.status == Status::Fail {
            let (locations, total) = grid.locations(mask, margin);
            result.message.push_str(&format!("; {total} location(s)"));
            result.locations = locations;
        }
        result
    };

    let structure = grid.structure(policy);
    let mut checks = vec![
        located(
            check(
                Rule::NeckSection,
                target,
                structure.core_components == 1,
                if structure.core_cells == 0 {
                    format!(
                        "no material survives a {} mm section; grid {} mm",
                        policy.min_neck_mm, pitch
                    )
                } else {
                    format!(
                        "{} component(s) remain after eroding to a {} mm section; grid {} mm",
                        structure.core_components, policy.min_neck_mm, pitch
                    )
                },
            ),
            &structure.neck,
            // 前線が出会う面は薄いため、断面の幅だけ広げて接続部を囲む。
            policy.min_neck_mm / 2.0,
        ),
        located(
            check(
                Rule::ClosedCavity,
                target,
                structure.enclosed == 0,
                format!(
                    "{} enclosed void cell(s); grid {} mm; cavities reachable only through gaps below the grid size are not distinguished",
                    structure.enclosed, pitch
                ),
            ),
            &structure.void,
            0.0,
        ),
    ];

    for (plan, adopted) in plans {
        let limits = Limits::of(plan);
        let made = grid.manufacture(&limits, &structure.void);
        let planned_check = |rule: Rule, passed: bool, message: String| {
            planned(rule, target, Some(&plan.id), *adopted, passed, message)
        };
        checks.push(located(
            planned_check(
                Rule::FinalWallThickness,
                made.thin_regions == 0,
                format!(
                    "{} region(s) thinner than {} mm; largest {:.3} mm³ of {:.3} mm³; grid {} mm; regions below {} mm³ are treated as edges",
                    made.thin_regions,
                    limits.min_wall_mm,
                    made.largest_thin as f64 * cell_mm3,
                    solid_cells as f64 * cell_mm3,
                    pitch,
                    limits.min_wall_mm.powi(3)
                ),
            ),
            &made.thin,
            0.0,
        ));
        match limits.process.layering() {
            Some((overhang, bridge)) => checks.push(located(
                planned_check(
                    Rule::SupportFree,
                    made.unsupported_cells == 0,
                    format!(
                        "{:.3} mm³ unsupported beyond {}° from {}; {:.3} mm³ carried by bridges up to {} mm; grid {} mm; slicer settings are not modelled",
                        made.unsupported_cells as f64 * cell_mm3,
                        overhang,
                        limits.up,
                        made.spanned_cells as f64 * cell_mm3,
                        bridge,
                        pitch
                    ),
                ),
                &made.unsupported,
                0.0,
            )),
            // 積層しない製造法には支持の要否が無い。対象が無いことを明示してpassとする。
            None => checks.push(planned_check(
                Rule::SupportFree,
                true,
                format!(
                    "not applicable to {}: the part is not built in layers",
                    limits.process.name()
                ),
            )),
        }
        match &limits.process {
            Process::Resin { min_drain_mm, .. } => {
                let drain = *min_drain_mm;
                checks.push(located(
                    planned_check(
                        Rule::ResinDrain,
                        made.trapped_regions == 0,
                        format!(
                            "{} pocket(s) drain only through passages narrower than {drain} mm; grid {pitch} mm; fully enclosed voids are reported by closed_cavity",
                            made.trapped_regions
                        ),
                    ),
                    &made.trapped,
                    // 判定したcellは通路の幅の球の中心であり、空間はその半径だけ広い。
                    (drain + pitch) / 2.0,
                ));
                checks.push(located(
                    planned_check(
                        Rule::ResinSuction,
                        made.suction_regions == 0,
                        format!(
                            "{} cup(s) sealed toward the build plate while printing along {}; cups narrower than {drain} mm are ignored; grid {pitch} mm",
                            made.suction_regions, limits.up
                        ),
                    ),
                    &made.suction,
                    0.0,
                ));
            }
            Process::Milling {
                tool_diameter_mm,
                tool_length_mm,
                ..
            } => {
                let setups: Vec<String> = limits.setups.iter().map(ToString::to_string).collect();
                let setups = setups.join(", ");
                checks.push(located(
                    planned_check(
                        Rule::MillingReach,
                        made.unreachable_cells == 0,
                        format!(
                            "{:.3} mm³ of the stock cannot be reached by a tool lowered from {setups} within {tool_length_mm} mm of the stock top; grid {pitch} mm; fixturing is not modelled",
                            made.unreachable_cells as f64 * cell_mm3
                        ),
                    ),
                    &made.unreachable,
                    0.0,
                ));
                checks.push(located(
                    planned_check(
                        Rule::MillingCorner,
                        made.corner_cells == 0,
                        format!(
                            "{:.3} mm³ is left by a {tool_diameter_mm} mm tool in inner corners with a radius below {} mm and gaps narrower than the tool; grid {pitch} mm",
                            made.corner_cells as f64 * cell_mm3,
                            (tool_diameter_mm + pitch) / 2.0
                        ),
                    ),
                    &made.corner,
                    0.0,
                ));
            }
            Process::Molding { max_wall_mm, .. } => {
                checks.push(located(
                    planned_check(
                        Rule::MoldUndercut,
                        made.undercut_cells == 0,
                        format!(
                            "{:.3} mm³ cannot be released toward {} or the opposite direction; side actions are not modelled; grid {pitch} mm",
                            made.undercut_cells as f64 * cell_mm3,
                            limits.up
                        ),
                    ),
                    &made.undercut,
                    0.0,
                ));
                let thick_regions = grid.labels(&made.thick).1.len();
                checks.push(located(
                    planned_check(
                        Rule::MoldThickWall,
                        thick_regions == 0,
                        format!(
                            "{thick_regions} region(s) thicker than {max_wall_mm} mm; thickest at most {:.3} mm; grid {pitch} mm",
                            made.thickest_cells * pitch
                        ),
                    ),
                    &made.thick,
                    // 判定したcellは内接球の中心であり、厚肉部はその半径だけ広い。
                    max_wall_mm / 2.0,
                ));
                let mut draft = planned_check(
                    Rule::MoldDraft,
                    false,
                    "not evaluated: axis-aligned boxes and cylinders carry no draft; add draft in the mold design".into(),
                );
                draft.status = Status::NotEvaluated;
                checks.push(draft);
            }
            Process::Fdm { .. } => {}
        }
    }
    checks
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Feature, Orientation, Policy, Process, Role, Shape, Units};

    fn part(features: Vec<Feature>) -> Part {
        Part {
            id: "block".into(),
            features,
            material: None,
            manufacturing: vec![],
            adopted: String::new(),
        }
    }

    /// 検査の条件。Policyと、部品に持たせるFDMの製造案。
    #[derive(Clone)]
    struct Setup {
        policy: Policy,
        plan: ManufacturingPlan,
    }

    fn fdm(min_wall: f64, overhang: f64, bridge: f64, up: Direction) -> ManufacturingPlan {
        ManufacturingPlan {
            id: "fdm".into(),
            material: None,
            process: Process::Fdm {
                min_wall_mm: min_wall,
                overhang_angle_deg: overhang,
                bridge_max_mm: bridge,
            },
            orientation: Orientation { up, turn_deg: 0 },
            source: "test".into(),
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

    fn model_with(mut part: Part, setup: Setup) -> Model {
        part.adopted = setup.plan.id.clone();
        part.manufacturing = vec![setup.plan];
        let policy = setup.policy;
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

    fn policy(min_wall: f64, min_neck: f64, pitch: f64) -> Setup {
        Setup {
            policy: Policy {
                min_feature_mm: 0.001,
                mesh_volume_tolerance: 0.01,
                voxel_mm: pitch,
                min_neck_mm: min_neck,
                required: vec![],
            },
            plan: fdm(min_wall, 45.0, 5.0, Direction::PlusZ),
        }
    }

    fn stl(bytes: &[u8], target: &str, setup: &Setup) -> Result<Vec<Check>, String> {
        evaluate_stl(bytes, target, &setup.policy, &setup.plan)
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
        sideways.plan.orientation.up = Direction::PlusX;
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
        narrow.plan = fdm(0.5, 0.0, 0.0, Direction::PlusZ);
        let vertical = evaluate(&model_with(stepped, narrow)).unwrap();
        // 真下にしか支持を認めないと、同じ張り出しが未支持になる。
        assert_eq!(status(&vertical, Rule::SupportFree), Status::Fail);
    }

    /// boxの12三角形。inwardなら法線を内向きにする (空洞の内面)。
    fn box_triangles(min: [f64; 3], max: [f64; 3], inward: bool) -> Vec<[[f64; 3]; 3]> {
        let corner = |i: usize| {
            [
                if i & 1 == 0 { min[0] } else { max[0] },
                if i & 2 == 0 { min[1] } else { max[1] },
                if i & 4 == 0 { min[2] } else { max[2] },
            ]
        };
        // 外向きの法線で反時計回りに見える4頂点の組。
        let faces = [
            [0, 2, 3, 1],
            [4, 5, 7, 6],
            [0, 1, 5, 4],
            [2, 6, 7, 3],
            [0, 4, 6, 2],
            [1, 3, 7, 5],
        ];
        let mut triangles = Vec::new();
        for [a, b, c, d] in faces {
            for tri in [[a, b, c], [a, c, d]] {
                let mut t = tri.map(corner);
                if inward {
                    t.swap(1, 2);
                }
                triangles.push(t);
            }
        }
        triangles
    }

    fn binary_stl(triangles: &[[[f64; 3]; 3]], header: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0u8; 80];
        bytes[..header.len()].copy_from_slice(header);
        bytes.extend((triangles.len() as u32).to_le_bytes());
        for triangle in triangles {
            bytes.extend([0u8; 12]);
            for vertex in triangle {
                for value in vertex {
                    bytes.extend((*value as f32).to_le_bytes());
                }
            }
            bytes.extend([0u8; 2]);
        }
        bytes
    }

    #[test]
    fn closed_box_mesh_fills_its_volume_exactly() {
        let grid = Grid::from_triangles(&box_triangles([0.; 3], [10.; 3], false), 1.0).unwrap();
        // 格子を半cellずらすため、面がcell中心を通らず1000 cellちょうどになる。
        assert_eq!(grid.occupied_cells(), 1000);
    }

    #[test]
    fn cell_centres_on_a_shared_diagonal_are_counted_once() {
        // 上下面は対角線で2三角形に分かれ、cell中心 (-0.1 + 0.2i, 22.9 + 0.2i) が
        // 対角線上に並ぶ。座標は2進で表せず、辺ごとの計算では丸めが一致しない。
        let grid =
            Grid::from_triangles(&box_triangles([0., 23., 0.], [3., 26., 3.], false), 0.2).unwrap();
        assert_eq!(grid.occupied_cells(), 15 * 15 * 15);
    }

    #[test]
    fn slanted_faces_follow_the_analytic_volume() {
        // |x|+|y|+|z| <= 6 の八面体。体積は(4/3)·6³ = 288 mm³。
        let r = 6.0;
        let tips = [
            [r, 0., 0.],
            [-r, 0., 0.],
            [0., r, 0.],
            [0., -r, 0.],
            [0., 0., r],
            [0., 0., -r],
        ];
        let mut triangles = Vec::new();
        for (x, y, z) in [
            (0, 2, 4),
            (2, 1, 4),
            (1, 3, 4),
            (3, 0, 4),
            (2, 0, 5),
            (1, 2, 5),
            (3, 1, 5),
            (0, 3, 5),
        ] {
            triangles.push([tips[x], tips[y], tips[z]]);
        }
        let pitch = 0.25;
        let grid = Grid::from_triangles(&triangles, pitch).unwrap();
        let volume = grid.occupied_cells() as f64 * pitch.powi(3);
        assert!((volume - 288.0).abs() / 288.0 < 0.03, "{volume}");
    }

    #[test]
    fn open_mesh_is_rejected() {
        let rules = policy(1.2, 1.2, 1.0);
        // 最後の2三角形は+x面。z方向から見て面積0のため、偶奇判定では検出できない。
        let mut side = box_triangles([0.; 3], [10.; 3], false);
        side.pop();
        let error = stl(&binary_stl(&side, b""), "a", &rules).unwrap_err();
        assert!(error.contains("not paired"), "{error}");
        // 上面の穴はwinding numberでも検出する。
        let mut top = box_triangles([0.; 3], [10.; 3], false);
        top.remove(2);
        let error = Grid::from_triangles(&top, 1.0).err().unwrap();
        assert!(error.contains("winding number"), "{error}");
    }

    #[test]
    fn a_reversed_triangle_on_a_vertical_face_is_rejected() {
        // +x面の三角形を1枚だけ裏返す。z方向から見て面積0のため、交点の数にも
        // 向きを問わない辺の偶奇にも現れない。
        let mut flipped = box_triangles([0.; 3], [10.; 3], false);
        let last = flipped.len() - 1;
        flipped[last].swap(1, 2);
        let error = stl(&binary_stl(&flipped, b""), "a", &policy(1.2, 1.2, 1.0)).unwrap_err();
        assert!(error.contains("not consistently oriented"), "{error}");
    }

    #[test]
    fn overlapping_solids_are_united_and_inverted_shells_are_rejected() {
        // x=0..10とx=5..15の2つのboxが重なる。偶奇則では重なりが外側になる。
        let mut pair = box_triangles([0.; 3], [10.; 3], false);
        pair.extend(box_triangles([5., 0., 0.], [15., 10., 10.], false));
        let grid = Grid::from_triangles(&pair, 1.0).unwrap();
        assert_eq!(grid.occupied_cells(), 15 * 10 * 10);
        // 内向きの面だけからなるshellは、上へ抜ける前にwinding numberが負になる。
        let error = Grid::from_triangles(&box_triangles([0.; 3], [10.; 3], true), 1.0)
            .err()
            .unwrap();
        assert!(error.contains("orientation"), "{error}");
    }

    #[test]
    fn stl_rules_detect_a_sealed_cavity_and_a_thin_plate() {
        let rules = policy(1.2, 1.2, 0.5);
        let solid = binary_stl(&box_triangles([0.; 3], [20.; 3], false), b"");
        let checks = stl(&solid, "block", &rules).unwrap();
        assert_eq!(status(&checks, Rule::ClosedCavity), Status::Pass);
        assert_eq!(status(&checks, Rule::FinalWallThickness), Status::Pass);
        assert!(checks.iter().all(|c| c.target == "block"));

        let mut hollow = box_triangles([0.; 3], [20.; 3], false);
        hollow.extend(box_triangles([5.; 3], [15.; 3], true));
        let checks = stl(&binary_stl(&hollow, b""), "hollow", &rules).unwrap();
        assert_eq!(status(&checks, Rule::ClosedCavity), Status::Fail);

        let plate = binary_stl(&box_triangles([0.; 3], [20., 20., 0.8], false), b"");
        let checks = stl(&plate, "plate", &rules).unwrap();
        assert_eq!(status(&checks, Rule::FinalWallThickness), Status::Fail);
    }

    #[test]
    fn ascii_and_binary_with_a_solid_header_are_both_read() {
        let triangles = box_triangles([0.; 3], [10.; 3], false);
        let mut text = String::from("solid cube\n");
        for t in &triangles {
            text.push_str("facet normal 0 0 0\nouter loop\n");
            for v in t {
                text.push_str(&format!("vertex {} {} {}\n", v[0], v[1], v[2]));
            }
            text.push_str("endloop\nendfacet\n");
        }
        text.push_str("endsolid cube\n");
        let rules = policy(1.2, 1.2, 1.0);
        let ascii = stl(text.as_bytes(), "a", &rules).unwrap();
        let binary = stl(&binary_stl(&triangles, b"solid header"), "b", &rules).unwrap();
        assert_eq!(ascii.len(), Rule::VOXEL.len());
        for (x, y) in ascii.iter().zip(binary.iter()) {
            assert_eq!(
                (x.rule, x.status, &x.message),
                (y.rule, y.status, &y.message)
            );
        }
    }

    #[test]
    fn invalid_policy_is_rejected_for_stl() {
        let mut rules = policy(1.2, 1.2, 1.0);
        rules.policy.voxel_mm = 0.0;
        let solid = binary_stl(&box_triangles([0.; 3], [10.; 3], false), b"");
        assert!(stl(&solid, "block", &rules).is_err());
    }

    fn locations(checks: &[Check], rule: Rule) -> Vec<Location> {
        checks
            .iter()
            .find(|c| c.rule == rule)
            .map(|c| c.locations.clone())
            .unwrap_or_default()
    }

    /// boxがpointを含むか。量子化の分だけ許す。
    fn encloses(location: &Location, point: [f64; 3]) -> bool {
        (0..3).all(|i| location.min[i] <= point[i] && point[i] <= location.max[i])
    }

    #[test]
    fn passing_checks_carry_no_locations() {
        let block = part(vec![add("body", box_shape([0.; 3], [10.; 3]))]);
        let checks = evaluate(&model_with(block, policy(1.0, 1.0, 0.5))).unwrap();
        assert!(checks.iter().all(|c| c.locations.is_empty()));
        let json = serde_json::to_string(&checks[0]).unwrap();
        assert!(!json.contains("locations"), "{json}");
    }

    #[test]
    fn thin_plate_location_covers_the_plate() {
        let plate = part(vec![add("body", box_shape([0.; 3], [10., 10., 0.8]))]);
        let checks = evaluate(&model_with(plate, policy(1.2, 0.1, 0.2))).unwrap();
        let found = locations(&checks, Rule::FinalWallThickness);
        assert_eq!(found.len(), 1);
        assert!(encloses(&found[0], [5., 5., 0.4]));
        assert!(
            found[0].max[2] <= 1.0 && found[0].min[2] >= -0.2,
            "{found:?}"
        );
    }

    #[test]
    fn neck_location_sits_on_the_neck() {
        let dumbbell = part(vec![
            add("left", box_shape([0., 0., 0.], [5., 5., 5.])),
            add("neck", box_shape([5., 2., 2.], [7., 3., 3.])),
            add("right", box_shape([7., 0., 0.], [12., 5., 5.])),
        ]);
        let checks = evaluate(&model_with(dumbbell, policy(0.5, 2.0, 0.25))).unwrap();
        let found = locations(&checks, Rule::NeckSection);
        assert_eq!(found.len(), 1, "{found:?}");
        // 首の中央を含み、両側の塊の中心は含まない。
        assert!(encloses(&found[0], [6., 2.5, 2.5]), "{found:?}");
        assert!(!encloses(&found[0], [2.5, 2.5, 2.5]));
        assert!(!encloses(&found[0], [9.5, 2.5, 2.5]));
    }

    #[test]
    fn separated_blocks_report_the_smaller_one() {
        // 2つの塊が1 mm離れている。材料で繋がらないため、小さい方を位置として示す。
        let pair = part(vec![
            add("big", box_shape([0., 0., 0.], [6., 6., 6.])),
            add("small", box_shape([7., 0., 0.], [10., 3., 3.])),
        ]);
        let checks = evaluate(&model_with(pair, policy(0.5, 1.0, 0.25))).unwrap();
        let found = locations(&checks, Rule::NeckSection);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(encloses(&found[0], [8.5, 1.5, 1.5]), "{found:?}");
        assert!(!encloses(&found[0], [3., 3., 3.]));
    }

    #[test]
    fn cavity_location_matches_the_pocket() {
        let sealed = part(vec![
            add("body", box_shape([0.; 3], [10., 10., 10.])),
            cut("void", box_shape([3., 3., 3.], [7., 7., 7.])),
        ]);
        let checks = evaluate(&model_with(sealed, policy(0.5, 0.5, 0.5))).unwrap();
        let found = locations(&checks, Rule::ClosedCavity);
        assert_eq!(found.len(), 1);
        for axis in 0..3 {
            assert!((found[0].min[axis] - 3.0).abs() <= 0.5, "{found:?}");
            assert!((found[0].max[axis] - 7.0).abs() <= 0.5, "{found:?}");
        }
    }

    #[test]
    fn unsupported_location_is_under_the_overhang() {
        let checks = evaluate(&model_with(table(8.0), policy(1.0, 1.0, 0.5))).unwrap();
        let found = locations(&checks, Rule::SupportFree);
        assert!(!found.is_empty());
        // 張り出しの下面 (x>4、z=6付近) にあり、脚の上には無い。
        assert!(
            found.iter().all(|l| l.max[0] > 4.0 && l.min[2] >= 5.5),
            "{found:?}"
        );
    }

    #[test]
    fn locations_are_capped_and_the_total_is_reported() {
        // 1 mm角の閉空洞を5×5で25個並べる。
        let mut features = vec![add("body", box_shape([0.; 3], [26., 26., 4.]))];
        for i in 0..5 {
            for j in 0..5 {
                let (x, y) = (2.0 + 5.0 * i as f64, 2.0 + 5.0 * j as f64);
                features.push(cut(
                    &format!("v{i}{j}"),
                    box_shape([x, y, 1.5], [x + 1., y + 1., 2.5]),
                ));
            }
        }
        let checks = evaluate(&model_with(part(features), policy(0.5, 0.5, 0.25))).unwrap();
        let cavity = checks
            .iter()
            .find(|c| c.rule == Rule::ClosedCavity)
            .unwrap();
        assert_eq!(cavity.locations.len(), MAX_LOCATIONS);
        assert!(
            cavity.message.ends_with("; 25 location(s)"),
            "{}",
            cavity.message
        );
    }

    fn count_bits(section: &Section, bit: u8) -> usize {
        section
            .rows
            .iter()
            .flat_map(|row| row.bytes())
            .map(|c| u8::from_str_radix(std::str::from_utf8(&[c]).unwrap(), 32).unwrap())
            .filter(|value| value & bit != 0)
            .count()
    }

    #[test]
    fn section_through_a_pocket_shows_the_void_and_the_wall() {
        let sealed = part(vec![
            add("body", box_shape([0.; 3], [10., 10., 10.])),
            cut("void", box_shape([3., 3., 3.], [7., 7., 7.])),
        ]);
        let model = model_with(sealed, policy(0.5, 0.5, 0.5));
        let planes = [
            Plane {
                axis: Axis::Z,
                coordinate: Some(5.0),
            },
            Plane {
                axis: Axis::Z,
                coordinate: Some(1.0),
            },
        ];
        let sections = sections_of_part(&model, "block", None, &planes).unwrap();
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].plane_coordinate, 5.0);
        // 中央の断面では空洞 (4 mm角、0.5 mm格子で9×9 cell) が現れ、床下の断面には無い。
        assert_eq!(count_bits(&sections[0], SECTION_VOID), 81);
        assert_eq!(count_bits(&sections[1], SECTION_VOID), 0);
        assert!(count_bits(&sections[0], SECTION_SOLID) > 0);
        let width = sections[0].rows[0].len();
        assert!(sections[0].rows.iter().all(|row| row.len() == width));
    }

    #[test]
    fn section_marks_the_neck() {
        let dumbbell = part(vec![
            add("left", box_shape([0., 0., 0.], [5., 5., 5.])),
            add("neck", box_shape([5., 2., 2.], [7., 3., 3.])),
            add("right", box_shape([7., 0., 0.], [12., 5., 5.])),
        ]);
        let model = model_with(dumbbell, policy(0.5, 2.0, 0.25));
        let planes = [Plane {
            axis: Axis::Y,
            coordinate: Some(2.5),
        }];
        let section = &sections_of_part(&model, "block", None, &planes).unwrap()[0];
        assert!(count_bits(section, SECTION_NECK) > 0);
        assert!(sections_of_part(&model, "ghost", None, &planes).is_err());
    }

    #[test]
    fn stl_sections_use_the_same_encoding() {
        let mut hollow = box_triangles([0.; 3], [20.; 3], false);
        hollow.extend(box_triangles([5.; 3], [15.; 3], true));
        let planes = [Plane {
            axis: Axis::X,
            coordinate: None,
        }];
        let setup = policy(1.2, 1.2, 1.0);
        let sections = sections_of_stl(
            &binary_stl(&hollow, b""),
            &setup.policy,
            &setup.plan,
            &planes,
        )
        .unwrap();
        assert_eq!(count_bits(&sections[0], SECTION_VOID), 100);
    }

    fn resin(min_drain: f64, up: Direction) -> ManufacturingPlan {
        ManufacturingPlan {
            id: "resin".into(),
            material: None,
            process: Process::Resin {
                min_wall_mm: 0.5,
                overhang_angle_deg: 45.0,
                bridge_max_mm: 5.0,
                min_drain_mm: min_drain,
            },
            orientation: Orientation { up, turn_deg: 0 },
            source: "test".into(),
        }
    }

    fn with_plan(plan: ManufacturingPlan) -> Setup {
        Setup {
            plan,
            ..policy(0.5, 0.5, 0.5)
        }
    }

    /// 20 mm角の塊の中に10 mm角の空洞があり、上面から径`hole` mmの角穴で外へ通じる。
    fn pocket_with_hole(hole: f64) -> Part {
        let half = hole / 2.0;
        part(vec![
            add("block", box_shape([0.; 3], [20.; 3])),
            cut("pocket", box_shape([5.; 3], [15.; 3])),
            cut(
                "hole",
                box_shape([10. - half, 10. - half, 14.], [10. + half, 10. + half, 21.]),
            ),
        ])
    }

    #[test]
    fn resin_pocket_behind_a_narrow_hole_is_trapped() {
        let narrow = evaluate(&model_with(
            pocket_with_hole(1.0),
            with_plan(resin(3.0, Direction::PlusZ)),
        ))
        .unwrap();
        assert_eq!(status(&narrow, Rule::ResinDrain), Status::Fail);
        // 外へ通じるため閉空洞ではない。
        assert_eq!(status(&narrow, Rule::ClosedCavity), Status::Pass);
        let location = locations(&narrow, Rule::ResinDrain)[0];
        assert!(encloses(&location, [10., 10., 10.]), "{location:?}");

        let wide = evaluate(&model_with(
            pocket_with_hole(5.0),
            with_plan(resin(3.0, Direction::PlusZ)),
        ))
        .unwrap();
        assert_eq!(status(&wide, Rule::ResinDrain), Status::Pass);
    }

    /// 上面が開いた20 mm角、深さ10 mmの箱。壁と床は2 mm。
    fn open_box() -> Part {
        part(vec![
            add("block", box_shape([0.; 3], [20., 20., 12.])),
            cut("inside", box_shape([2., 2., 2.], [18., 18., 13.])),
        ])
    }

    #[test]
    fn open_box_printed_upright_is_a_suction_cup() {
        let upright = evaluate(&model_with(
            open_box(),
            with_plan(resin(2.0, Direction::PlusZ)),
        ))
        .unwrap();
        assert_eq!(status(&upright, Rule::ResinSuction), Status::Fail);
        let location = locations(&upright, Rule::ResinSuction)[0];
        assert!(encloses(&location, [10., 10., 6.]), "{location:?}");
        // 横に置けば、各層の断面はU字で、開いた側から外周へ通じる。
        let sideways = evaluate(&model_with(
            open_box(),
            with_plan(resin(2.0, Direction::PlusX)),
        ))
        .unwrap();
        assert_eq!(status(&sideways, Rule::ResinSuction), Status::Pass);
    }

    #[test]
    fn narrow_cups_are_ignored() {
        // 幅1 mmの溝は、排出路の幅2 mmの球が収まらない。
        let groove = part(vec![
            add("block", box_shape([0.; 3], [20., 20., 6.])),
            cut("groove", box_shape([2., 9.5, 3.], [18., 10.5, 7.])),
        ]);
        let checks =
            evaluate(&model_with(groove, with_plan(resin(2.0, Direction::PlusZ)))).unwrap();
        assert_eq!(status(&checks, Rule::ResinSuction), Status::Pass);
    }

    #[test]
    fn every_plan_is_evaluated_and_marked() {
        let mut block = open_box();
        block.manufacturing = vec![
            fdm(1.0, 45.0, 5.0, Direction::PlusZ),
            resin(2.0, Direction::PlusZ),
        ];
        block.adopted = "fdm".into();
        let mut model = model_with(open_box(), policy(1.0, 1.0, 0.5));
        model.parts = vec![block];
        let checks = evaluate(&model).unwrap();
        let summary: Vec<(Rule, Option<&str>, bool)> = checks
            .iter()
            .map(|c| (c.rule, c.plan.as_deref(), c.adopted))
            .collect();
        assert_eq!(
            summary,
            vec![
                (Rule::NeckSection, None, true),
                (Rule::ClosedCavity, None, true),
                (Rule::FinalWallThickness, Some("fdm"), true),
                (Rule::SupportFree, Some("fdm"), true),
                (Rule::FinalWallThickness, Some("resin"), false),
                (Rule::SupportFree, Some("resin"), false),
                (Rule::ResinDrain, Some("resin"), false),
                (Rule::ResinSuction, Some("resin"), false),
            ]
        );
        // 採用していない製造案のfailは、出力の可否に用いない。
        let report = crate::Report {
            checks,
            required: vec![],
        };
        assert!(report.export_allowed());
    }

    fn milling(diameter: f64, length: f64, up: Direction, more: &[Direction]) -> ManufacturingPlan {
        ManufacturingPlan {
            id: "milling".into(),
            material: None,
            process: Process::Milling {
                min_wall_mm: 0.5,
                tool_diameter_mm: diameter,
                tool_length_mm: length,
                additional_setups: more
                    .iter()
                    .map(|up| Orientation {
                        up: *up,
                        turn_deg: 0,
                    })
                    .collect(),
            },
            orientation: Orientation { up, turn_deg: 0 },
            source: "test".into(),
        }
    }

    fn molding(max_wall: f64, up: Direction) -> ManufacturingPlan {
        ManufacturingPlan {
            id: "molding".into(),
            material: None,
            process: Process::Molding {
                min_wall_mm: 0.5,
                max_wall_mm: max_wall,
            },
            orientation: Orientation { up, turn_deg: 0 },
            source: "test".into(),
        }
    }

    fn z_cylinder(id: &str, center: [f64; 2], radius: f64, span: [f64; 2]) -> Feature {
        cut(
            id,
            Shape::Cylinder {
                axis: crate::Axis::Z,
                center,
                radius,
                span,
            },
        )
    }

    /// 30 mm角、高さ10 mmの塊の上面から、16 mm角、深さ8 mmのpocketを削る。
    /// `fillet`が正なら、pocketの縦の内角をその半径で丸める。
    fn pocket(fillet: f64) -> Part {
        let (low, high, z) = (7.0, 23.0, [2.0, 11.0]);
        let mut features = vec![add("block", box_shape([0.; 3], [30., 30., 10.]))];
        if fillet > 0.0 {
            let r = fillet;
            features.push(cut(
                "pocket_x",
                box_shape([low + r, low, z[0]], [high - r, high, z[1]]),
            ));
            features.push(cut(
                "pocket_y",
                box_shape([low, low + r, z[0]], [high, high - r, z[1]]),
            ));
            for (index, center) in [
                [low + r, low + r],
                [high - r, low + r],
                [low + r, high - r],
                [high - r, high - r],
            ]
            .into_iter()
            .enumerate()
            {
                features.push(z_cylinder(&format!("corner_{index}"), center, r, z));
            }
        } else {
            features.push(cut(
                "pocket",
                box_shape([low, low, z[0]], [high, high, z[1]]),
            ));
        }
        part(features)
    }

    #[test]
    fn sharp_inner_corners_are_left_by_the_tool() {
        let sharp = evaluate(&model_with(
            pocket(0.0),
            with_plan(milling(3.0, 20.0, Direction::PlusZ, &[])),
        ))
        .unwrap();
        assert_eq!(status(&sharp, Rule::MillingReach), Status::Pass);
        assert_eq!(status(&sharp, Rule::MillingCorner), Status::Fail);
        // 4つの縦の内角に1件ずつ残る。
        let corners = locations(&sharp, Rule::MillingCorner);
        assert_eq!(corners.len(), 4, "{corners:?}");
        assert!(
            corners.iter().any(|c| encloses(c, [7.25, 7.25, 5.0])),
            "{corners:?}"
        );
        // 積層しない製造法に支持の要否は無い。
        assert_eq!(status(&sharp, Rule::SupportFree), Status::Pass);

        // 工具の半径に格子1つ分を加えた丸みなら削り残さない。
        let rounded = evaluate(&model_with(
            pocket(2.0),
            with_plan(milling(3.0, 20.0, Direction::PlusZ, &[])),
        ))
        .unwrap();
        let corner = rounded
            .iter()
            .find(|c| c.rule == Rule::MillingCorner)
            .unwrap();
        assert_eq!(
            corner.status,
            Status::Pass,
            "{} {:?}",
            corner.message,
            corner.locations
        );
        assert_eq!(status(&rounded, Rule::MillingReach), Status::Pass);
    }

    #[test]
    fn a_short_tool_does_not_reach_the_pocket_floor() {
        let short = evaluate(&model_with(
            pocket(2.0),
            with_plan(milling(3.0, 5.0, Direction::PlusZ, &[])),
        ))
        .unwrap();
        assert_eq!(status(&short, Rule::MillingReach), Status::Fail);
        let floor = locations(&short, Rule::MillingReach)[0];
        assert!(encloses(&floor, [15.0, 15.0, 3.0]), "{floor:?}");
        assert!(floor.max[2] <= 5.5, "{floor:?}");
    }

    /// 20×10×10 mmの塊を、x方向に4 mm角の穴が貫く。
    fn tunnel() -> Part {
        part(vec![
            add("block", box_shape([0.; 3], [20., 10., 10.])),
            cut("tunnel", box_shape([-1., 3., 3.], [21., 7., 7.])),
        ])
    }

    #[test]
    fn side_holes_need_another_setup() {
        let top_only = evaluate(&model_with(
            tunnel(),
            with_plan(milling(3.0, 30.0, Direction::PlusZ, &[])),
        ))
        .unwrap();
        assert_eq!(status(&top_only, Rule::MillingReach), Status::Fail);
        let hole = locations(&top_only, Rule::MillingReach)[0];
        assert!(encloses(&hole, [10.0, 5.0, 5.0]), "{hole:?}");

        let both_ends = evaluate(&model_with(
            tunnel(),
            with_plan(milling(3.0, 30.0, Direction::PlusZ, &[Direction::PlusX])),
        ))
        .unwrap();
        assert_eq!(status(&both_ends, Rule::MillingReach), Status::Pass);
        // 穴の縦横の内角は、軸に沿って下ろす工具でも丸く残る。
        assert_eq!(status(&both_ends, Rule::MillingCorner), Status::Fail);
    }

    #[test]
    fn a_side_hole_is_an_undercut_for_a_vertical_mold() {
        let vertical = evaluate(&model_with(
            tunnel(),
            with_plan(molding(4.0, Direction::PlusZ)),
        ))
        .unwrap();
        assert_eq!(status(&vertical, Rule::MoldUndercut), Status::Fail);
        let hole = locations(&vertical, Rule::MoldUndercut)[0];
        assert!(encloses(&hole, [10.0, 5.0, 5.0]), "{hole:?}");
        // 穴の軸に沿って型を開けば抜ける。
        let along = evaluate(&model_with(
            tunnel(),
            with_plan(molding(4.0, Direction::MinusX)),
        ))
        .unwrap();
        assert_eq!(status(&along, Rule::MoldUndercut), Status::Pass);
        // 抜き勾配は評価しない。
        assert_eq!(status(&along, Rule::MoldDraft), Status::NotEvaluated);
        assert_eq!(status(&along, Rule::SupportFree), Status::Pass);
    }

    #[test]
    fn thick_sections_are_found_by_the_inscribed_sphere() {
        // 板と、それに立つ壁。どこも厚さ2 mmである。
        let tee = part(vec![
            add("plate", box_shape([0.; 3], [20., 20., 2.])),
            add("wall", box_shape([9., 0., 0.], [11., 20., 15.])),
        ]);
        // 交差部の内接円の直径は2.5 mmであり、格子の誤差を加えても4 mmに収まる。
        let thin = evaluate(&model_with(
            tee.clone(),
            with_plan(molding(4.0, Direction::PlusZ)),
        ))
        .unwrap();
        assert_eq!(status(&thin, Rule::MoldThickWall), Status::Pass);
        // 3 mmでは、格子の誤差を失敗側へ倒した交差部が上限を超える。
        let strict = evaluate(&model_with(tee, with_plan(molding(3.0, Direction::PlusZ)))).unwrap();
        let junction = locations(&strict, Rule::MoldThickWall)[0];
        assert!(encloses(&junction, [10.0, 10.0, 1.0]), "{junction:?}");

        let lump = part(vec![
            add("plate", box_shape([0.; 3], [20., 20., 2.])),
            add("boss", box_shape([8., 8., 0.], [14., 14., 6.])),
        ]);
        let thick = evaluate(&model_with(lump, with_plan(molding(4.0, Direction::PlusZ)))).unwrap();
        assert_eq!(status(&thick, Rule::MoldThickWall), Status::Fail);
        let lump = locations(&thick, Rule::MoldThickWall)[0];
        assert!(encloses(&lump, [11.0, 11.0, 3.0]), "{lump:?}");
    }

    #[test]
    fn disc_extremes_match_the_disc_offsets() {
        // 値は決まった擬似乱数列とし、円板の全cellを数える方法と比べる。
        let dims = [9usize, 13usize];
        let values: Vec<u32> = (0..dims[0] * dims[1])
            .map(|i| ((i * 7919 + 13) % 31) as u32)
            .collect();
        for radius in [0.5, 1.5, 2.0, 3.5, 4.2, 20.0] {
            let offsets = disc_offsets(radius);
            for extreme in [Extreme::Max, Extreme::Min] {
                let fast = disc_extreme(&values, dims, radius, extreme);
                for a in 0..dims[0] {
                    for b in 0..dims[1] {
                        let inside = offsets.iter().filter_map(|(da, db)| {
                            let na = a.checked_add_signed(*da).filter(|v| *v < dims[0])?;
                            let nb = b.checked_add_signed(*db).filter(|v| *v < dims[1])?;
                            Some(values[na * dims[1] + nb])
                        });
                        let slow = match extreme {
                            Extreme::Max => inside.max(),
                            Extreme::Min => inside.min(),
                        }
                        .unwrap();
                        assert_eq!(fast[a * dims[1] + b], slow, "radius {radius} at {a},{b}");
                    }
                }
            }
        }
    }

    #[test]
    fn a_tool_too_large_for_the_grid_is_rejected() {
        let mut model = model_with(
            tunnel(),
            Setup {
                plan: milling(1000.0, 30.0, Direction::PlusZ, &[]),
                ..policy(0.5, 0.5, 0.05)
            },
        );
        let error = model.validate().unwrap_err();
        assert!(error.contains("smaller tool"), "{error}");
        // 格子を粗くすれば受理する。
        model.policy.voxel_mm = 0.5;
        model.parts[0].manufacturing[0] = milling(20.0, 30.0, Direction::PlusZ, &[]);
        model.validate().unwrap();
    }

    #[test]
    fn grid_limit_counts_padding() {
        let block = part(vec![add("body", box_shape([0.; 3], [10., 10., 10.]))]);
        // 10 mmを1 mm刻みで覆うと、両端のpaddingを含めて13 cellになる。
        assert_eq!(grid_cells(&block, 1.0), Some(13 * 13 * 13));
        assert_eq!(Grid::rasterize(&block, 1.0).unwrap().size(), [13, 13, 13]);
    }
}
