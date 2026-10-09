"""検査結果の3D viewer。部品のmesh、keepout、checkと検出箇所を1つのHTMLに埋め込む。

HTMLは外部の資源を読まず、file://で開ける。描画は自作のWebGL 1で行い、JavaScriptの
packageに依存しない。表示状態 (選択したcheck、注視する検出箇所、断面、部品の表示) は
URL fragmentに書き、同じURLで同じ表示を再現できる。

本moduleはCadQueryに依存しない。CadQueryの形状からmeshを作る処理は
typedsolid.cadqueryが持ち、外部のSTLは判定に使った三角形をそのまま描く。
"""

from array import array
from collections.abc import Sequence
import base64
from dataclasses import dataclass
import html
import json
from pathlib import Path
import re
import sys

from . import _native

__all__ = ["MeshPart", "render_viewer", "stl_part", "write_viewer"]

ASSETS = Path(__file__).resolve().parent / "viewer_assets"
# 外接boxの座標を丸める小数点以下の桁数。OCCTの許容差による端数を除く。検出箇所と同じ桁である。
BOUNDS_DIGITS = 6
PLACEHOLDER = re.compile(r"\{\{(TITLE|STYLE|DATA|CORE|APP)\}\}")

Box3 = tuple[Sequence[float], Sequence[float]]


@dataclass(frozen=True)
class MeshPart:
    """viewerに描く1部品の三角形。

    positionsは頂点座標をlittle endianのf32で並べたbytes。indicesは三角形ごとの頂点番号を
    little endianのu32で並べたbytesで、Noneならpositionsが三角形ごとに3頂点を並べたものとする。
    """

    id: str
    positions: bytes
    indices: bytes | None = None

    def triangle_count(self) -> int:
        return (len(self.indices) // 12) if self.indices is not None else len(self.positions) // 36

    def bounds(self) -> dict:
        """頂点の外接box。座標は4 byteのまま走査し、三角形の多いmeshでも全座標をPythonのfloatにしない。"""
        coordinates = array("f")
        coordinates.frombytes(self.positions)
        if sys.byteorder == "big":
            coordinates.byteswap()
        return {
            key: [round(pick(coordinates[axis::3]), BOUNDS_DIGITS) + 0.0 for axis in range(3)]
            for key, pick in (("min", min), ("max", max))
        }


def stl_part(id: str, stl: bytes) -> MeshPart:
    """STLの三角形をそのまま描く部品。外部形状の判定に使ったmeshと同じである。"""
    return MeshPart(id, _native.stl_triangles(stl))


def _base64(data: bytes) -> str:
    return base64.b64encode(data).decode("ascii")


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


def _part_data(part: MeshPart) -> dict:
    data = {"id": part.id, "bounds": part.bounds(), "positions": _base64(part.positions)}
    if part.indices is not None:
        data["indices"] = _base64(part.indices)
    return data


def render_viewer(
    parts: Sequence[MeshPart], checks: list[dict], keepouts: Sequence[tuple[str, Box3]] = (),
    title: str = "TypedSolid",
) -> str:
    """viewerのHTML。partsは三角形を1つ以上持つ部品に限ること。"""
    data = {
        "title": title,
        "units": "mm",
        "parts": [_part_data(part) for part in parts],
        "keepouts": [{"id": keepout_id, "min": list(low), "max": list(high)} for keepout_id, (low, high) in keepouts],
        "checks": [
            {
                key: check[key]
                for key in ("rule", "target", "status", "message", "locations", "plan", "adopted")
                if key in check
            }
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
    parts: Sequence[MeshPart], checks: list[dict], path: str | Path,
    keepouts: Sequence[tuple[str, Box3]] = (), title: str = "TypedSolid",
) -> Path:
    """viewerのHTMLをpathへ書く。"""
    target = Path(path)
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(render_viewer(parts, checks, keepouts, title), encoding="utf-8")
    return target
