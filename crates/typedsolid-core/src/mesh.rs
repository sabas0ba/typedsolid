//! backendが生成したSTL meshの構造検査。設計入力ではなく出力表現を対象とする。

use crate::{Check, Rule, Status};
use std::collections::HashMap;

const HEADER_BYTES: usize = 80;
const COUNT_BYTES: usize = 4;
const TRIANGLE_BYTES: usize = 50;
/// 面積がこの値以下の三角形を退化とみなす。単位はmm²。
const DEGENERATE_AREA_MM2: f64 = 1e-12;

/// 頂点は正規化したf32のbit patternで同一視する。同一座標を異なるfloatで書いたmeshは
/// 隣接を検出できず非manifoldと判定される。判定は安全側に倒れる。
type VertexKey = [u32; 3];

pub struct Triangle {
    vertices: [[f64; 3]; 3],
    keys: [VertexKey; 3],
}

impl Triangle {
    /// 3頂点の座標。単位はmm。
    pub fn vertices(&self) -> [[f64; 3]; 3] {
        self.vertices
    }

    fn area_mm2(&self) -> f64 {
        let [a, b, c] = self.vertices;
        let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let n = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt() / 2.0
    }

    /// 原点を頂点とする四面体の符号付き体積。全三角形の和が閉じたmeshの体積になる。
    fn signed_volume_mm3(&self) -> f64 {
        let [a, b, c] = self.vertices;
        (a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
            + a[2] * (b[0] * c[1] - b[1] * c[0]))
            / 6.0
    }
}

/// -0.0と+0.0を同一視する。NaNは読み込み時に拒否済み。
fn vertex_key(value: [f32; 3]) -> VertexKey {
    value.map(|c| if c == 0.0 { 0.0f32 } else { c }.to_bits())
}

pub fn parse_binary_stl(bytes: &[u8]) -> Result<Vec<Triangle>, String> {
    if bytes.len() < HEADER_BYTES + COUNT_BYTES {
        return Err("binary STL requires an 80 byte header and a 4 byte triangle count".into());
    }
    let declared = u32::from_le_bytes(
        bytes[HEADER_BYTES..HEADER_BYTES + COUNT_BYTES]
            .try_into()
            .expect("slice is 4 bytes"),
    ) as usize;
    let expected = declared
        .checked_mul(TRIANGLE_BYTES)
        .and_then(|n| n.checked_add(HEADER_BYTES + COUNT_BYTES))
        .ok_or("declared triangle count exceeds the addressable length")?;
    if bytes.len() != expected {
        return Err(format!(
            "length {} does not match {declared} triangles ({expected} bytes); only binary STL is supported",
            bytes.len()
        ));
    }
    if declared == 0 {
        return Err("mesh declares no triangles".into());
    }
    let mut triangles = Vec::with_capacity(declared);
    for index in 0..declared {
        // 各記録はnormal 12 bytes、頂点36 bytes、attribute 2 bytesの順に並ぶ。
        // normalは頂点から再計算できるため読まない。
        let base = HEADER_BYTES + COUNT_BYTES + index * TRIANGLE_BYTES + 12;
        let mut vertices = [[0.0f64; 3]; 3];
        let mut keys = [[0u32; 3]; 3];
        for corner in 0..3 {
            let mut raw = [0.0f32; 3];
            for axis in 0..3 {
                let at = base + (corner * 3 + axis) * 4;
                let value =
                    f32::from_le_bytes(bytes[at..at + 4].try_into().expect("slice is 4 bytes"));
                if !value.is_finite() {
                    return Err(format!("triangle {index} has a non-finite coordinate"));
                }
                raw[axis] = value;
                vertices[corner][axis] = f64::from(value);
            }
            keys[corner] = vertex_key(raw);
        }
        triangles.push(Triangle { vertices, keys });
    }
    Ok(triangles)
}

/// binaryとASCIIのSTLを読む。長さが宣言した三角形数と一致すればbinaryとして読み、
/// そうでなく`solid`で始まればASCIIとして読む。binaryのheaderも`solid`で始まり得るため、
/// 長さを先に照合する。
pub fn parse_stl(bytes: &[u8]) -> Result<Vec<Triangle>, String> {
    match parse_binary_stl(bytes) {
        Ok(triangles) => Ok(triangles),
        Err(_) if bytes.trim_ascii_start().starts_with(b"solid") => parse_ascii_stl(bytes),
        Err(error) => Err(format!("{error}; an ASCII STL must start with \"solid\"")),
    }
}

/// ASCII STL。`vertex x y z`を3つずつ1つの三角形として読む。normalは再計算できるため読まない。
fn parse_ascii_stl(bytes: &[u8]) -> Result<Vec<Triangle>, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "ASCII STL is not valid UTF-8")?;
    let mut tokens = text.split_ascii_whitespace();
    let mut corners: Vec<([f64; 3], VertexKey)> = Vec::new();
    while let Some(token) = tokens.next() {
        if token != "vertex" {
            continue;
        }
        let mut position = [0.0f64; 3];
        let mut raw = [0.0f32; 3];
        for axis in 0..3 {
            let value: f32 = tokens
                .next()
                .and_then(|t| t.parse().ok())
                .filter(|v: &f32| v.is_finite())
                .ok_or_else(|| format!("vertex {} has an invalid coordinate", corners.len()))?;
            raw[axis] = value;
            position[axis] = f64::from(value);
        }
        corners.push((position, vertex_key(raw)));
    }
    if corners.is_empty() || !corners.len().is_multiple_of(3) {
        return Err(format!(
            "ASCII STL has {} vertices; a positive multiple of 3 is required",
            corners.len()
        ));
    }
    Ok(corners
        .chunks_exact(3)
        .map(|c| Triangle {
            vertices: [c[0].0, c[1].0, c[2].0],
            keys: [c[0].1, c[1].1, c[2].1],
        })
        .collect())
}

/// 向き付きで対にならない辺の数。0でなければ、meshは閉じていないか向きが不整合である。
///
/// 閉じて向きの揃ったmeshでは、各辺をa→bの向きに使う三角形とb→aの向きに使う
/// 三角形が同数ある。向きを問わない出現回数の偶奇だけでは、z軸に平行な面の三角形が
/// 1枚だけ裏返っている場合を検出できない。空洞の内面や複数のshellを含むmeshも、
/// 閉じて向きが揃っていれば0になる。2頂点が一致する退化三角形の長さ0の辺は、面を
/// 区切らないため数えない。
pub fn unpaired_edges(triangles: &[Triangle]) -> usize {
    // 頂点を昇順に並べた辺ごとに、昇順の向きの出現を+1、逆向きを-1として数える。
    let mut balance: HashMap<(VertexKey, VertexKey), i64> = HashMap::new();
    for triangle in triangles {
        for corner in 0..3 {
            let (a, b) = (triangle.keys[corner], triangle.keys[(corner + 1) % 3]);
            if a == b {
                continue;
            }
            let (key, step) = if a < b { ((a, b), 1) } else { ((b, a), -1) };
            *balance.entry(key).or_default() += step;
        }
    }
    balance.values().filter(|count| **count != 0).count()
}

fn find(parent: &mut [usize], mut node: usize) -> usize {
    while parent[node] != node {
        parent[node] = parent[parent[node]];
        node = parent[node];
    }
    node
}

fn shell_count(triangles: &[Triangle]) -> usize {
    let mut index: HashMap<VertexKey, usize> = HashMap::new();
    let mut parent: Vec<usize> = Vec::new();
    for triangle in triangles {
        let mut ids = [0usize; 3];
        for (slot, key) in ids.iter_mut().zip(triangle.keys.iter()) {
            *slot = *index.entry(*key).or_insert_with(|| {
                parent.push(parent.len());
                parent.len() - 1
            });
        }
        for other in [ids[1], ids[2]] {
            let (a, b) = (find(&mut parent, ids[0]), find(&mut parent, other));
            parent[b] = a;
        }
    }
    (0..parent.len())
        .filter(|&node| find(&mut parent, node) == node)
        .count()
}

fn check(rule: Rule, target: &str, passed: bool, message: String) -> Check {
    Check {
        locations: Vec::new(),
        rule,
        status: if passed { Status::Pass } else { Status::Fail },
        target: target.into(),
        message,
    }
}

/// 閉じた向き整合の単一shellであること、およびsolid体積との一致を検査する。
pub fn inspect(
    triangles: &[Triangle],
    target: &str,
    solid_volume_mm3: f64,
    relative_tolerance: f64,
) -> Vec<Check> {
    let degenerate = triangles
        .iter()
        .filter(|t| {
            t.keys[0] == t.keys[1]
                || t.keys[1] == t.keys[2]
                || t.keys[2] == t.keys[0]
                || t.area_mm2() <= DEGENERATE_AREA_MM2
        })
        .count();

    let mut directed: HashMap<(VertexKey, VertexKey), usize> = HashMap::new();
    for triangle in triangles {
        for corner in 0..3 {
            *directed
                .entry((triangle.keys[corner], triangle.keys[(corner + 1) % 3]))
                .or_default() += 1;
        }
    }
    // 閉じた向き整合のmeshでは、各有向edgeが1回、その逆向きも1回だけ現れる。
    let repeated = directed.values().filter(|&&count| count != 1).count();
    let unpaired = directed
        .keys()
        .filter(|(a, b)| !directed.contains_key(&(*b, *a)))
        .count();
    let shells = shell_count(triangles);

    let mut problems = Vec::new();
    if degenerate > 0 {
        problems.push(format!("{degenerate} degenerate triangle(s)"));
    }
    if unpaired > 0 {
        problems.push(format!("{unpaired} boundary edge(s): mesh is not closed"));
    }
    if repeated > 0 {
        problems.push(format!(
            "{repeated} edge(s) repeated in the same direction: orientation is inconsistent or faces are duplicated"
        ));
    }
    if shells != 1 {
        problems.push(format!("{shells} connected component(s)"));
    }

    let signed = triangles
        .iter()
        .map(Triangle::signed_volume_mm3)
        .sum::<f64>();
    let volume_message = if solid_volume_mm3 <= 0.0 {
        format!("solid volume {solid_volume_mm3:.9e} mm³ is not positive")
    } else if signed <= 0.0 {
        format!("mesh volume {signed:.9e} mm³ is not positive: triangle winding is inverted")
    } else {
        let relative = (signed - solid_volume_mm3).abs() / solid_volume_mm3;
        format!(
            "mesh {signed:.9e} mm³ vs solid {solid_volume_mm3:.9e} mm³; relative difference {relative:.3e}; tolerance {relative_tolerance:.3e}"
        )
    };
    let volume_passed = solid_volume_mm3 > 0.0
        && signed > 0.0
        && (signed - solid_volume_mm3).abs() / solid_volume_mm3 <= relative_tolerance;

    vec![
        check(
            Rule::MeshManifold,
            target,
            problems.is_empty(),
            if problems.is_empty() {
                format!(
                    "{} triangles, {} edges, 1 shell; closed and consistently oriented",
                    triangles.len(),
                    directed.len()
                )
            } else {
                problems.join("; ")
            },
        ),
        check(Rule::MeshVolume, target, volume_passed, volume_message),
    ]
}

/// 解析失敗を例外にせず、検査のfailとして保持する。
pub fn inspect_binary_stl(
    bytes: &[u8],
    target: &str,
    solid_volume_mm3: f64,
    relative_tolerance: f64,
) -> Vec<Check> {
    match parse_binary_stl(bytes) {
        Ok(triangles) => inspect(&triangles, target, solid_volume_mm3, relative_tolerance),
        Err(error) => vec![
            check(Rule::MeshManifold, target, false, error.clone()),
            check(
                Rule::MeshVolume,
                target,
                false,
                format!("mesh could not be read: {error}"),
            ),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 軸平行boxの12三角形を外向きに構成する。
    fn box_triangles(origin: [f32; 3], size: f32) -> Vec<[[f32; 3]; 3]> {
        let [x, y, z] = origin;
        let corner = |i: usize, j: usize, k: usize| {
            [
                x + i as f32 * size,
                y + j as f32 * size,
                z + k as f32 * size,
            ]
        };
        let quads = [
            // 面ごとに外向きの巡回順で4隅を並べる。
            [(0, 0, 0), (0, 1, 0), (1, 1, 0), (1, 0, 0)], // -Z
            [(0, 0, 1), (1, 0, 1), (1, 1, 1), (0, 1, 1)], // +Z
            [(0, 0, 0), (1, 0, 0), (1, 0, 1), (0, 0, 1)], // -Y
            [(0, 1, 0), (0, 1, 1), (1, 1, 1), (1, 1, 0)], // +Y
            [(0, 0, 0), (0, 0, 1), (0, 1, 1), (0, 1, 0)], // -X
            [(1, 0, 0), (1, 1, 0), (1, 1, 1), (1, 0, 1)], // +X
        ];
        quads
            .iter()
            .flat_map(|quad| {
                let v: Vec<_> = quad.iter().map(|&(i, j, k)| corner(i, j, k)).collect();
                [[v[0], v[1], v[2]], [v[0], v[2], v[3]]]
            })
            .collect()
    }

    fn encode(triangles: &[[[f32; 3]; 3]]) -> Vec<u8> {
        let mut bytes = vec![0u8; HEADER_BYTES];
        bytes.extend((triangles.len() as u32).to_le_bytes());
        for triangle in triangles {
            bytes.extend([0u8; 12]);
            for vertex in triangle {
                for axis in vertex {
                    bytes.extend(axis.to_le_bytes());
                }
            }
            bytes.extend([0u8; 2]);
        }
        bytes
    }

    fn inspect_bytes(bytes: &[u8], volume: f64) -> Vec<Check> {
        inspect_binary_stl(bytes, "part", volume, 0.01)
    }

    fn status(checks: &[Check], rule: Rule) -> Status {
        checks.iter().find(|c| c.rule == rule).unwrap().status
    }

    #[test]
    fn closed_box_passes() {
        let checks = inspect_bytes(&encode(&box_triangles([0.; 3], 10.)), 1000.);
        assert_eq!(status(&checks, Rule::MeshManifold), Status::Pass);
        assert_eq!(status(&checks, Rule::MeshVolume), Status::Pass);
    }

    #[test]
    fn open_mesh_fails() {
        let mut triangles = box_triangles([0.; 3], 10.);
        triangles.pop();
        let checks = inspect_bytes(&encode(&triangles), 1000.);
        assert_eq!(status(&checks, Rule::MeshManifold), Status::Fail);
        assert!(checks[0].message.contains("not closed"));
    }

    #[test]
    fn inverted_triangle_fails() {
        let mut triangles = box_triangles([0.; 3], 10.);
        triangles[0].swap(1, 2);
        let checks = inspect_bytes(&encode(&triangles), 1000.);
        assert_eq!(status(&checks, Rule::MeshManifold), Status::Fail);
    }

    #[test]
    fn inverted_mesh_fails_volume() {
        let triangles: Vec<_> = box_triangles([0.; 3], 10.)
            .into_iter()
            .map(|mut t| {
                t.swap(1, 2);
                t
            })
            .collect();
        let checks = inspect_bytes(&encode(&triangles), 1000.);
        // 全反転はedgeの対応を保つため、体積の符号だけが不正を示す。
        assert_eq!(status(&checks, Rule::MeshManifold), Status::Pass);
        assert_eq!(status(&checks, Rule::MeshVolume), Status::Fail);
        assert!(checks[1].message.contains("inverted"));
    }

    #[test]
    fn degenerate_triangle_fails() {
        let mut triangles = box_triangles([0.; 3], 10.);
        triangles.push([[0., 0., 0.], [1., 0., 0.], [1., 0., 0.]]);
        let checks = inspect_bytes(&encode(&triangles), 1000.);
        assert_eq!(status(&checks, Rule::MeshManifold), Status::Fail);
        assert!(checks[0].message.contains("degenerate"));
    }

    #[test]
    fn disconnected_shells_fail() {
        let mut triangles = box_triangles([0.; 3], 10.);
        triangles.extend(box_triangles([50.; 3], 10.));
        let checks = inspect_bytes(&encode(&triangles), 2000.);
        assert_eq!(status(&checks, Rule::MeshManifold), Status::Fail);
        assert!(checks[0].message.contains("2 connected component"));
    }

    #[test]
    fn volume_mismatch_fails() {
        let bytes = encode(&box_triangles([0.; 3], 10.));
        assert_eq!(
            status(&inspect_bytes(&bytes, 900.), Rule::MeshVolume),
            Status::Fail
        );
        assert_eq!(
            status(&inspect_bytes(&bytes, 1005.), Rule::MeshVolume),
            Status::Pass
        );
        assert_eq!(
            status(&inspect_bytes(&bytes, 0.), Rule::MeshVolume),
            Status::Fail
        );
    }

    #[test]
    fn malformed_input_is_rejected() {
        let valid = encode(&box_triangles([0.; 3], 10.));
        for bytes in [
            vec![0u8; 10],
            valid[..valid.len() - 1].to_vec(),
            encode(&[]),
        ] {
            assert!(parse_binary_stl(&bytes).is_err());
        }
        let mut count_too_large = valid.clone();
        count_too_large[HEADER_BYTES..HEADER_BYTES + COUNT_BYTES]
            .copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(parse_binary_stl(&count_too_large).is_err());

        let mut not_finite = valid.clone();
        let at = HEADER_BYTES + COUNT_BYTES + 12;
        not_finite[at..at + 4].copy_from_slice(&f32::NAN.to_le_bytes());
        assert!(parse_binary_stl(&not_finite).is_err());
        assert_eq!(
            status(&inspect_bytes(&not_finite, 1000.), Rule::MeshManifold),
            Status::Fail
        );
    }

    #[test]
    fn signed_zero_vertices_are_one_vertex() {
        // -0.0で書かれた座標が別頂点として扱われると、閉じたmeshが非manifoldになる。
        let mut triangles = box_triangles([0.; 3], 10.);
        for triangle in triangles.iter_mut() {
            for vertex in triangle.iter_mut() {
                for axis in vertex.iter_mut() {
                    if *axis == 0.0 {
                        *axis = -0.0;
                    }
                }
            }
        }
        assert_eq!(
            status(
                &inspect_bytes(&encode(&triangles), 1000.),
                Rule::MeshManifold
            ),
            Status::Pass
        );
    }
}
