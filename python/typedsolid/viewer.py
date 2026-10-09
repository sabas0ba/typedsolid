"""検査結果の3D viewer。部品のmesh、keepout、checkと検出箇所を1つのHTMLに埋め込む。

HTMLは外部の資源を読まず、file://で開ける。描画は自作のWebGL 1で行い、JavaScriptの
packageに依存しない。表示状態 (選択したcheck、注視する検出箇所、断面、部品の表示) は
URL fragmentに書き、同じURLで同じ表示を再現できる。
"""

from collections.abc import Sequence
import base64
import html
import json
from pathlib import Path
import re
import struct

import cadquery as cq

__all__ = ["render_viewer", "write_viewer"]

ASSETS = Path(__file__).resolve().parent / "viewer_assets"
# 曲面を三角形にする許容誤差。部品全体の外接boxの対角長に対する比と、角度 (rad)。
TESSELLATION_RATIO = 0.001
ANGULAR_TOLERANCE = 0.2
# 外接boxの座標を丸める小数点以下の桁数。OCCTの許容差による端数を除く。検出箇所と同じ桁である。
BOUNDS_DIGITS = 6
PLACEHOLDER = re.compile(r"\{\{(TITLE|STYLE|DATA|CORE|APP)\}\}")

Box3 = tuple[Sequence[float], Sequence[float]]


def _base64(fmt: str, values: list) -> str:
    """little endianの配列をbase64にする。viewer_core.jsのdecode*が読む。"""
    return base64.b64encode(struct.pack(f"<{len(values)}{fmt}", *values)).decode("ascii")


def _mesh(part_id: str, shape: cq.Shape, tolerance: float) -> dict:
    vertices, triangles = shape.tessellate(tolerance, ANGULAR_TOLERANCE)
    box = shape.BoundingBox()
    return {
        "id": part_id,
        "bounds": {
            "min": [round(v, BOUNDS_DIGITS) + 0.0 for v in (box.xmin, box.ymin, box.zmin)],
            "max": [round(v, BOUNDS_DIGITS) + 0.0 for v in (box.xmax, box.ymax, box.zmax)],
        },
        "positions": _base64("f", [c for v in vertices for c in (v.x, v.y, v.z)]),
        "indices": _base64("I", [i for triangle in triangles for i in triangle]),
    }


def _script_json(value: object) -> str:
    """`<script type="application/json">`に埋め込むJSON。`<`、`>`、`&`をescapeし、
    文字列中の`</script>`で要素が閉じないようにする。"""
    text = json.dumps(value, ensure_ascii=False, allow_nan=False, separators=(",", ":"))
    return text.replace("&", "\\u0026").replace("<", "\\u003c").replace(">", "\\u003e")


def _asset(name: str) -> str:
    return (ASSETS / name).read_text(encoding="utf-8")


def _embedded(name: str) -> str:
    """scriptとstyleの要素へそのまま入れる資源。要素を閉じる文字列を含めない。"""
    text = _asset(name)
    if re.search(r"</(script|style)", text, re.IGNORECASE):
        raise ValueError(f"{name} must not contain a closing script or style tag")
    return text


def render_viewer(
    shapes: dict[str, cq.Shape], checks: list[dict], keepouts: Sequence[tuple[str, Box3]] = (), title: str = "TypedSolid",
) -> str:
    """viewerのHTML。shapesは描ける (体積を持つ) 部品に限ること。"""
    boxes = [shape.BoundingBox() for shape in shapes.values()]
    diagonal = max((box.DiagonalLength for box in boxes), default=1.0)
    tolerance = TESSELLATION_RATIO * diagonal
    data = {
        "title": title,
        "units": "mm",
        "parts": [_mesh(part_id, shape, tolerance) for part_id, shape in shapes.items()],
        "keepouts": [{"id": keepout_id, "min": list(low), "max": list(high)} for keepout_id, (low, high) in keepouts],
        "checks": [
            {key: check[key] for key in ("rule", "target", "status", "message", "locations") if key in check}
            for check in checks
        ],
    }
    values = {
        "TITLE": html.escape(title),
        "STYLE": _embedded("viewer.css"),
        "DATA": _script_json(data),
        "CORE": _embedded("viewer_core.js"),
        "APP": _embedded("viewer.js"),
    }
    # 1回の走査で置き換える。埋め込んだ値の中の`{{...}}`は再び置き換えない。
    return PLACEHOLDER.sub(lambda match: values[match.group(1)], _asset("viewer.html"))


def write_viewer(
    shapes: dict[str, cq.Shape], checks: list[dict], path: str | Path,
    keepouts: Sequence[tuple[str, Box3]] = (), title: str = "TypedSolid",
) -> Path:
    """viewerのHTMLをpathへ書く。"""
    target = Path(path)
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(render_viewer(shapes, checks, keepouts, title), encoding="utf-8")
    return target
