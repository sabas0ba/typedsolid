//! 八分木の格子の8節点六面体要素の連立方程式を、幾何マルチグリッドを前処理とする共役勾配法で
//! 解く。
//!
//! 要素は大きさの異なる立方体のcellで、隣より大きいcellの面や辺に乗る節点 (hanging node) は
//! 大きいcellの隅の値で補間し、未知数から除く (`octree`)。要素の行列は隅の自由度について
//! 持ち、積をとるときに従属する隅を主節点へ展開する。
//!
//! 粗い格子は、最も小さい要素を8つずつ親のcellへまとめて作る。それより大きい要素はそのまま
//! 残す。粗い格子の演算子は、粗い格子の関数を細かい節点で評価する補間PによるGalerkin近似
//! P^T A Pである。まとめた親の要素の行列は、子の要素の行列を親の三重線形補間で写したものの
//! 和となる。薄い形状が粗い格子で失われることはない。一様な格子ではすべての要素を毎回まとめる。
//! 拘束した自由度に接する要素は行列から拘束の行と列を除いて写す。従属する隅は、その主節点が
//! すべて拘束されていれば拘束とみなす。粗い格子の演算子はこの点で近似となるが、前処理に使う
//! だけであり、解の精度は細かい格子の演算子で決まる。
//!
//! 平滑化は対角で前処理した行列D⁻¹AのChebyshev多項式で、前後に同じ次数で行い、V-cycleを
//! 対称にする。D⁻¹Aの最大固有値は格子ごとに冪乗法で見積もる。減衰Jacobi法は係数と最大
//! 固有値の積が2を超えると高周波の誤差を増やし、弾性の要素ではその値が3.5〜4となる。
//! 最も粗い格子は密行列のCholesky分解で解く。
//!
//! 固定した自由度 (拘束、固定温度、材料に接しない節点) と従属する節点の自由度は方程式から除く。
//!
//! 要素ごとの行列とベクトルの積は、要素をz方向の区間に分けて並列に計算する。偶数番と奇数番の
//! 区間を交互に処理し、同じ組の区間が書き込む節点の範囲が重ならないことを確かめる。

use crate::octree::{self, Cell, Constraints, Locator, corner};
use std::collections::{BTreeMap, HashMap};
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

/// 格子の形。節点と要素のcellは格子座標で表す。
pub(crate) struct Geometry {
    /// 節点の格子座標。zの順に並ぶ。
    pub(crate) positions: Vec<[i64; 3]>,
    /// 要素のcell。最小の隅のzの順に並ぶ。
    pub(crate) cells: Vec<Cell>,
    /// 要素の8隅の節点の番号。
    pub(crate) elements: Vec<[usize; 8]>,
    pub(crate) constraints: Constraints,
}

/// 1つの格子の連立方程式。
pub(crate) struct Level {
    /// 節点あたりの自由度。弾性は3、熱伝導は1。
    dofs_per_node: usize,
    geometry: Geometry,
    /// 要素の行列の表。n×n (nは要素の自由度) を行優先で並べる。
    table: Vec<Vec<f64>>,
    /// 要素ごとの、表の番号と倍率。
    assigned: Vec<(usize, f64)>,
    /// 自由度ごとに対角へ加える値 (対流の膜係数)。空なら0。
    extra: Vec<f64>,
    /// 方程式から除く自由度。従属する節点の自由度を含む。
    fixed: Vec<bool>,
    /// 従属する隅を持つ要素の、(隅, 主節点, 重み)の範囲の始まり。要素の数より1多い。
    link_start: Vec<usize>,
    links: Vec<(usize, usize, f64)>,
    diagonal: Vec<f64>,
    /// D⁻¹Aの最大固有値の見積もり。
    largest: f64,
    /// 要素をz方向に区切った区間。要素の番号の範囲と、その要素が書き込む節点の番号の範囲。
    slabs: Vec<(Range<usize>, Range<usize>)>,
    /// 1つ粗い格子への転送。
    transfer: Transfer,
}

/// 細かい節点の、粗い格子の主節点による補間。
#[derive(Default)]
struct Transfer {
    nodes: Vec<usize>,
    /// 節点ごとの`parents`の範囲の始まり。`nodes`より1多い。
    start: Vec<usize>,
    parents: Vec<(usize, f64)>,
}

impl Level {
    /// 格子の連立方程式。`fixed`は除く自由度で、従属する節点の自由度は加えて除く。
    pub(crate) fn new(
        dofs_per_node: usize,
        geometry: Geometry,
        table: Vec<Vec<f64>>,
        assigned: Vec<(usize, f64)>,
        extra: Vec<f64>,
        mut fixed: Vec<bool>,
    ) -> Self {
        let mut link_start = Vec::with_capacity(geometry.elements.len() + 1);
        let mut links = Vec::new();
        for element in &geometry.elements {
            link_start.push(links.len());
            if element
                .iter()
                .any(|node| geometry.constraints.is_dependent(*node))
            {
                for (a, node) in element.iter().enumerate() {
                    for (master, weight) in geometry.constraints.expand(*node) {
                        links.push((a, master, weight));
                    }
                }
            }
        }
        link_start.push(links.len());
        for node in 0..geometry.positions.len() {
            if geometry.constraints.is_dependent(node) {
                for c in 0..dofs_per_node {
                    fixed[dofs_per_node * node + c] = true;
                }
            }
        }
        let mut level = Self {
            dofs_per_node,
            geometry,
            table,
            assigned,
            extra,
            fixed,
            link_start,
            links,
            diagonal: Vec::new(),
            largest: 0.0,
            slabs: Vec::new(),
            transfer: Transfer::default(),
        };
        level.slabs = level.compute_slabs();
        level.diagonal = level.compute_diagonal();
        level
    }

    pub(crate) fn geometry(&self) -> &Geometry {
        &self.geometry
    }

    fn links(&self, element: usize) -> &[(usize, usize, f64)] {
        &self.links[self.link_start[element]..self.link_start[element + 1]]
    }

    /// 要素が書き込む節点の番号の範囲。
    fn touched(&self, element: usize) -> Range<usize> {
        let links = self.links(element);
        let nodes: Box<dyn Iterator<Item = usize>> = if links.is_empty() {
            Box::new(self.geometry.elements[element].iter().copied())
        } else {
            Box::new(links.iter().map(|(_, m, _)| *m))
        };
        let (low, high) = nodes.fold((usize::MAX, 0), |(lo, hi), n| (lo.min(n), hi.max(n)));
        low..high + 1
    }

    /// 要素をz方向の区間に分ける。要素はcellのzの順に並んでいる。並列化しない場合と、
    /// 同じ組の区間が節点を共有する場合は1つの区間とする。
    fn compute_slabs(&self) -> Vec<(Range<usize>, Range<usize>)> {
        let cells = &self.geometry.cells;
        let count = cells.len();
        let all = vec![(0..count, 0..self.geometry.positions.len())];
        let threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
            .min(MAX_THREADS)
            .min(count / MIN_ELEMENTS_PER_THREAD);
        if threads <= 1 || cells.windows(2).any(|pair| pair[0].0[2] > pair[1].0[2]) {
            return all;
        }
        let pieces = 2 * threads;
        let mut slabs = Vec::with_capacity(pieces);
        let mut start = 0;
        for k in 1..=pieces {
            let target = count * k / pieces;
            let mut end = target.max(start + 1).min(count);
            // 区間の境界はzの層の境界に合わせる。
            while end < count && cells[end].0[2] == cells[end - 1].0[2] {
                end += 1;
            }
            if start >= end {
                continue;
            }
            let (low, high) = (start..end).fold((usize::MAX, 0), |(low, high), e| {
                let touched = self.touched(e);
                (low.min(touched.start), high.max(touched.end))
            });
            slabs.push((start..end, low..high));
            start = end;
            if start == count {
                break;
            }
        }
        // 1つおきの区間が節点を共有しないことを確かめる。
        let separated = slabs.windows(3).all(|w| w[0].1.end <= w[2].1.start)
            && slabs.windows(2).all(|w| w[0].1.start <= w[1].1.start);
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

    fn matrix(&self, element: usize) -> (&[f64], f64) {
        let (index, scale) = self.assigned[element];
        (&self.table[index], scale)
    }

    /// 要素の局所自由度 (隅a、成分c) の全体の番号。
    fn dof(&self, element: &[usize; 8], local: usize) -> usize {
        let d = self.dofs_per_node;
        d * element[local / d] + local % d
    }

    fn compute_diagonal(&self) -> Vec<f64> {
        let mut diagonal = vec![0.0; self.dofs()];
        let n = self.size();
        let d = self.dofs_per_node;
        for (e, element) in self.geometry.elements.iter().enumerate() {
            let (matrix, scale) = self.matrix(e);
            let links = self.links(e);
            if links.is_empty() {
                for i in 0..n {
                    diagonal[self.dof(element, i)] += scale * matrix[i * n + i];
                }
                continue;
            }
            for &(a, m1, w1) in links {
                for &(b, m2, w2) in links {
                    if m1 != m2 {
                        continue;
                    }
                    for c in 0..d {
                        diagonal[d * m1 + c] +=
                            scale * w1 * w2 * matrix[(a * d + c) * n + b * d + c];
                    }
                }
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
    /// `masked`なら除いた自由度のxを0とみなす。従属する節点のxは使わず、主節点から補間する。
    fn accumulate(
        &self,
        x: &[f64],
        y: &mut [f64],
        offset: usize,
        elements: Range<usize>,
        masked: bool,
    ) {
        let n = self.size();
        let d = self.dofs_per_node;
        let value = |dof: usize| {
            if masked && self.fixed[dof] {
                0.0
            } else {
                x[dof]
            }
        };
        let mut local = [0.0; 24];
        let mut product = [0.0; 24];
        for e in elements {
            let element = &self.geometry.elements[e];
            let links = self.links(e);
            if links.is_empty() {
                for (i, slot) in local[..n].iter_mut().enumerate() {
                    *slot = value(self.dof(element, i));
                }
            } else {
                local[..n].fill(0.0);
                for &(a, m, w) in links {
                    for c in 0..d {
                        local[a * d + c] += w * value(d * m + c);
                    }
                }
            }
            let (matrix, scale) = self.matrix(e);
            for i in 0..n {
                let row = &matrix[i * n..(i + 1) * n];
                product[i] = scale * row.iter().zip(&local[..n]).map(|(k, u)| k * u).sum::<f64>();
            }
            if links.is_empty() {
                for (i, p) in product[..n].iter().enumerate() {
                    y[self.dof(element, i) - offset] += p;
                }
            } else {
                for &(a, m, w) in links {
                    for c in 0..d {
                        y[d * m + c - offset] += w * product[a * d + c];
                    }
                }
            }
        }
    }

    /// すべての要素について`y += A x`を加える。区間を偶数番と奇数番に分けて並列に処理する。
    fn accumulate_all(&self, x: &[f64], y: &mut [f64], masked: bool) {
        if self.slabs.len() <= 1 {
            self.accumulate(x, y, 0, 0..self.geometry.elements.len(), masked);
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

    /// 自由度を持つ節点ごとに、粗い格子の関数をその節点で評価する主節点と重みを求めておく。
    fn compute_transfer(&mut self, coarse: &Level) {
        let d = self.dofs_per_node;
        let locator = Locator::new(&coarse.geometry.cells);
        let mut transfer = Transfer::default();
        let mut found = Vec::new();
        for (node, point) in self.geometry.positions.iter().enumerate() {
            if (0..d).all(|c| self.fixed[d * node + c]) {
                continue;
            }
            locator.containing(&coarse.geometry.cells, *point, &mut found);
            let Some(&k) = found.first() else {
                continue;
            };
            let cell = &coarse.geometry.cells[k];
            let t: [f64; 3] =
                std::array::from_fn(|axis| (point[axis] - cell.0[axis]) as f64 / cell.1 as f64);
            transfer.nodes.push(node);
            transfer.start.push(transfer.parents.len());
            let first = transfer.parents.len();
            for (a, coarse_node) in coarse.geometry.elements[k].iter().enumerate() {
                let weight: f64 = (0..3)
                    .map(|axis| {
                        if CORNERS[a][axis] == 1 {
                            t[axis]
                        } else {
                            1.0 - t[axis]
                        }
                    })
                    .product();
                if weight == 0.0 {
                    continue;
                }
                for (master, w) in coarse.geometry.constraints.expand(*coarse_node) {
                    match transfer.parents[first..]
                        .iter_mut()
                        .find(|(m, _)| *m == master)
                    {
                        Some(entry) => entry.1 += weight * w,
                        None => transfer.parents.push((master, weight * w)),
                    }
                }
            }
        }
        transfer.start.push(transfer.parents.len());
        self.transfer = transfer;
    }

    /// Galerkin近似による粗い格子。粗くできなければNone。
    // 要素の行列の積は、隅と成分の番号の算術で書く方が対応を追いやすい。
    #[allow(clippy::needless_range_loop)]
    fn coarsen(&self) -> Option<Level> {
        let cells = &self.geometry.cells;
        if cells.len() <= 1 {
            return None;
        }
        let smallest = cells.iter().map(|cell| cell.1).min()?;
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

        // 対角へ加える値は、その節点を隅に持つ最初の要素の行列に含める。
        let constraints = &self.geometry.constraints;
        let mut owner = vec![usize::MAX; self.geometry.positions.len()];
        if !self.extra.is_empty() {
            for (e, element) in self.geometry.elements.iter().enumerate() {
                for node in element {
                    if owner[*node] == usize::MAX && !constraints.is_dependent(*node) {
                        owner[*node] = e;
                    }
                }
            }
        }
        // 隅の成分を拘束とみなすか。従属する隅は主節点がすべて拘束されていれば拘束とする。
        let corner_fixed = |node: usize, c: usize| {
            if constraints.is_dependent(node) {
                constraints
                    .masters(node)
                    .iter()
                    .all(|(m, _)| self.fixed[d * m + c])
            } else {
                self.fixed[d * node + c]
            }
        };
        // 拘束にも対角の値にも関わらない要素は、行列を表のまま共有できる。
        let plain: Vec<bool> = self
            .geometry
            .elements
            .iter()
            .enumerate()
            .map(|(e, element)| {
                element
                    .iter()
                    .all(|node| owner[*node] != e && (0..d).all(|c| !corner_fixed(*node, c)))
            })
            .collect();

        // 粗い要素のcellと、その子 (細かい要素の番号と子の位置) 。zの順に並べる。
        type Members = Vec<(usize, Option<usize>)>;
        let mut groups: BTreeMap<([i64; 3], i64), Members> = BTreeMap::new();
        for (e, cell) in cells.iter().enumerate() {
            let (parent, slot) = if cell.1 == smallest {
                let size = 2 * smallest;
                let at = cell.0.map(|v| v.div_euclid(size) * size);
                let child = (0..3)
                    .map(|axis| (((cell.0[axis] - at[axis]) / smallest) as usize) << axis)
                    .sum::<usize>();
                ((at, size), Some(child))
            } else {
                (*cell, None)
            };
            groups
                .entry(([parent.0[2], parent.0[1], parent.0[0]], parent.1))
                .or_default()
                .push((e, slot));
        }

        // 細かい要素の、拘束の行と列を除き対角の値を加えた行列。
        let local = |e: usize, out: &mut Vec<f64>| {
            let (matrix, scale) = self.matrix(e);
            out.clear();
            out.extend(matrix.iter().map(|k| scale * k));
            let element = &self.geometry.elements[e];
            for i in 0..n {
                let node = element[i / d];
                if !self.extra.is_empty() && owner[node] == e {
                    out[i * n + i] += self.extra[d * node + i % d];
                }
                if corner_fixed(node, i % d) {
                    for j in 0..n {
                        out[i * n + j] = 0.0;
                        out[j * n + i] = 0.0;
                    }
                }
            }
        };
        // target += (W ⊗ I)^T · fine · (W ⊗ I)
        let mut product = vec![0.0; n * n];
        let mut transform = |fine: &[f64], w: &[[f64; 8]; 8], target: &mut [f64]| {
            for i in 0..n {
                for b in 0..8 {
                    for c in 0..d {
                        product[i * n + b * d + c] =
                            (0..8).map(|a| fine[i * n + a * d + c] * w[a][b]).sum();
                    }
                }
            }
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
        };

        let mut table: Vec<Vec<f64>> = Vec::new();
        // 細かい表の番号から粗い表の番号。
        let mut carried: HashMap<usize, usize> = HashMap::new();
        // 子の配置と細かい表の番号から、まとめた行列の粗い表の番号。
        let mut merged: HashMap<(u8, usize), usize> = HashMap::new();
        let mut coarse_cells = Vec::with_capacity(groups.len());
        let mut assigned = Vec::with_capacity(groups.len());
        let mut scratch = Vec::new();
        for ((key, size), members) in groups {
            coarse_cells.push(([key[2], key[1], key[0]], size));
            let shared = members.iter().all(|(e, _)| plain[*e])
                && members
                    .iter()
                    .all(|(e, _)| self.assigned[*e] == self.assigned[members[0].0]);
            let (index, scale) = self.assigned[members[0].0];
            if shared && members[0].1.is_none() {
                let next = table.len();
                let coarse = *carried.entry(index).or_insert(next);
                if coarse == next {
                    table.push(self.table[index].clone());
                }
                assigned.push((coarse, scale));
                continue;
            }
            if shared {
                let mask = members
                    .iter()
                    .map(|(_, slot)| 1u8 << slot.expect("merged"))
                    .fold(0, |a, b| a | b);
                let next = table.len();
                let coarse = *merged.entry((mask, index)).or_insert(next);
                if coarse == next {
                    let mut matrix = vec![0.0; n * n];
                    for (_, slot) in &members {
                        transform(
                            &self.table[index],
                            &weights[slot.expect("merged")],
                            &mut matrix,
                        );
                    }
                    table.push(matrix);
                }
                assigned.push((coarse, scale));
                continue;
            }
            let mut matrix = vec![0.0; n * n];
            for (e, slot) in &members {
                local(*e, &mut scratch);
                match slot {
                    Some(slot) => transform(&scratch, &weights[*slot], &mut matrix),
                    None => {
                        for (target, value) in matrix.iter_mut().zip(&scratch) {
                            *target += value;
                        }
                    }
                }
            }
            table.push(matrix);
            assigned.push((table.len() - 1, 1.0));
        }

        // 粗い要素の隅を節点とし、zの順に番号を付ける。
        let mut positions: Vec<[i64; 3]> = coarse_cells
            .iter()
            .flat_map(|cell| (0..8).map(move |a| corner(cell, a)))
            .collect();
        positions.sort_unstable_by_key(|p| [p[2], p[1], p[0]]);
        positions.dedup();
        let index: HashMap<[i64; 3], usize> =
            positions.iter().enumerate().map(|(k, p)| (*p, k)).collect();
        let elements: Vec<[usize; 8]> = coarse_cells
            .iter()
            .map(|cell| std::array::from_fn(|a| index[&corner(cell, a)]))
            .collect();
        let locator = Locator::new(&coarse_cells);
        let coarse_constraints =
            octree::constraints(&positions, &coarse_cells, &elements, &locator);
        let dofs = d * positions.len();
        let mut coarse = Level::new(
            d,
            Geometry {
                positions,
                cells: coarse_cells,
                elements,
                constraints: coarse_constraints,
            },
            table,
            assigned,
            Vec::new(),
            vec![false; dofs],
        );
        // 細かい格子のどの自由度にも補間されない自由度を除く。従属する自由度は対角が0となる。
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
        let t = &self.transfer;
        for (k, node) in t.nodes.iter().enumerate() {
            for c in 0..d {
                let dof = d * node + c;
                if self.fixed[dof] {
                    continue;
                }
                for (parent, w) in &t.parents[t.start[k]..t.start[k + 1]] {
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
        let t = &self.transfer;
        for (k, node) in t.nodes.iter().enumerate() {
            for c in 0..d {
                let dof = d * node + c;
                if self.fixed[dof] {
                    continue;
                }
                for (parent, w) in &t.parents[t.start[k]..t.start[k + 1]] {
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

    /// 除いた自由度も含む`y = A x`。固定した値の右辺への寄与を求めるために使う。
    fn apply_full(&self, x: &[f64], y: &mut [f64]) {
        y.fill(0.0);
        self.accumulate_all(x, y, false);
        for (i, extra) in self.extra.iter().enumerate() {
            y[i] += extra * x[i];
        }
    }
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
        let d = level.dofs_per_node;
        let mut add = |row: usize, col: usize, value: f64| {
            let (row, col) = (position[row], position[col]);
            if row != usize::MAX && col != usize::MAX {
                a[row * m + col] += value;
            }
        };
        for (e, element) in level.geometry.elements.iter().enumerate() {
            let (matrix, scale) = level.matrix(e);
            let links = level.links(e);
            if links.is_empty() {
                for i in 0..n {
                    for j in 0..n {
                        add(
                            level.dof(element, i),
                            level.dof(element, j),
                            scale * matrix[i * n + j],
                        );
                    }
                }
                continue;
            }
            for &(a1, m1, w1) in links {
                for &(a2, m2, w2) in links {
                    for c1 in 0..d {
                        for c2 in 0..d {
                            let k = matrix[(a1 * d + c1) * n + a2 * d + c2];
                            add(d * m1 + c1, d * m2 + c2, scale * w1 * w2 * k);
                        }
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
                let (fine, coarse) = levels.split_at_mut(k + 1);
                fine[k].compute_transfer(&coarse[0]);
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
    /// 従属する節点の値は、解いた後に主節点から補間する。収束すれば反復回数を返す。
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
                level.geometry.constraints.fill(level.dofs_per_node, x);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::octree::children;

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

    /// cellの集まりの格子。節点はzの順に番号を付ける。
    fn geometry(mut cells: Vec<Cell>) -> Geometry {
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
        let constraints = octree::constraints(&positions, &cells, &elements, &locator);
        Geometry {
            positions,
            cells,
            elements,
            constraints,
        }
    }

    /// x方向の両端の温度を固定した棒を解き、温度が長さ方向に線形であることを確かめる。
    fn linear_temperature(cells: Vec<Cell>, length: i64, dense_limit: usize) -> usize {
        let geometry = geometry(cells);
        let count = geometry.positions.len();
        let mut fixed = vec![false; count];
        let mut x = vec![0.0; count];
        for (node, point) in geometry.positions.iter().enumerate() {
            if point[0] == 0 || point[0] == length {
                fixed[node] = true;
                x[node] = 10.0 + 40.0 * point[0] as f64 / length as f64;
            }
        }
        let assigned = geometry
            .cells
            .iter()
            .map(|cell| (0, cell.1 as f64))
            .collect();
        let positions = geometry.positions.clone();
        let level = Level::new(1, geometry, vec![conduction()], assigned, Vec::new(), fixed);
        let hierarchy = Hierarchy::with_dense_limit(level, dense_limit);
        assert!(hierarchy.levels.len() >= 3);
        let iterations = hierarchy.solve(&vec![0.0; count], &mut x).unwrap();
        for (node, point) in positions.iter().enumerate() {
            let expected = 10.0 + 40.0 * point[0] as f64 / length as f64;
            assert!(
                (x[node] - expected).abs() < 1e-6,
                "{point:?} {} {expected}",
                x[node]
            );
        }
        iterations
    }

    #[test]
    fn multigrid_matches_the_linear_temperature_field() {
        // 長さ40 cellの棒。粗くすると奇数のcell数を含む格子になる。
        let mut cells = Vec::new();
        for z in 0..3 {
            for y in 0..5 {
                for x in 0..40 {
                    cells.push(([x, y, z], 1));
                }
            }
        }
        let iterations = linear_temperature(cells, 40, 50);
        assert!(iterations < 30, "{iterations}");
    }

    #[test]
    fn hanging_nodes_keep_the_linear_temperature_field() {
        // 一辺4のcellを並べた棒の中央を2段まで分割し、2:1に揃えた格子。線形の温度は
        // 従属する節点を含めて厳密に表せる。
        let mut cells = Vec::new();
        for x in 0..10 {
            for y in 0..2 {
                let cell = ([4 * x, 4 * y, 0], 4);
                match x {
                    4 | 5 => {
                        for child in children(&cell) {
                            if x == 4 && child.0[0] >= 18 {
                                cells.extend(children(&child));
                            } else {
                                cells.push(child);
                            }
                        }
                    }
                    _ => cells.push(cell),
                }
            }
        }
        let iterations = linear_temperature(cells, 40, 20);
        assert!(iterations < 40, "{iterations}");
    }
}
