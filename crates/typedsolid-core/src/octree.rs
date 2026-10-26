//! 解析の格子を局所的に細かくする八分木。
//!
//! 格子は大きさの異なる立方体のcellからなる。cellの位置は、最も細かいcellの一辺を単位とする
//! 整数の格子座標で表す。cellの一辺は2の冪で、最小の隅はその一辺の倍数に揃う。基準の格子の
//! cellを必要な段まで8分割し、隣り合う (面、辺、頂点を共有する) cellの一辺の比を2以下に
//! 揃える (2:1 balance)。
//!
//! 大きいcellの面や辺の内側に、隣の小さいcellの節点が乗る (hanging node)。その節点の値は、
//! 大きいcellの隅の値の三重線形補間で決まるものとし、方程式の未知数から除く。補間に使う
//! 隅がさらに別のcellの面に乗る場合は、乗らない節点に達するまで展開する。これにより
//! 変位と温度は格子全体で連続となる。

use crate::multigrid::CORNERS;
use std::collections::{HashMap, HashSet};

/// cellの最小の隅の格子座標と一辺。
pub(crate) type Cell = ([i64; 3], i64);

/// cellの隅aの格子座標。隅の順は`CORNERS`である。
pub(crate) fn corner(cell: &Cell, a: usize) -> [i64; 3] {
    std::array::from_fn(|axis| cell.0[axis] + CORNERS[a][axis] as i64 * cell.1)
}

/// 重ならないcellの集まりを、点から引く索引。
pub(crate) struct Locator {
    /// 最小の隅からcellの番号。cellは重ならないため、隅は1つのcellだけを指す。
    by_corner: HashMap<[i64; 3], usize>,
    /// 現れる一辺。大きい順。
    sizes: Vec<i64>,
}

impl Locator {
    pub(crate) fn new(cells: &[Cell]) -> Self {
        let by_corner = cells
            .iter()
            .enumerate()
            .map(|(k, cell)| (cell.0, k))
            .collect();
        let mut sizes: Vec<i64> = cells.iter().map(|cell| cell.1).collect();
        sizes.sort_unstable_by(|a, b| b.cmp(a));
        sizes.dedup();
        Self { by_corner, sizes }
    }

    /// 格子座標を`scale`倍した座標の点`point`を、半開区間で含むcell。
    pub(crate) fn find(&self, cells: &[Cell], point: [i64; 3], scale: i64) -> Option<usize> {
        self.sizes.iter().find_map(|&size| {
            let at = point.map(|v| v.div_euclid(scale * size) * size);
            self.by_corner
                .get(&at)
                .copied()
                .filter(|k| cells[*k].1 == size)
        })
    }

    /// 格子座標の点を閉じた範囲に含むcell。
    pub(crate) fn containing(&self, cells: &[Cell], point: [i64; 3], out: &mut Vec<usize>) {
        out.clear();
        for &size in &self.sizes {
            let candidates = point.map(|v| {
                let base = v.div_euclid(size) * size;
                if v == base {
                    [base, base - size]
                } else {
                    [base, base]
                }
            });
            for k in 0..8 {
                let at = std::array::from_fn(|axis| candidates[axis][(k >> axis) & 1]);
                if let Some(&index) = self.by_corner.get(&at)
                    && cells[index].1 == size
                    && !out.contains(&index)
                {
                    out.push(index);
                }
            }
        }
    }
}

/// 節点がcellの隅でなければ、そのcellの隅の三重線形補間の重み。
fn interpolation(cell: &Cell, point: [i64; 3]) -> Option<[f64; 8]> {
    let on_corner =
        (0..3).all(|axis| point[axis] == cell.0[axis] || point[axis] == cell.0[axis] + cell.1);
    if on_corner {
        return None;
    }
    let t: [f64; 3] =
        std::array::from_fn(|axis| (point[axis] - cell.0[axis]) as f64 / cell.1 as f64);
    Some(std::array::from_fn(|a| {
        (0..3)
            .map(|axis| {
                if CORNERS[a][axis] == 1 {
                    t[axis]
                } else {
                    1.0 - t[axis]
                }
            })
            .product()
    }))
}

/// 従属する節点 (hanging node) と、その値を決める主節点と重み。
pub(crate) struct Constraints {
    /// 節点ごとの`masters`の範囲の始まり。節点の数より1多い。
    start: Vec<usize>,
    masters: Vec<(usize, f64)>,
}

impl Constraints {
    /// 従属する節点の無い格子。
    pub(crate) fn none(nodes: usize) -> Self {
        Self {
            start: vec![0; nodes + 1],
            masters: Vec::new(),
        }
    }

    /// 節点の値を決める主節点と重み。従属しない節点では空。
    pub(crate) fn masters(&self, node: usize) -> &[(usize, f64)] {
        &self.masters[self.start[node]..self.start[node + 1]]
    }

    pub(crate) fn is_dependent(&self, node: usize) -> bool {
        self.start[node + 1] > self.start[node]
    }

    /// 節点の、従属を展開した (主節点, 重み)。従属しない節点は自身を重み1で返す。
    pub(crate) fn expand(&self, node: usize) -> Vec<(usize, f64)> {
        if self.is_dependent(node) {
            self.masters(node).to_vec()
        } else {
            vec![(node, 1.0)]
        }
    }

    /// 節点ごとの値`x` (節点あたり`dofs`成分) の、従属する節点の値を主節点から補間する。
    pub(crate) fn fill(&self, dofs: usize, x: &mut [f64]) {
        for node in (0..self.start.len() - 1).filter(|n| self.is_dependent(*n)) {
            for c in 0..dofs {
                x[dofs * node + c] = self
                    .masters(node)
                    .iter()
                    .map(|(m, w)| w * x[dofs * m + c])
                    .sum();
            }
        }
    }
}

/// 要素の隅が他のcellの面や辺の内側に乗る節点を求める。節点`positions`は要素`cells`の隅で、
/// `elements`は要素ごとの8隅の節点の番号である。
pub(crate) fn constraints(
    positions: &[[i64; 3]],
    cells: &[Cell],
    elements: &[[usize; 8]],
    locator: &Locator,
) -> Constraints {
    // 節点ごとに、隅としない最も大きいcellの隅による補間。
    let mut direct: Vec<Option<Vec<(usize, f64)>>> = vec![None; positions.len()];
    let mut found = Vec::new();
    for (node, point) in positions.iter().enumerate() {
        locator.containing(cells, *point, &mut found);
        let host = found
            .iter()
            .filter_map(|&k| interpolation(&cells[k], *point).map(|w| (k, w)))
            .max_by_key(|(k, _)| cells[*k].1);
        if let Some((k, weights)) = host {
            direct[node] = Some(
                weights
                    .iter()
                    .enumerate()
                    .filter(|(_, w)| **w > 0.0)
                    .map(|(a, w)| (elements[k][a], *w))
                    .collect(),
            );
        }
    }
    // 補間に使う隅が従属する場合は展開する。従属の先は常により大きいcellの隅であり、循環しない。
    fn resolve(
        direct: &[Option<Vec<(usize, f64)>>],
        node: usize,
        weight: f64,
        out: &mut Vec<(usize, f64)>,
    ) {
        match &direct[node] {
            None => match out.iter_mut().find(|(m, _)| *m == node) {
                Some(entry) => entry.1 += weight,
                None => out.push((node, weight)),
            },
            Some(masters) => {
                for (m, w) in masters {
                    resolve(direct, *m, weight * w, out);
                }
            }
        }
    }
    let mut start = Vec::with_capacity(positions.len() + 1);
    let mut masters = Vec::new();
    for node in 0..positions.len() {
        start.push(masters.len());
        if direct[node].is_some() {
            let mut out = Vec::new();
            resolve(&direct, node, 1.0, &mut out);
            masters.extend(out);
        }
    }
    start.push(masters.len());
    Constraints { start, masters }
}

/// 基準の格子のcellを分割した木。分割しない基準のcellは`leaves`に持たず、`split`に無いことで表す。
pub(crate) struct Tree {
    /// 基準のcellの一辺。
    pub(crate) base: i64,
    /// 基準の格子の各軸のcell数。
    pub(crate) size: [usize; 3],
    /// 分割した基準のcellの番号 (各軸の位置)。
    split: HashSet<[i64; 3]>,
    /// 分割で生じた葉。最小の隅から一辺。
    leaves: HashMap<[i64; 3], i64>,
}

impl Tree {
    /// 基準のcellを、`depth`が返す段まで分割する。葉の数が`limit`を超えれば、その時点の数を返す。
    pub(crate) fn new(
        size: [usize; 3],
        base: i64,
        depth: impl Fn(&Cell) -> u32,
        limit: usize,
    ) -> Result<Self, usize> {
        let mut tree = Self {
            base,
            size,
            split: HashSet::new(),
            leaves: HashMap::new(),
        };
        let mut stack = Vec::new();
        for z in 0..size[2] as i64 {
            for y in 0..size[1] as i64 {
                for x in 0..size[0] as i64 {
                    let cell = ([x * base, y * base, z * base], base);
                    if depth(&cell) == 0 {
                        continue;
                    }
                    tree.split.insert([x, y, z]);
                    stack.push((cell, 0u32));
                    while let Some((cell, level)) = stack.pop() {
                        if level < depth(&cell) && cell.1 > 1 {
                            stack.extend(children(&cell).map(|child| (child, level + 1)));
                        } else if level > 0 {
                            tree.leaves.insert(cell.0, cell.1);
                        }
                    }
                    if tree.count() > limit {
                        return Err(tree.count());
                    }
                }
            }
        }
        tree.balance(limit)?;
        Ok(tree)
    }

    /// 葉の数。
    pub(crate) fn count(&self) -> usize {
        self.size.iter().product::<usize>() - self.split.len() + self.leaves.len()
    }

    /// 分割した葉があるか。
    pub(crate) fn refined(&self) -> bool {
        !self.split.is_empty()
    }

    /// 格子座標を`scale`倍した座標の点を半開区間で含む葉。
    fn leaf(&self, point: [i64; 3], scale: i64) -> Option<Cell> {
        let index = point.map(|v| v.div_euclid(scale * self.base));
        if (0..3).any(|axis| index[axis] < 0 || index[axis] >= self.size[axis] as i64) {
            return None;
        }
        if !self.split.contains(&index) {
            return Some((index.map(|v| v * self.base), self.base));
        }
        let mut size = self.base / 2;
        while size >= 1 {
            let at = point.map(|v| v.div_euclid(scale * size) * size);
            if self.leaves.get(&at) == Some(&size) {
                return Some((at, size));
            }
            size /= 2;
        }
        None
    }

    /// 隣り合う葉の一辺の比を2以下にする。
    fn balance(&mut self, limit: usize) -> Result<(), usize> {
        loop {
            let mut coarse: HashSet<Cell> = HashSet::new();
            for (&at, &size) in &self.leaves {
                if 4 * size > self.base {
                    continue;
                }
                for k in 0..27 {
                    let offset = [k % 3, (k / 3) % 3, k / 9];
                    if offset == [1, 1, 1] {
                        continue;
                    }
                    // 2倍の座標で、隣のcellの内側の点。
                    let probe: [i64; 3] = std::array::from_fn(|axis| match offset[axis] {
                        0 => 2 * at[axis] - 1,
                        1 => 2 * at[axis] + size,
                        _ => 2 * (at[axis] + size) + 1,
                    });
                    if let Some(neighbour) = self.leaf(probe, 2)
                        && neighbour.1 > 2 * size
                    {
                        coarse.insert(neighbour);
                    }
                }
            }
            if coarse.is_empty() {
                return Ok(());
            }
            for cell in coarse {
                if cell.1 == self.base {
                    self.split.insert(cell.0.map(|v| v / self.base));
                } else {
                    self.leaves.remove(&cell.0);
                }
                for child in children(&cell) {
                    self.leaves.insert(child.0, child.1);
                }
            }
            if self.count() > limit {
                return Err(self.count());
            }
        }
    }

    /// 分割で生じた葉。
    pub(crate) fn refined_leaves(&self) -> impl Iterator<Item = Cell> + '_ {
        self.leaves.iter().map(|(at, size)| (*at, *size))
    }

    /// 分割していない基準のcellか。番号は各軸の位置。
    pub(crate) fn is_whole(&self, index: [i64; 3]) -> bool {
        !self.split.contains(&index)
    }
}

/// 8つの子のcell。
pub(crate) fn children(cell: &Cell) -> [Cell; 8] {
    let half = cell.1 / 2;
    std::array::from_fn(|k| {
        (
            std::array::from_fn(|axis| cell.0[axis] + ((k >> axis) & 1) as i64 * half),
            half,
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn balance_limits_neighbour_ratio_to_two() {
        // 4×1×1の基準の格子 (一辺8) の端の1 cellだけを3段分割すると、隣の基準のcellも分割される。
        let tree = Tree::new([4, 1, 1], 8, |cell| if cell.0[0] < 8 { 3 } else { 0 }, 1000).unwrap();
        let mut cells: Vec<Cell> = tree.refined_leaves().collect();
        for x in 0..4 {
            if tree.is_whole([x, 0, 0]) {
                cells.push(([8 * x, 0, 0], 8));
            }
        }
        let locator = Locator::new(&cells);
        for cell in &cells {
            for offset in [-1i64, 1] {
                let probe = [
                    if offset < 0 {
                        2 * cell.0[0] - 1
                    } else {
                        2 * (cell.0[0] + cell.1) + 1
                    },
                    2 * cell.0[1] + cell.1,
                    2 * cell.0[2] + cell.1,
                ];
                if let Some(k) = locator.find(&cells, probe, 2) {
                    assert!(
                        cells[k].1 <= 2 * cell.1 && cell.1 <= 2 * cells[k].1,
                        "{cell:?} {:?}",
                        cells[k]
                    );
                }
            }
        }
        // 細かい葉 (一辺1) は端の基準のcellの中だけにある。
        assert!(cells.iter().filter(|c| c.1 == 1).all(|c| c.0[0] < 8));
        assert!(!tree.is_whole([1, 0, 0]));
        assert!(tree.is_whole([3, 0, 0]));
    }

    #[test]
    fn hanging_nodes_interpolate_the_larger_cell() {
        // 一辺2のcellの+x側に一辺1のcellを4つ並べる。大きいcellの面の中心と辺の中点は従属する。
        let mut cells: Vec<Cell> = vec![([0, 0, 0], 2)];
        for k in 0..4 {
            cells.push(([2, (k & 1) as i64, (k >> 1) as i64], 1));
        }
        let mut index: HashMap<[i64; 3], usize> = HashMap::new();
        let mut positions = Vec::new();
        let elements: Vec<[usize; 8]> = cells
            .iter()
            .map(|cell| {
                std::array::from_fn(|a| {
                    let point = corner(cell, a);
                    *index.entry(point).or_insert_with(|| {
                        positions.push(point);
                        positions.len() - 1
                    })
                })
            })
            .collect();
        let locator = Locator::new(&cells);
        let constraints = constraints(&positions, &cells, &elements, &locator);
        let centre = index[&[2, 1, 1]];
        let masters = constraints.masters(centre);
        assert_eq!(masters.len(), 4);
        assert!(masters.iter().all(|(_, w)| (*w - 0.25).abs() < 1e-12));
        let edge = index[&[2, 1, 0]];
        assert_eq!(constraints.masters(edge).len(), 2);
        assert!(!constraints.is_dependent(index[&[2, 0, 0]]));
        assert!(!constraints.is_dependent(index[&[3, 1, 1]]));
    }
}
