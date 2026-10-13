//! voxel格子の8節点六面体要素の連立方程式を、幾何マルチグリッドを前処理とする共役勾配法で解く。
//!
//! 格子間隔を2倍ずつにした格子の階層を作る。粗い格子の演算子は、節点の三重線形補間Pによる
//! Galerkin近似P^T A Pである。細かい要素は1つの粗いcellに収まるため、粗い要素の行列は
//! その子の要素の行列をPで写したものの和となる。薄い形状が粗い格子で失われることはない。
//! 平滑化は対角で前処理した行列D⁻¹AのChebyshev多項式で、前後に同じ次数で行い、V-cycleを
//! 対称にする。D⁻¹Aの最大固有値は格子ごとに冪乗法で見積もる。減衰Jacobi法は係数と最大
//! 固有値の積が2を超えると高周波の誤差を増やし、弾性の要素ではその値が3.5〜4となる。
//! 最も粗い格子は密行列のCholesky分解で解く。
//!
//! 固定した自由度 (拘束、固定温度、材料に接しない節点) は方程式から除く。
//!
//! 要素ごとの行列とベクトルの積は、要素をz方向の区間に分けて並列に計算する。隣り合う区間は
//! 節点の面を共有するため、偶数番と奇数番の区間を交互に処理する。同じ組の区間は節点を
//! 共有せず、書き込む範囲が重ならない。

use std::collections::BTreeMap;
use std::ops::Range;

/// 8節点六面体要素の節点の、要素内の位置 (0か1)。
pub(crate) const CORNERS: [[usize; 3]; 8] = [
    [0, 0, 0],
    [1, 0, 0],
    [1, 1, 0],
    [0, 1, 0],
    [0, 0, 1],
    [1, 0, 1],
    [1, 1, 1],
    [0, 1, 1],
];

/// 解の相対残差の目標。
const TOLERANCE: f64 = 1e-8;
/// 共役勾配法の反復の上限。前処理が効いていれば数十回で収束する。
const MAX_ITERATIONS: usize = 2_000;
/// 平滑化のChebyshev多項式の次数。前後それぞれ。
const SMOOTHING: usize = 3;
/// 平滑化で減衰させる固有値の区間の下端。最大固有値に対する比。
const SMOOTHING_RANGE: f64 = 1.0 / 30.0;
/// 冪乗法の見積もりに掛ける余裕。冪乗法は最大固有値を下から近似する。
const EIGENVALUE_MARGIN: f64 = 1.1;
/// 最大固有値を見積もる冪乗法の反復回数。
const POWER_ITERATIONS: usize = 15;
/// 最も粗い格子を密行列で解く自由度の上限。
const DENSE_LIMIT: usize = 1_000;
/// 並列に処理する区間の数の上限。
const MAX_THREADS: usize = 8;
/// 並列に処理する区間あたりの要素数の下限。少なければ並列化の費用が上回る。
const MIN_ELEMENTS_PER_THREAD: usize = 5_000;
/// 密行列で解けない最も粗い格子で、代わりに行う平滑化の次数。
const COARSEST_SMOOTHING: usize = 30;

/// 要素の行列。n×n (nは要素の自由度) を行優先で並べる。
pub(crate) enum Matrices {
    /// すべての要素が同じ行列を持つ。
    Uniform(Vec<f64>),
    /// 要素ごとの行列を順に並べる。
    Each(Vec<f64>),
}

/// 1つの格子の連立方程式。
pub(crate) struct Level {
    /// 節点あたりの自由度。弾性は3、熱伝導は1。
    dofs_per_node: usize,
    /// 節点の格子の各軸の数。cellの数より1多い。
    nodes: [usize; 3],
    /// 要素のcellの位置。
    cells: Vec<[usize; 3]>,
    /// 要素の8節点の番号。
    elements: Vec<[usize; 8]>,
    matrices: Matrices,
    /// 自由度ごとに対角へ加える値 (対流の膜係数)。空なら0。
    extra: Vec<f64>,
    /// 方程式から除く自由度。
    fixed: Vec<bool>,
    diagonal: Vec<f64>,
    /// D⁻¹Aの最大固有値の見積もり。
    largest: f64,
    /// 要素をz方向に区切った区間。要素の番号の範囲と、その要素が触れる節点の番号の範囲。
    slabs: Vec<(Range<usize>, Range<usize>)>,
    /// 1つ粗い格子への転送。自由度を持つ節点と、補間する粗い節点と重み。
    transfer: Vec<Transfer>,
}

/// 細かい節点1つの補間。粗い節点は最大8つで、`count`個が有効である。
struct Transfer {
    node: usize,
    parents: [(usize, f64); 8],
    count: usize,
}

impl Level {
    /// 最も細かい格子。`cells`は材料cellの位置、`fixed`は除く自由度。
    pub(crate) fn new(
        dofs_per_node: usize,
        nodes: [usize; 3],
        cells: Vec<[usize; 3]>,
        matrices: Matrices,
        extra: Vec<f64>,
        fixed: Vec<bool>,
    ) -> Self {
        let elements = cells
            .iter()
            .map(|cell| {
                CORNERS.map(|c| node_index(nodes, [cell[0] + c[0], cell[1] + c[1], cell[2] + c[2]]))
            })
            .collect();
        let mut level = Self {
            dofs_per_node,
            nodes,
            cells,
            elements,
            matrices,
            extra,
            fixed,
            diagonal: Vec::new(),
            largest: 0.0,
            slabs: Vec::new(),
            transfer: Vec::new(),
        };
        level.slabs = level.compute_slabs();
        level.diagonal = level.compute_diagonal();
        level
    }

    /// 要素をz方向の区間に分ける。要素はcellのzの順に並んでいる。並列化しない場合と、
    /// zの順でない場合は1つの区間とする。
    fn compute_slabs(&self) -> Vec<(Range<usize>, Range<usize>)> {
        let all = vec![(0..self.elements.len(), 0..self.nodes.iter().product())];
        let threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
            .min(MAX_THREADS)
            .min(self.elements.len() / MIN_ELEMENTS_PER_THREAD);
        if threads <= 1 || self.cells.windows(2).any(|pair| pair[0][2] > pair[1][2]) {
            return all;
        }
        let count = 2 * threads;
        let plane = self.nodes[0] * self.nodes[1];
        let mut slabs = Vec::with_capacity(count);
        let mut start = 0;
        for k in 1..=count {
            let target = self.elements.len() * k / count;
            let mut end = target.max(start + 1).min(self.elements.len());
            // 区間の境界はzの層の境界に合わせる。
            while end < self.elements.len() && self.cells[end][2] == self.cells[end - 1][2] {
                end += 1;
            }
            if start >= end {
                continue;
            }
            let low = self.cells[start][2];
            let high = self.cells[end - 1][2] + 1;
            slabs.push((start..end, low * plane..(high + 1) * plane));
            start = end;
            if start == self.elements.len() {
                break;
            }
        }
        // 1つおきの区間が節点を共有しないことを確かめる。
        let separated = slabs.windows(3).all(|w| w[0].1.end <= w[2].1.start);
        if slabs.len() < 2 || !separated {
            return all;
        }
        slabs
    }

    fn size(&self) -> usize {
        self.dofs_per_node * 8
    }

    fn dofs(&self) -> usize {
        self.fixed.len()
    }

    fn matrix(&self, element: usize) -> &[f64] {
        let n = self.size() * self.size();
        match &self.matrices {
            Matrices::Uniform(matrix) => matrix,
            Matrices::Each(matrices) => &matrices[element * n..(element + 1) * n],
        }
    }

    /// 要素の局所自由度 (隅a、成分c) の全体の番号。
    fn dof(&self, element: &[usize; 8], local: usize) -> usize {
        let d = self.dofs_per_node;
        d * element[local / d] + local % d
    }

    fn compute_diagonal(&self) -> Vec<f64> {
        let mut diagonal = vec![0.0; self.dofs()];
        let n = self.size();
        for (e, element) in self.elements.iter().enumerate() {
            let matrix = self.matrix(e);
            for i in 0..n {
                diagonal[self.dof(element, i)] += matrix[i * n + i];
            }
        }
        for (value, extra) in diagonal.iter_mut().zip(&self.extra) {
            *value += extra;
        }
        for (value, fixed) in diagonal.iter_mut().zip(&self.fixed) {
            if *fixed {
                *value = 0.0;
            }
        }
        diagonal
    }

    /// 要素の範囲について`y += A x`を加える。`y`は全体の自由度の`offset`からの部分である。
    /// `masked`なら除いた自由度のxを0とみなす。
    fn accumulate(
        &self,
        x: &[f64],
        y: &mut [f64],
        offset: usize,
        elements: Range<usize>,
        masked: bool,
    ) {
        let n = self.size();
        let mut local = [0.0; 24];
        for e in elements {
            let element = &self.elements[e];
            for (i, slot) in local[..n].iter_mut().enumerate() {
                let dof = self.dof(element, i);
                *slot = if masked && self.fixed[dof] {
                    0.0
                } else {
                    x[dof]
                };
            }
            let matrix = self.matrix(e);
            for i in 0..n {
                let row = &matrix[i * n..(i + 1) * n];
                let value: f64 = row.iter().zip(&local[..n]).map(|(k, u)| k * u).sum();
                y[self.dof(element, i) - offset] += value;
            }
        }
    }

    /// すべての要素について`y += A x`を加える。区間を偶数番と奇数番に分けて並列に処理する。
    fn accumulate_all(&self, x: &[f64], y: &mut [f64], masked: bool) {
        if self.slabs.len() <= 1 {
            self.accumulate(x, y, 0, 0..self.elements.len(), masked);
            return;
        }
        let d = self.dofs_per_node;
        for parity in 0..2 {
            std::thread::scope(|scope| {
                let mut rest: &mut [f64] = y;
                let mut consumed = 0;
                for (elements, nodes) in self.slabs.iter().skip(parity).step_by(2) {
                    let (start, end) = (d * nodes.start, d * nodes.end);
                    let (_, tail) = std::mem::take(&mut rest).split_at_mut(start - consumed);
                    let (chunk, tail) = tail.split_at_mut(end - start);
                    rest = tail;
                    consumed = end;
                    let elements = elements.clone();
                    scope.spawn(move || self.accumulate(x, chunk, start, elements, masked));
                }
            });
        }
    }

    /// 除いた自由度を0とみなした`y = A x`。除いた自由度のyは0とする。
    pub(crate) fn apply(&self, x: &[f64], y: &mut [f64]) {
        y.fill(0.0);
        self.accumulate_all(x, y, true);
        for (i, extra) in self.extra.iter().enumerate() {
            y[i] += extra * x[i];
        }
        for (value, fixed) in y.iter_mut().zip(&self.fixed) {
            if *fixed {
                *value = 0.0;
            }
        }
    }

    /// 自由度を持つ節点ごとに、補間する粗い節点と重みを求めておく。
    fn compute_transfer(&mut self, coarse: [usize; 3]) {
        let d = self.dofs_per_node;
        let per_axis = |v: usize| -> ([(usize, f64); 2], usize) {
            if v.is_multiple_of(2) {
                ([(v / 2, 1.0), (0, 0.0)], 1)
            } else {
                ([((v - 1) / 2, 0.5), (v.div_ceil(2), 0.5)], 2)
            }
        };
        let [nx, ny, nz] = self.nodes;
        let mut transfer = Vec::new();
        for z in 0..nz {
            for y in 0..ny {
                for x in 0..nx {
                    let node = node_index(self.nodes, [x, y, z]);
                    if (0..d).all(|c| self.fixed[d * node + c]) {
                        continue;
                    }
                    let (px, cx) = per_axis(x);
                    let (py, cy) = per_axis(y);
                    let (pz, cz) = per_axis(z);
                    let mut parents = [(0, 0.0); 8];
                    let mut count = 0;
                    for (zz, wz) in &pz[..cz] {
                        for (yy, wy) in &py[..cy] {
                            for (xx, wx) in &px[..cx] {
                                parents[count] =
                                    (node_index(coarse, [*xx, *yy, *zz]), wx * wy * wz);
                                count += 1;
                            }
                        }
                    }
                    transfer.push(Transfer {
                        node,
                        parents,
                        count,
                    });
                }
            }
        }
        self.transfer = transfer;
    }

    /// Galerkin近似による粗い格子。粗くできなければNone。
    // 要素の行列の積は、隅と成分の番号の算術で書く方が対応を追いやすい。
    #[allow(clippy::needless_range_loop)]
    fn coarsen(&self) -> Option<Level> {
        let cells = self.nodes.map(|n| n - 1);
        if cells.iter().all(|c| *c <= 1) {
            return None;
        }
        let coarse_cells = cells.map(|c| c.div_ceil(2));
        let coarse_nodes = coarse_cells.map(|c| c + 1);
        let d = self.dofs_per_node;
        let n = self.size();

        // 子の位置ごとの補間の重み。weights[child][a][b]は細かい隅aの、粗い隅bの重み。
        let weight = |offset: usize, corner: usize| match (offset, corner) {
            (0, 0) | (2, 1) => 1.0,
            (1, _) => 0.5,
            _ => 0.0,
        };
        let weights: Vec<[[f64; 8]; 8]> = (0..8)
            .map(|child| {
                let child = [child & 1, (child >> 1) & 1, (child >> 2) & 1];
                std::array::from_fn(|a| {
                    std::array::from_fn(|b| {
                        (0..3)
                            .map(|axis| weight(child[axis] + CORNERS[a][axis], CORNERS[b][axis]))
                            .product()
                    })
                })
            })
            .collect();

        // 対角へ加える値は、その節点を含む最初の要素の行列に含める。
        let mut owner = vec![usize::MAX; self.nodes.iter().product()];
        if !self.extra.is_empty() {
            for (e, element) in self.elements.iter().enumerate() {
                for node in element {
                    if owner[*node] == usize::MAX {
                        owner[*node] = e;
                    }
                }
            }
        }

        let mut index: BTreeMap<[usize; 3], usize> = BTreeMap::new();
        let mut coarse_list: Vec<[usize; 3]> = Vec::new();
        let mut matrices: Vec<f64> = Vec::new();
        let mut fine = vec![0.0; n * n];
        let mut product = vec![0.0; n * n];
        for (e, element) in self.elements.iter().enumerate() {
            let cell = self.cells[e];
            let parent = cell.map(|v| v / 2);
            let child = (cell[0] % 2) | ((cell[1] % 2) << 1) | ((cell[2] % 2) << 2);
            let k = *index.entry(parent).or_insert_with(|| {
                coarse_list.push(parent);
                matrices.extend(std::iter::repeat_n(0.0, n * n));
                coarse_list.len() - 1
            });
            // 除いた自由度の行と列を0にした要素の行列。
            fine.copy_from_slice(self.matrix(e));
            for i in 0..n {
                let dof = self.dof(element, i);
                if !self.extra.is_empty() && owner[element[i / d]] == e {
                    fine[i * n + i] += self.extra[dof];
                }
                if self.fixed[dof] {
                    for j in 0..n {
                        fine[i * n + j] = 0.0;
                        fine[j * n + i] = 0.0;
                    }
                }
            }
            let w = &weights[child];
            // product = fine · (W ⊗ I)
            for i in 0..n {
                for b in 0..8 {
                    for c in 0..d {
                        product[i * n + b * d + c] =
                            (0..8).map(|a| fine[i * n + a * d + c] * w[a][b]).sum();
                    }
                }
            }
            // coarse += (W ⊗ I)^T · product
            let target = &mut matrices[k * n * n..(k + 1) * n * n];
            for b in 0..8 {
                for c in 0..d {
                    let row = b * d + c;
                    for col in 0..n {
                        let value: f64 = (0..8)
                            .map(|a| w[a][b] * product[(a * d + c) * n + col])
                            .sum();
                        target[row * n + col] += value;
                    }
                }
            }
        }

        let dofs = d * coarse_nodes.iter().product::<usize>();
        let mut coarse = Level::new(
            d,
            coarse_nodes,
            coarse_list,
            Matrices::Each(matrices),
            Vec::new(),
            vec![false; dofs],
        );
        // 細かい格子のどの自由度にも補間されない自由度を除く。
        let scale = coarse.diagonal.iter().copied().fold(0.0, f64::max);
        let fixed: Vec<bool> = coarse
            .diagonal
            .iter()
            .map(|value| *value <= 1e-12 * scale)
            .collect();
        coarse.fixed = fixed;
        coarse.diagonal = coarse.compute_diagonal();
        Some(coarse)
    }

    /// 細かい格子の残差を粗い格子へ写す。r_c = P^T r。
    fn restrict(&self, coarse: &Level, residual: &[f64], out: &mut [f64]) {
        out.fill(0.0);
        let d = self.dofs_per_node;
        for t in &self.transfer {
            for c in 0..d {
                let dof = d * t.node + c;
                if self.fixed[dof] {
                    continue;
                }
                for (parent, w) in &t.parents[..t.count] {
                    out[d * parent + c] += w * residual[dof];
                }
            }
        }
        for (value, fixed) in out.iter_mut().zip(&coarse.fixed) {
            if *fixed {
                *value = 0.0;
            }
        }
    }

    /// 粗い格子の修正を細かい格子へ補間して加える。x += P e。
    fn prolong_add(&self, coarse: &Level, correction: &[f64], x: &mut [f64]) {
        let d = self.dofs_per_node;
        for t in &self.transfer {
            for c in 0..d {
                let dof = d * t.node + c;
                if self.fixed[dof] {
                    continue;
                }
                for (parent, w) in &t.parents[..t.count] {
                    let coarse_dof = d * parent + c;
                    if !coarse.fixed[coarse_dof] {
                        x[dof] += w * correction[coarse_dof];
                    }
                }
            }
        }
    }

    /// `D⁻¹ (rhs - A x)`を`out`へ書く。
    fn scaled_residual(&self, rhs: &[f64], x: &[f64], scratch: &mut [f64], out: &mut [f64]) {
        self.apply(x, scratch);
        for i in 0..x.len() {
            out[i] = if self.fixed[i] || self.diagonal[i] <= 0.0 {
                0.0
            } else {
                (rhs[i] - scratch[i]) / self.diagonal[i]
            };
        }
    }

    /// D⁻¹Aの最大固有値を冪乗法で見積もり、余裕を掛けて保持する。初期ベクトルは決定的に作る。
    fn estimate_largest(&mut self) {
        let n = self.dofs();
        let mut v: Vec<f64> = (0..n)
            .map(|i| {
                if self.fixed[i] {
                    0.0
                } else {
                    1.0 + ((i * 7919) % 1009) as f64 / 1009.0
                }
            })
            .collect();
        let mut w = vec![0.0; n];
        let mut largest = 0.0;
        for _ in 0..POWER_ITERATIONS {
            let norm = v.iter().map(|x| x * x).sum::<f64>().sqrt();
            if norm == 0.0 {
                break;
            }
            for value in v.iter_mut() {
                *value /= norm;
            }
            self.apply(&v, &mut w);
            for (value, diagonal) in w.iter_mut().zip(&self.diagonal) {
                *value = if *diagonal > 0.0 {
                    *value / diagonal
                } else {
                    0.0
                };
            }
            largest = w.iter().map(|x| x * x).sum::<f64>().sqrt();
            std::mem::swap(&mut v, &mut w);
        }
        self.largest = EIGENVALUE_MARGIN * largest;
    }

    /// D⁻¹AのChebyshev多項式による平滑化。区間[λ/30, λ]の固有値の誤差を減衰させる。
    fn smooth(&self, rhs: &[f64], x: &mut [f64], scratch: &mut [f64], degree: usize) {
        if self.largest <= 0.0 {
            return;
        }
        let n = x.len();
        let upper = self.largest;
        let lower = upper * SMOOTHING_RANGE;
        let theta = (upper + lower) / 2.0;
        let delta = (upper - lower) / 2.0;
        let sigma = theta / delta;
        let mut rho = 1.0 / sigma;
        let mut residual = vec![0.0; n];
        self.scaled_residual(rhs, x, scratch, &mut residual);
        let mut step: Vec<f64> = residual.iter().map(|r| r / theta).collect();
        for _ in 1..degree {
            for i in 0..n {
                x[i] += step[i];
            }
            self.scaled_residual(rhs, x, scratch, &mut residual);
            let next = 1.0 / (2.0 * sigma - rho);
            for i in 0..n {
                step[i] = next * rho * step[i] + 2.0 * next / delta * residual[i];
            }
            rho = next;
        }
        for i in 0..n {
            x[i] += step[i];
        }
    }
}

fn node_index(nodes: [usize; 3], xyz: [usize; 3]) -> usize {
    (xyz[2] * nodes[1] + xyz[1]) * nodes[0] + xyz[0]
}

/// 最も粗い格子の密行列のCholesky分解。
struct Dense {
    /// 方程式に残る自由度。
    dofs: Vec<usize>,
    /// 下三角の分解L (行優先)。
    factor: Vec<f64>,
    /// 分解で消えた (ピボットが正でない) 自由度。前処理では0とする。
    dropped: Vec<bool>,
}

impl Dense {
    fn new(level: &Level) -> Self {
        let dofs: Vec<usize> = (0..level.dofs()).filter(|i| !level.fixed[*i]).collect();
        let m = dofs.len();
        let mut position = vec![usize::MAX; level.dofs()];
        for (k, dof) in dofs.iter().enumerate() {
            position[*dof] = k;
        }
        let mut a = vec![0.0; m * m];
        let n = level.size();
        for (e, element) in level.elements.iter().enumerate() {
            let matrix = level.matrix(e);
            for i in 0..n {
                let row = position[level.dof(element, i)];
                if row == usize::MAX {
                    continue;
                }
                for j in 0..n {
                    let col = position[level.dof(element, j)];
                    if col != usize::MAX {
                        a[row * m + col] += matrix[i * n + j];
                    }
                }
            }
        }
        for (k, dof) in dofs.iter().enumerate() {
            if let Some(extra) = level.extra.get(*dof) {
                a[k * m + k] += extra;
            }
        }
        let original: Vec<f64> = (0..m).map(|k| a[k * m + k]).collect();
        let mut dropped = vec![false; m];
        for j in 0..m {
            let mut pivot = a[j * m + j];
            for k in 0..j {
                pivot -= a[j * m + k] * a[j * m + k];
            }
            if pivot <= 1e-12 * original[j] || pivot <= 0.0 {
                dropped[j] = true;
                for k in 0..j {
                    a[j * m + k] = 0.0;
                }
                a[j * m + j] = 1.0;
                for i in j + 1..m {
                    a[i * m + j] = 0.0;
                }
                continue;
            }
            let pivot = pivot.sqrt();
            a[j * m + j] = pivot;
            for i in j + 1..m {
                let mut value = a[i * m + j];
                for k in 0..j {
                    value -= a[i * m + k] * a[j * m + k];
                }
                a[i * m + j] = value / pivot;
            }
        }
        Self {
            dofs,
            factor: a,
            dropped,
        }
    }

    fn solve(&self, rhs: &[f64], x: &mut [f64]) {
        let m = self.dofs.len();
        let mut y: Vec<f64> = self.dofs.iter().map(|dof| rhs[*dof]).collect();
        for (k, dropped) in self.dropped.iter().enumerate() {
            if *dropped {
                y[k] = 0.0;
            }
        }
        // L z = y、L^T x = zの順に解く。
        for i in 0..m {
            let row = &self.factor[i * m..i * m + i];
            let value = y[i] - row.iter().zip(&y[..i]).map(|(l, z)| l * z).sum::<f64>();
            y[i] = value / self.factor[i * m + i];
        }
        for i in (0..m).rev() {
            let value = y[i]
                - (i + 1..m)
                    .map(|k| self.factor[k * m + i] * y[k])
                    .sum::<f64>();
            y[i] = value / self.factor[i * m + i];
        }
        x.fill(0.0);
        for (k, dof) in self.dofs.iter().enumerate() {
            if !self.dropped[k] {
                x[*dof] = y[k];
            }
        }
    }
}

/// 格子の階層と、最も粗い格子の解法。
pub(crate) struct Hierarchy {
    levels: Vec<Level>,
    dense: Option<Dense>,
}

impl Hierarchy {
    pub(crate) fn new(fine: Level) -> Self {
        Self::with_dense_limit(fine, DENSE_LIMIT)
    }

    /// 自由度が`limit`以下になるまで粗くする。
    fn with_dense_limit(fine: Level, limit: usize) -> Self {
        let mut levels = vec![fine];
        loop {
            let last = levels.last().expect("non-empty");
            let active = last.fixed.iter().filter(|f| !**f).count();
            if active <= limit {
                break;
            }
            match last.coarsen() {
                Some(coarse) if coarse.dofs() < last.dofs() => levels.push(coarse),
                _ => break,
            }
        }
        for k in 0..levels.len() {
            levels[k].estimate_largest();
            if k + 1 < levels.len() {
                let coarse = levels[k + 1].nodes;
                levels[k].compute_transfer(coarse);
            }
        }
        let last = levels.last().expect("non-empty");
        let active = last.fixed.iter().filter(|f| !**f).count();
        let dense = (active <= limit).then(|| Dense::new(last));
        Self { levels, dense }
    }

    pub(crate) fn fine(&self) -> &Level {
        &self.levels[0]
    }

    /// V-cycleを1回行い、`rhs`に対する近似解を`x`へ書く。
    fn cycle(&self, depth: usize, rhs: &[f64], x: &mut [f64]) {
        let level = &self.levels[depth];
        x.fill(0.0);
        if depth + 1 == self.levels.len() {
            match &self.dense {
                Some(dense) => dense.solve(rhs, x),
                None => {
                    let mut scratch = vec![0.0; x.len()];
                    level.smooth(rhs, x, &mut scratch, COARSEST_SMOOTHING);
                    level.smooth(rhs, x, &mut scratch, COARSEST_SMOOTHING);
                }
            }
            return;
        }
        let mut scratch = vec![0.0; x.len()];
        level.smooth(rhs, x, &mut scratch, SMOOTHING);
        level.apply(x, &mut scratch);
        let residual: Vec<f64> = (0..x.len()).map(|i| rhs[i] - scratch[i]).collect();
        let coarse = &self.levels[depth + 1];
        let mut coarse_rhs = vec![0.0; coarse.dofs()];
        level.restrict(coarse, &residual, &mut coarse_rhs);
        let mut correction = vec![0.0; coarse.dofs()];
        self.cycle(depth + 1, &coarse_rhs, &mut correction);
        level.prolong_add(coarse, &correction, x);
        level.smooth(rhs, x, &mut scratch, SMOOTHING);
        for (value, fixed) in x.iter_mut().zip(&level.fixed) {
            if *fixed {
                *value = 0.0;
            }
        }
    }

    /// `A x = rhs`を解く。除いた自由度は`x`の初期値のまま動かさず、その寄与を右辺へ移す。
    /// 収束すれば反復回数を返す。
    pub(crate) fn solve(&self, rhs: &[f64], x: &mut [f64]) -> Option<usize> {
        let level = self.fine();
        let n = rhs.len();
        let dot = |a: &[f64], b: &[f64]| a.iter().zip(b).map(|(p, q)| p * q).sum::<f64>();
        // 除いた自由度の値による寄与。applyは除いた自由度を0とみなすため、全体の行列を
        // 要素ごとに掛けて求める。
        let mut known = vec![0.0; n];
        let mut lifted = vec![0.0; n];
        for i in 0..n {
            if level.fixed[i] {
                known[i] = x[i];
            }
        }
        level.apply_full(&known, &mut lifted);
        let mut residual: Vec<f64> = (0..n)
            .map(|i| {
                if level.fixed[i] {
                    0.0
                } else {
                    rhs[i] - lifted[i]
                }
            })
            .collect();
        let mut free = vec![0.0; n];
        for i in 0..n {
            if !level.fixed[i] {
                free[i] = x[i];
            }
        }
        let mut product = vec![0.0; n];
        level.apply(&free, &mut product);
        for i in 0..n {
            residual[i] -= product[i];
        }
        let scale = (0..n)
            .filter(|i| !level.fixed[*i])
            .map(|i| (rhs[i] - lifted[i]).powi(2))
            .sum::<f64>()
            .sqrt()
            .max(f64::MIN_POSITIVE);
        let mut z = vec![0.0; n];
        self.cycle(0, &residual, &mut z);
        let mut direction = z.clone();
        let mut rz = dot(&residual, &z);
        for iteration in 0..MAX_ITERATIONS {
            if dot(&residual, &residual).sqrt() <= TOLERANCE * scale {
                for i in 0..n {
                    if !level.fixed[i] {
                        x[i] = free[i];
                    }
                }
                return Some(iteration);
            }
            level.apply(&direction, &mut product);
            let curvature = dot(&direction, &product);
            if curvature <= 0.0 || !curvature.is_finite() {
                return None;
            }
            let alpha = rz / curvature;
            for i in 0..n {
                free[i] += alpha * direction[i];
                residual[i] -= alpha * product[i];
            }
            self.cycle(0, &residual, &mut z);
            let next = dot(&residual, &z);
            let beta = next / rz;
            rz = next;
            for i in 0..n {
                direction[i] = z[i] + beta * direction[i];
            }
        }
        None
    }
}

impl Level {
    /// 除いた自由度も含む`y = A x`。固定した値の右辺への寄与を求めるために使う。
    fn apply_full(&self, x: &[f64], y: &mut [f64]) {
        y.fill(0.0);
        self.accumulate_all(x, y, false);
        for (i, extra) in self.extra.iter().enumerate() {
            y[i] += extra * x[i];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一辺1、熱伝導率1の要素の伝導行列。
    fn conduction() -> Vec<f64> {
        let mut k = vec![0.0; 64];
        let a = 0.5 - 0.5 / 3f64.sqrt();
        let b = 0.5 + 0.5 / 3f64.sqrt();
        for g in 0..8 {
            let xi = [
                if g & 1 == 0 { a } else { b },
                if g & 2 == 0 { a } else { b },
                if g & 4 == 0 { a } else { b },
            ];
            let gradient = |corner: [usize; 3]| -> [f64; 3] {
                let f = |axis: usize| {
                    if corner[axis] == 1 {
                        xi[axis]
                    } else {
                        1.0 - xi[axis]
                    }
                };
                let s = |axis: usize| if corner[axis] == 1 { 1.0 } else { -1.0 };
                [s(0) * f(1) * f(2), f(0) * s(1) * f(2), f(0) * f(1) * s(2)]
            };
            for i in 0..8 {
                for j in 0..8 {
                    let (gi, gj) = (gradient(CORNERS[i]), gradient(CORNERS[j]));
                    k[i * 8 + j] += (0..3).map(|axis| gi[axis] * gj[axis]).sum::<f64>() / 8.0;
                }
            }
        }
        k
    }

    #[test]
    fn multigrid_matches_the_linear_temperature_field() {
        // 長さ40 cellの棒の両端の温度を固定すると、温度は長さ方向に線形となる。粗くすると
        // 奇数のcell数を含む格子になる。
        let cells_per_axis = [40, 5, 3];
        let nodes = cells_per_axis.map(|c| c + 1);
        let mut cells = Vec::new();
        for z in 0..cells_per_axis[2] {
            for y in 0..cells_per_axis[1] {
                for x in 0..cells_per_axis[0] {
                    cells.push([x, y, z]);
                }
            }
        }
        let count: usize = nodes.iter().product();
        let mut fixed = vec![false; count];
        let mut x = vec![0.0; count];
        for z in 0..nodes[2] {
            for y in 0..nodes[1] {
                for (end, value) in [(0, 10.0), (nodes[0] - 1, 50.0)] {
                    let node = node_index(nodes, [end, y, z]);
                    fixed[node] = true;
                    x[node] = value;
                }
            }
        }
        let level = Level::new(
            1,
            nodes,
            cells,
            Matrices::Uniform(conduction()),
            Vec::new(),
            fixed,
        );
        let hierarchy = Hierarchy::with_dense_limit(level, 50);
        assert!(hierarchy.levels.len() >= 3);
        let iterations = hierarchy.solve(&vec![0.0; count], &mut x).unwrap();
        assert!(iterations < 30, "{iterations}");
        for z in 0..nodes[2] {
            for y in 0..nodes[1] {
                for xi in 0..nodes[0] {
                    let expected = 10.0 + 40.0 * xi as f64 / 40.0;
                    let value = x[node_index(nodes, [xi, y, z])];
                    assert!((value - expected).abs() < 1e-6, "{value} {expected}");
                }
            }
        }
    }
}
