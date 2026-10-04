"""検査結果の投影図。組立状態の全部品を3面図と等角図に描き、検出箇所のboxを重ねる。

部品の線はOCCTの隠線処理 (HLR) で求め、見える線を実線、隠れる線を破線で描く。
検出箇所とkeepoutのboxは隠線処理をせず、部品と同じ投影で描く。検出箇所は部品の
内部にあることが多いためである。部品の隠線処理は方向ごとに1回だけ行い、図ごとに
使い回す。SVGは標準libraryで書く。
"""

from collections.abc import Iterable, Sequence
from dataclasses import dataclass
import math
from pathlib import Path
from xml.sax.saxutils import escape

import cadquery as cq
from OCP.BRepAdaptor import BRepAdaptor_Curve
from OCP.BRepLib import BRepLib
from OCP.GCPnts import GCPnts_QuasiUniformDeflection
from OCP.GeomAbs import GeomAbs_Line
from OCP.gp import gp_Ax2, gp_Dir, gp_Pnt
from OCP.HLRAlgo import HLRAlgo_Projector
from OCP.HLRBRep import HLRBRep_Algo, HLRBRep_HLRToShape
from OCP.TopAbs import TopAbs_EDGE
from OCP.TopExp import TopExp_Explorer
from OCP.TopoDS import TopoDS, TopoDS_Shape

from .figures import unique_file_name

__all__ = ["Projection", "View", "VIEWS", "write_projections"]

Vector = tuple[float, float, float]
Box3 = tuple[Sequence[float], Sequence[float]]
Polyline = list[tuple[float, float]]

PANEL_PX = 360
GAP_PX = 30
HEADER_PX = 40
FOOTER_PX = 30
# 曲線を折れ線にする許容誤差。部品全体の外接boxの対角長に対する比。
DEFLECTION_RATIO = 0.001
VISIBLE_STYLE = 'stroke="#2d4155" stroke-width="0.9"'
HIDDEN_STYLE = 'stroke="#aab2bd" stroke-width="0.6" stroke-dasharray="3 2"'
KEEPOUT_STYLE = 'stroke="#209059" stroke-width="0.8"'
LOCATION_STYLE = 'stroke="#d62728" stroke-width="1.6"'
# boxの12辺。角の番号は各軸の上下をbit (x=1, y=2, z=4) で表す。
BOX_EDGES = tuple(
    (corner, corner | bit) for corner in range(8) for bit in (1, 2, 4) if not corner & bit
)


def _normalized(vector: Vector) -> Vector:
    length = math.sqrt(sum(c * c for c in vector))
    return tuple(c / length for c in vector)


def _cross(a: Vector, b: Vector) -> Vector:
    return (a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0])


@dataclass(frozen=True)
class View:
    """投影の方向。normalは視点の側を向き、図の右がx_direction、上がnormal×x_directionである。"""

    name: str
    normal: Vector
    x_direction: Vector
    caption: str

    def axes(self) -> tuple[Vector, Vector]:
        right = _normalized(self.x_direction)
        return right, _cross(_normalized(self.normal), right)

    def project(self, point: Sequence[float]) -> tuple[float, float]:
        """OCCTのHLRAlgo_Projectorと同じ2D座標 (右、上)。単位はmm。"""
        right, up = self.axes()
        return sum(p * r for p, r in zip(point, right)), sum(p * u for p, u in zip(point, up))


# 第三角法の配置。上段に平面図と等角図、下段に正面図と右側面図を置く。
VIEWS = (
    View("top", (0.0, 0.0, 1.0), (1.0, 0.0, 0.0), "top (x right, y up)"),
    View("isometric", (1.0, -1.0, 1.0), (1.0, 1.0, 0.0), "isometric (from +x -y +z)"),
    View("front", (0.0, -1.0, 0.0), (1.0, 0.0, 0.0), "front (x right, z up)"),
    View("right", (1.0, 0.0, 0.0), (0.0, 1.0, 0.0), "right (y right, z up)"),
)


def _polylines(compound: TopoDS_Shape, deflection: float) -> list[Polyline]:
    """HLRが返す2Dの辺を折れ線にする。直線は両端だけを使う。"""
    if compound.IsNull():
        return []
    BRepLib.BuildCurves3d_s(compound)
    lines = []
    explorer = TopExp_Explorer(compound, TopAbs_EDGE)
    while explorer.More():
        curve = BRepAdaptor_Curve(TopoDS.Edge_s(explorer.Current()))
        if curve.GetType() == GeomAbs_Line:
            points = [curve.Value(curve.FirstParameter()), curve.Value(curve.LastParameter())]
        else:
            sampler = GCPnts_QuasiUniformDeflection(curve, deflection)
            if not sampler.IsDone():
                raise ValueError("curve discretization failed")
            points = [sampler.Value(index) for index in range(1, sampler.NbPoints() + 1)]
        lines.append([(point.X(), point.Y()) for point in points])
        explorer.Next()
    return lines


def _hidden_line_removal(shape: cq.Shape, view: View, deflection: float) -> tuple[list[Polyline], list[Polyline]]:
    """見える線と隠れる線。外形線 (曲面の輪郭) も含める。"""
    algo = HLRBRep_Algo()
    algo.Add(shape.wrapped)
    frame = gp_Ax2(gp_Pnt(0.0, 0.0, 0.0), gp_Dir(*view.normal), gp_Dir(*view.x_direction))
    algo.Projector(HLRAlgo_Projector(frame))
    algo.Update()
    algo.Hide()
    result = HLRBRep_HLRToShape(algo)
    visible = _polylines(result.VCompound(), deflection) + _polylines(result.OutLineVCompound(), deflection)
    hidden = _polylines(result.HCompound(), deflection) + _polylines(result.OutLineHCompound(), deflection)
    return visible, hidden


def _box_lines(box: Box3, view: View) -> list[Polyline]:
    low, high = box
    corners = [
        view.project([high[axis] if corner >> axis & 1 else low[axis] for axis in range(3)])
        for corner in range(8)
    ]
    return [[corners[a], corners[b]] for a, b in BOX_EDGES]


class Projection:
    """組立状態の部品を4方向へ隠線処理した線。"""

    def __init__(self, shapes: Iterable[cq.Shape]):
        compound = cq.Compound.makeCompound(list(shapes))
        deflection = DEFLECTION_RATIO * compound.BoundingBox().DiagonalLength
        self.lines = [_hidden_line_removal(compound, view, deflection) for view in VIEWS]

    def render(self, title: str, locations: Sequence[dict], keepouts: Sequence[Box3], legend: str) -> str:
        """4方向を2×2に並べたSVG。4枚は同じ縮尺で描く。"""
        layers = []
        for view, (visible, hidden) in zip(VIEWS, self.lines):
            keepout_lines = [line for box in keepouts for line in _box_lines(box, view)]
            location_lines = [
                line for location in locations for line in _box_lines((location["min"], location["max"]), view)
            ]
            layers.append([
                (HIDDEN_STYLE, hidden), (VISIBLE_STYLE, visible),
                (KEEPOUT_STYLE, keepout_lines), (LOCATION_STYLE, location_lines),
            ])
        extents = [_extent([line for _, lines in layer for line in lines]) for layer in layers]
        span = max(max(u1 - u0, v1 - v0) for u0, u1, v0, v1 in extents)
        scale = PANEL_PX / max(span, 1e-6)
        width = 2 * PANEL_PX + 3 * GAP_PX
        height = HEADER_PX + 2 * (PANEL_PX + GAP_PX) + FOOTER_PX
        body = []
        for index, (view, layer, extent) in enumerate(zip(VIEWS, layers, extents)):
            left = GAP_PX + (index % 2) * (PANEL_PX + GAP_PX)
            top = HEADER_PX + GAP_PX + (index // 2) * (PANEL_PX + GAP_PX)
            u0, u1, v0, v1 = extent
            # 内容を枠の中央に置く。SVGの縦軸は下向きのため、上の座標を反転する。
            du = left + PANEL_PX / 2 - (u0 + u1) / 2 * scale
            dv = top + PANEL_PX / 2 + (v0 + v1) / 2 * scale
            body.append(f'<text x="{left}" y="{top - 8}" font-size="13">{escape(view.caption)}</text>')
            for style, lines in layer:
                data = _path_data(lines, scale, du, dv)
                if data:
                    body.append(f'<path d="{data}" fill="none" {style}/>')
        return "\n".join([
            f'<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" '
            f'viewBox="0 0 {width} {height}" font-family="sans-serif" stroke-linecap="round">',
            f'<rect width="{width}" height="{height}" fill="#ffffff"/>',
            f'<text x="{GAP_PX}" y="22" font-size="15" font-weight="bold">{escape(title)}</text>',
            *body,
            f'<text x="{GAP_PX}" y="{height - 12}" font-size="12">{escape(legend)}; scale {scale:.2f} px/mm</text>',
            "</svg>",
            "",
        ])


def _extent(lines: list[Polyline]) -> tuple[float, float, float, float]:
    points = [point for line in lines for point in line]
    if not points:
        return 0.0, 0.0, 0.0, 0.0
    us = [u for u, _ in points]
    vs = [v for _, v in points]
    return min(us), max(us), min(vs), max(vs)


def _path_data(lines: list[Polyline], scale: float, du: float, dv: float) -> str:
    """折れ線をpath dataにする。同じ線の重複を除き、文字列順に並べて出力を辺の列挙順に依らせない。"""
    segments = {
        "M" + " L".join(f"{du + u * scale:.2f} {dv - v * scale:.2f}" for u, v in line)
        for line in lines
    }
    return " ".join(sorted(segments))


def write_projections(
    shapes: dict[str, cq.Shape], checks: list[dict], directory: str | Path, keepouts: Sequence[Box3] = (),
    overview: bool = True,
) -> list[str]:
    """組立状態の概観と、検出箇所を持つfailしたcheckごとの投影図をdirectoryへ書き、file名を返す。

    overviewが偽なら概観を書かない。
    """
    if not shapes:
        return []
    projection = Projection(shapes.values())
    target = Path(directory)
    target.mkdir(parents=True, exist_ok=True)
    keepout_text = "; green: keepout" if keepouts else ""
    figures = [("overview", "assembly: overview", [], f"solid: visible edge; dashed: hidden edge{keepout_text}")] if overview else []
    for check in checks:
        if check["status"] != "fail" or not check.get("locations"):
            continue
        locations = check["locations"]
        figures.append((
            f"{check['rule']}--{check['target']}",
            f"{check['rule']} ({check['target']}): {len(locations)} location(s) drawn",
            locations,
            f"red: reported location; solid: visible edge; dashed: hidden edge{keepout_text}",
        ))
    written: list[str] = []
    for name, title, locations, legend in figures:
        file_name = unique_file_name(name, written)
        (target / file_name).write_text(projection.render(title, locations, keepouts, legend), encoding="utf-8")
        written.append(file_name)
    return written
