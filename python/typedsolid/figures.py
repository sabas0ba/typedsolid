"""検査結果の断面図。部品の概観と、検出箇所を通る3断面を、判定と同じvoxel格子から描く。

判定と図が同じ格子とmaskを使うため、図に塗った箇所は判定の根拠そのものである。
SVGは標準libraryだけで書く。CadQueryに依存しないため、外部形状にも使える。
"""

from collections.abc import Callable
from dataclasses import dataclass
import json
from pathlib import Path
from xml.sax.saxutils import escape

from . import _native

__all__ = ["Figure", "plan", "render", "write_model_figures", "write_stl_figures"]

# 断面の符号のbit。Rust coreのSECTION_*と同じ値。
SOLID, THIN, NECK, UNSUPPORTED, VOID = 1, 2, 4, 8, 16
# ruleごとに強調するbit、色、凡例に書く色名と意味。connector_fitと製造法ごとのrule
# (UV樹脂、切削、射出成形) は格子のbitを持たず、検出箇所の枠だけを描く。
HIGHLIGHT = {
    "final_wall_thickness": (THIN, "#d62728", "red: thinner than min_wall_mm"),
    "neck_section": (NECK, "#9467bd", "purple: connection below min_neck_mm"),
    "support_free": (UNSUPPORTED, "#1f77b4", "blue: unsupported"),
    "closed_cavity": (VOID, "#ff7f0e", "orange: enclosed void"),
    "connector_fit": (0, "#d62728", ""),
    "resin_drain": (0, "#d62728", ""),
    "resin_suction": (0, "#d62728", ""),
    "milling_reach": (0, "#d62728", ""),
    "milling_corner": (0, "#d62728", ""),
    "mold_undercut": (0, "#d62728", ""),
    "mold_thick_wall": (0, "#d62728", ""),
}
SOLID_COLOUR = "#c9ced6"
# 1つのcheckについて描く検出箇所の数。locationsは大きい順に並ぶ。
FIGURES_PER_CHECK = 3
# 1回の呼び出しで求める断面の数。Rust coreの上限 (256) 以下で3の倍数とする。
SECTIONS_PER_CALL = 255
# 検出箇所の周囲に含める範囲の下限。単位はmm。
MIN_WINDOW_MM = 10.0
PANEL_PX = 380
HEADER_PX = 40
AXES = ("x", "y", "z")
# 断面の法線ごとの、図の横軸と縦軸。Rust coreのAxis::planeと同じ順である。
PLANE_AXES = {"x": (1, 2), "y": (0, 2), "z": (0, 1)}

Box3 = tuple[tuple[float, float, float], tuple[float, float, float]]


@dataclass(frozen=True)
class Figure:
    """1枚の図。locationがNoneなら部品全体の概観で、全ruleのbitを塗る。

    planは断面を判定する製造案のidで、Noneなら採用した製造案である。
    """

    name: str
    part: str
    rule: str | None
    title: str
    location: dict | None
    plan: str | None = None

    def centre(self) -> tuple[float, float, float] | None:
        if self.location is None:
            return None
        low, high = self.location["min"], self.location["max"]
        return tuple((low[i] + high[i]) / 2.0 for i in range(3))

    def window(self) -> Box3 | None:
        """描く範囲。検出箇所の大きさの3倍とMIN_WINDOW_MMの大きい方を、箇所を中心に取る。"""
        if self.location is None:
            return None
        low, high = self.location["min"], self.location["max"]
        centre = self.centre()
        half = [max(1.5 * (high[i] - low[i]), MIN_WINDOW_MM / 2.0) for i in range(3)]
        return (
            tuple(centre[i] - half[i] for i in range(3)),
            tuple(centre[i] + half[i] for i in range(3)),
        )


def _owner(check: dict, model: dict) -> str | None:
    """checkが指す部品のid。voxel ruleはtargetが部品、connector_fitはconnectorの部品。"""
    if check["rule"] == "connector_fit":
        return next((c["part"] for c in model.get("connectors", []) if c["id"] == check["target"]), None)
    if check["rule"] in HIGHLIGHT:
        return check["target"]
    return None


def plan(model: dict, checks: list[dict]) -> list[Figure]:
    """各部品の概観と、failしたcheckの検出箇所ごとの図を決める。"""
    figures = [
        Figure(f"{part['id']}--overview", part["id"], None, f"{part['id']}: overview", None)
        for part in model["parts"]
    ]
    for check in checks:
        if check["status"] != "fail" or not check.get("locations"):
            continue
        owner = _owner(check, model)
        if owner is None:
            continue
        total = len(check["locations"])
        # 同じ部品・ruleに複数のcheckがある場合 (connector_fit) に名前が衝突しないよう、
        # targetが部品と異なればtargetも名前に含める。
        stem = f"{owner}--{check['rule']}"
        if check["target"] != owner:
            stem += f"--{check['target']}"
        # 採用していない製造案の図は、その製造案で判定した断面を描き、名前に製造案を含める。
        plan_id = None if check.get("adopted", True) else check.get("plan")
        heading = check["target"] if plan_id is None else f"{check['target']}, plan {plan_id} (not adopted)"
        if plan_id is not None:
            stem += f"--{plan_id}"
        for index, location in enumerate(check["locations"][:FIGURES_PER_CHECK]):
            figures.append(Figure(
                f"{stem}--{index + 1}", owner, check["rule"],
                f"{owner}: {check['rule']} ({heading}), location {index + 1} of {total}",
                location, plan_id,
            ))
    return figures


def _planes(figure: Figure) -> list[dict]:
    """3断面。概観は座標を省き、格子の中央を切る。"""
    centre = figure.centre()
    return [
        {"axis": axis} if centre is None else {"axis": axis, "coordinate": centre[i]}
        for i, axis in enumerate(AXES)
    ]


def _extent(section: dict) -> tuple[float, float, float, float]:
    """断面全体の範囲 (横の最小、横の最大、縦の最小、縦の最大)。"""
    pitch = section["pitch"]
    width, height = len(section["rows"][0]), len(section["rows"])
    u, v = section["origin"]
    return u - pitch / 2, u + (width - 0.5) * pitch, v - pitch / 2, v + (height - 0.5) * pitch


def _view(section: dict, window: Box3 | None) -> tuple[float, float, float, float]:
    """図に描く範囲。窓が無ければ断面全体とする。"""
    u0, u1, v0, v1 = _extent(section)
    if window is None:
        return u0, u1, v0, v1
    first, second = PLANE_AXES[section["axis"]]
    low, high = window
    return max(u0, low[first]), min(u1, high[first]), max(v0, low[second]), min(v1, high[second])


def _fill(figure: Figure, value: int) -> str | None:
    if figure.rule is None:
        for bit, colour, _ in HIGHLIGHT.values():
            if bit and value & bit:
                return colour
    else:
        bit, colour, _ = HIGHLIGHT[figure.rule]
        if bit and value & bit:
            return colour
    return SOLID_COLOUR if value & SOLID else None


def _panel(section: dict, figure: Figure, view, left_px: float, scale: float) -> list[str]:
    """断面1枚。範囲に入るcellを、行ごとに同じ色の連続をまとめて描く。"""
    first, second = PLANE_AXES[section["axis"]]
    u0, u1, v0, v1 = view
    pitch = section["pitch"]
    ou, ov = section["origin"]
    rows = section["rows"]
    i0, i1 = max(0, round((u0 - ou) / pitch)), min(len(rows[0]), round((u1 - ou) / pitch) + 1)
    j0, j1 = max(0, round((v0 - ov) / pitch)), min(len(rows), round((v1 - ov) / pitch) + 1)

    def x_px(u: float) -> float:
        return left_px + (u - u0) * scale

    def y_px(v: float) -> float:
        return HEADER_PX + (v1 - v) * scale

    out = []
    for j in range(j0, j1):
        run_start, run_fill = i0, None
        # 行末に番兵 (i1) を置き、最後の連続を書き出す。
        for i in range(i0, i1 + 1):
            fill = None if i == i1 else _fill(figure, int(rows[j][i], 32))
            if fill == run_fill:
                continue
            if run_fill is not None:
                u = ou + (run_start - 0.5) * pitch
                out.append(
                    f'<rect x="{x_px(u):.2f}" y="{y_px(ov + (j + 0.5) * pitch):.2f}" '
                    f'width="{(i - run_start) * pitch * scale:.2f}" height="{pitch * scale:.2f}" fill="{run_fill}"/>'
                )
            run_start, run_fill = i, fill
    if figure.location is not None:
        low, high = figure.location["min"], figure.location["max"]
        out.append(
            f'<rect x="{x_px(low[first]):.2f}" y="{y_px(high[second]):.2f}" '
            f'width="{(high[first] - low[first]) * scale:.2f}" height="{(high[second] - low[second]) * scale:.2f}" '
            'fill="none" stroke="#111111" stroke-width="1.5" stroke-dasharray="5 3"/>'
        )
    label = f"{section['axis']} = {section['plane_coordinate']:.2f} mm ({AXES[first]} right, {AXES[second]} up)"
    out.append(f'<text x="{left_px:.0f}" y="{HEADER_PX - 8}" font-size="13">{escape(label)}</text>')
    return out


def render(figure: Figure, sections: list[dict]) -> str:
    """3断面を横に並べたSVG。3枚は同じ縮尺で描く。"""
    views = [_view(section, figure.window()) for section in sections]
    span = max(max(u1 - u0, v1 - v0) for u0, u1, v0, v1 in views)
    scale = PANEL_PX / max(span, 1e-6)
    width = 3 * (PANEL_PX + 30) + 10
    height = HEADER_PX + PANEL_PX + 40
    body = []
    for index, (section, view) in enumerate(zip(sections, views)):
        body += _panel(section, figure, view, 10 + index * (PANEL_PX + 30), scale)
    if figure.rule is None:
        legend = "grey: material; " + "; ".join(text for bit, _, text in HIGHLIGHT.values() if bit)
    else:
        text = HIGHLIGHT[figure.rule][2]
        legend = "grey: material; dashed: reported location" + (f"; {text}" if text else "")
    return "\n".join([
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" '
        f'viewBox="0 0 {width} {height}" font-family="sans-serif" shape-rendering="crispEdges">',
        f'<rect width="{width}" height="{height}" fill="#ffffff"/>',
        f'<text x="10" y="18" font-size="15" font-weight="bold">{escape(figure.title)}</text>',
        *body,
        f'<text x="10" y="{height - 12}" font-size="12">{escape(legend)}; scale {scale:.2f} px/mm</text>',
        "</svg>",
        "",
    ])


def _file_name(name: str) -> str:
    """図の名前をdirectory直下のfile名にする。英数字と`._-`以外は`_`に置き換え、
    先頭の`.`も置き換える。外部形状のtargetは識別子として検証されないためである。"""
    safe = "".join(c if c.isascii() and (c.isalnum() or c in "._-") else "_" for c in name)
    return ("_" + safe[1:] if safe.startswith(".") else safe) + ".svg"


def unique_file_name(name: str, written: list[str]) -> str:
    """writtenと重ならないfile名。置き換えで名前が重なった場合は連番を付け、先の図を上書きしない。"""
    candidate = _file_name(name)
    suffix = 2
    while candidate in written:
        candidate = _file_name(f"{name}--{suffix}")
        suffix += 1
    return candidate


def _write(
    figures: list[Figure], sections_of: Callable[[str, str | None, str], str], directory: Path,
) -> list[str]:
    """部品と製造案ごとに断面を求め、図をdirectoryへ書く。断面はSECTIONS_PER_CALL枚ずつ求める。"""
    directory.mkdir(parents=True, exist_ok=True)
    written: list[str] = []
    by_part: dict[tuple[str, str | None], list[Figure]] = {}
    for figure in figures:
        by_part.setdefault((figure.part, figure.plan), []).append(figure)
    for (part, plan_id), group in by_part.items():
        per_call = SECTIONS_PER_CALL // 3
        for start in range(0, len(group), per_call):
            batch = group[start:start + per_call]
            planes = [plane for figure in batch for plane in _planes(figure)]
            sections = json.loads(sections_of(part, plan_id, json.dumps(planes)))
            for index, figure in enumerate(batch):
                name = unique_file_name(figure.name, written)
                path = directory / name
                path.write_text(render(figure, sections[3 * index:3 * index + 3]), encoding="utf-8")
                written.append(name)
    return written


def write_model_figures(model_json: str, checks: list[dict], directory: str | Path) -> list[str]:
    """IRのモデルについて、概観と検出箇所の図をdirectoryへ書き、file名を返す。"""
    return _write(
        plan(json.loads(model_json), checks),
        lambda part, plan_id, planes: _native.voxel_sections(model_json, part, plan_id, planes),
        Path(directory),
    )


def write_stl_figures(
    stl: bytes, policy_json: str, plan_json: str, target: str, checks: list[dict], directory: str | Path,
) -> list[str]:
    """外部のSTLについて、概観と検出箇所の図をdirectoryへ書き、file名を返す。plan_jsonは製造案。"""
    return _write(
        plan({"parts": [{"id": target}]}, checks),
        lambda _part, _plan, planes: _native.stl_sections(stl, policy_json, plan_json, planes),
        Path(directory),
    )
