"""寸法図のSVGから円を抽出し、中心と直径を出力する。

`pdftocairo -svg`で変換した図面は、円を4本の3次Bézier曲線からなる閉じたpathとして出力する。
本scriptはそのpathを検出し、祖先要素のtransformを適用した座標で中心と直径を返す。
catalogの取付穴を寸法線と照合するために使う。縮尺は図中の既知の寸法から利用者が求め、
`--mm-per-unit`に与える。
"""

import argparse
from dataclasses import dataclass
import math
from pathlib import Path
import re
import sys
import xml.etree.ElementTree as ET

Matrix = tuple[float, float, float, float, float, float]
Point = tuple[float, float]

IDENTITY: Matrix = (1.0, 0.0, 0.0, 1.0, 0.0, 0.0)
_TRANSFORM = re.compile(r"(matrix|translate|scale)\s*\(([^)]*)\)")
_TOKEN = re.compile(r"[MLCZmlcz]|-?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?")


@dataclass(frozen=True)
class Circle:
    center: Point
    diameter: float


def compose(outer: Matrix, inner: Matrix) -> Matrix:
    """outer・innerの順に適用する変換。SVGのmatrix(a b c d e f)表記に従う。"""
    a1, b1, c1, d1, e1, f1 = outer
    a2, b2, c2, d2, e2, f2 = inner
    return (
        a1 * a2 + c1 * b2,
        b1 * a2 + d1 * b2,
        a1 * c2 + c1 * d2,
        b1 * c2 + d1 * d2,
        a1 * e2 + c1 * f2 + e1,
        b1 * e2 + d1 * f2 + f1,
    )


def parse_transform(text: str | None) -> Matrix:
    result = IDENTITY
    if not text:
        return result
    for name, raw in _TRANSFORM.findall(text):
        values = [float(item) for item in re.split(r"[\s,]+", raw.strip()) if item]
        if name == "matrix":
            if len(values) != 6:
                raise ValueError(f"matrixの引数が6個でない: {raw!r}")
            step: Matrix = (values[0], values[1], values[2], values[3], values[4], values[5])
        elif name == "translate":
            step = (1.0, 0.0, 0.0, 1.0, values[0], values[1] if len(values) > 1 else 0.0)
        else:
            sy = values[1] if len(values) > 1 else values[0]
            step = (values[0], 0.0, 0.0, sy, 0.0, 0.0)
        result = compose(result, step)
    return result


def apply(matrix: Matrix, point: Point) -> Point:
    a, b, c, d, e, f = matrix
    x, y = point
    return (a * x + c * y + e, b * x + d * y + f)


def subpaths(d: str) -> list[list[tuple[str, list[float]]]]:
    """絶対座標のM/L/C/Zだけからなるpathを部分pathに分ける。相対座標のpathは扱わない。"""
    tokens = _TOKEN.findall(d)
    result: list[list[tuple[str, list[float]]]] = []
    current: list[tuple[str, list[float]]] = []
    command = ""
    arity = {"M": 2, "L": 2, "C": 6, "Z": 0}
    index = 0
    while index < len(tokens):
        token = tokens[index]
        if token.isalpha():
            if token.islower():
                return []
            command = token
            index += 1
            if command == "Z":
                current.append(("Z", []))
                continue
            if command == "M" and current:
                result.append(current)
                current = []
        count = arity[command]
        values = [float(item) for item in tokens[index:index + count]]
        if len(values) != count:
            return []
        current.append((command, values))
        index += count
    if current:
        result.append(current)
    return result


def as_circle(segments: list[tuple[str, list[float]]], matrix: Matrix, tolerance: float) -> Circle | None:
    """M、4本のC、任意のZからなり、4つの端点が正方形の外接を持つ場合に円とみなす。"""
    body = [item for item in segments if item[0] != "Z"]
    if len(body) != 5 or body[0][0] != "M" or any(command != "C" for command, _ in body[1:]):
        return None
    start = apply(matrix, (body[0][1][0], body[0][1][1]))
    ends = [apply(matrix, (values[4], values[5])) for _, values in body[1:]]
    if math.dist(start, ends[-1]) > tolerance * max(1e-9, math.dist(start, ends[1])):
        return None
    xs = [point[0] for point in ends]
    ys = [point[1] for point in ends]
    width = max(xs) - min(xs)
    height = max(ys) - min(ys)
    if width <= 0.0 or abs(width - height) > tolerance * max(width, height):
        return None
    center = ((max(xs) + min(xs)) / 2.0, (max(ys) + min(ys)) / 2.0)
    return Circle(center, (width + height) / 2.0)


def circles(svg: Path, tolerance: float = 0.02) -> list[Circle]:
    """SVG中の円を、祖先要素を含むtransformを適用した座標で返す。"""
    found: list[Circle] = []

    def walk(element: ET.Element, matrix: Matrix) -> None:
        local = compose(matrix, parse_transform(element.get("transform")))
        if element.tag.endswith("}path") and element.get("d"):
            for segments in subpaths(element.get("d", "")):
                circle = as_circle(segments, local, tolerance)
                if circle is not None:
                    found.append(circle)
        # 文字glyphの定義は図形ではないため辿らない。
        if element.tag.endswith("}defs"):
            return
        for child in element:
            walk(child, local)

    walk(ET.parse(svg).getroot(), IDENTITY)
    return found


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("svg", type=Path, help="pdftocairo -svgで変換した1ページのSVG")
    parser.add_argument("--mm-per-unit", type=float, default=1.0,
                        help="SVG座標1単位あたりのmm。図中の既知の寸法から求める")
    parser.add_argument("--min-diameter", type=float, default=0.0, help="出力する直径の下限 [mm]")
    parser.add_argument("--max-diameter", type=float, default=math.inf, help="出力する直径の上限 [mm]")
    args = parser.parse_args(argv)
    if args.mm_per_unit <= 0.0:
        parser.error("--mm-per-unitは正の値である必要がある")
    scale = args.mm_per_unit
    rows = sorted(
        (item for item in circles(args.svg)
         if args.min_diameter <= item.diameter * scale <= args.max_diameter),
        key=lambda item: (round(item.center[1], 3), item.center[0]),
    )
    print("x\ty\tdiameter")
    for item in rows:
        x, y = item.center
        print(f"{x * scale:.3f}\t{y * scale:.3f}\t{item.diameter * scale:.3f}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
