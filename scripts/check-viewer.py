"""部品間の欠陥fixtureの3D viewerを書き出し、headless Chromiumで表示状態ごとに撮影して画素を検査する。

Chromiumは既定でdocker/viewerのimageの中で、networkを持たずに動かす。--chromiumで手元の
binaryを指定することもできる。表示状態はURL fragmentで与える。各状態で、部品、検出箇所、
半透明、断面の色が画面に現れることを確かめる。WebGLやscriptが失敗すると画面は白く残り、
検査が落ちる。PNGは標準libraryで読む。
"""

import argparse
from collections.abc import Callable
from dataclasses import dataclass
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess
import sys
import zlib

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from examples.assembly_defects import ASSEMBLY_DEFECTS
from typedsolid.cadquery import build, write_viewer_figure

WIDTH, HEIGHT = 1200, 760
# 右側のpanelを除いた描画領域の幅。viewer.cssの列幅 (380 px) に合わせる。
CANVAS_WIDTH = WIDTH - 380
CHROMIUM_FLAGS = (
    "--headless", "--no-sandbox", "--use-angle=swiftshader", "--enable-unsafe-swiftshader",
    "--hide-scrollbars", "--disable-dev-shm-usage",
)
# 撮影の後、docsに置く画像。fixture名と状態名の組。
DOCS = (("post_in_keepout", "selected"), ("lid_overlap", "section"))


# ---- PNG ----

def _paeth(a: int, b: int, c: int) -> int:
    p = a + b - c
    pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
    if pa <= pb and pa <= pc:
        return a
    return b if pb <= pc else c


def read_png(path: Path) -> tuple[int, int, int, bytes]:
    """8 bit、非interlaceのRGBまたはRGBAのPNGを読み、幅、高さ、画素の大きさ、画素列を返す。"""
    data = path.read_bytes()
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise ValueError(f"{path} is not a PNG file")
    offset, chunks, header = 8, [], None
    while offset < len(data):
        length, kind = struct.unpack(">I4s", data[offset:offset + 8])
        body = data[offset + 8:offset + 8 + length]
        if kind == b"IHDR":
            header = struct.unpack(">IIBBBBB", body)
        elif kind == b"IDAT":
            chunks.append(body)
        offset += 12 + length
    width, height, depth, colour, _, _, interlace = header
    if depth != 8 or colour not in (2, 6) or interlace:
        raise ValueError(f"{path}: unsupported PNG format")
    size = 3 if colour == 2 else 4
    raw = zlib.decompress(b"".join(chunks))
    stride = width * size
    pixels = bytearray(height * stride)
    previous = bytearray(stride)
    for row in range(height):
        start = row * (stride + 1)
        kind = raw[start]
        line = bytearray(raw[start + 1:start + 1 + stride])
        if kind == 1:
            for i in range(size, stride):
                line[i] = (line[i] + line[i - size]) & 0xFF
        elif kind == 2:
            for i in range(stride):
                line[i] = (line[i] + previous[i]) & 0xFF
        elif kind == 3:
            for i in range(stride):
                left = line[i - size] if i >= size else 0
                line[i] = (line[i] + ((left + previous[i]) >> 1)) & 0xFF
        elif kind == 4:
            for i in range(stride):
                left = line[i - size] if i >= size else 0
                upper_left = previous[i - size] if i >= size else 0
                line[i] = (line[i] + _paeth(left, previous[i], upper_left)) & 0xFF
        pixels[row * stride:(row + 1) * stride] = line
        previous = line
    return width, height, size, bytes(pixels)


# ---- 画素の分類 ----

def is_location(r: int, g: int, b: int) -> bool:
    """検出箇所の赤。線と、注視時に部品へ重ねる半透明の塗りを含む。部品色は赤みを持たない。"""
    return r > 90 and r - g > 40 and r - b > 35


def is_part(r: int, g: int, b: int) -> bool:
    """陰影を付けた部品の色。部品の既定色はいずれも彩度が低く、白と赤ではない。"""
    return 60 < max(r, g, b) < 225 and max(r, g, b) - min(r, g, b) > 12 and not is_location(r, g, b)


def is_cut(r: int, g: int, b: int) -> bool:
    """断面の切り口から見える裏面の灰色 (viewer.jsのCUT_COLOUR)。"""
    return abs(r - 89) <= 6 and abs(g - 94) <= 6 and abs(b - 102) <= 6


def fractions(path: Path) -> dict[str, float]:
    """描画領域の画素のうち、各分類に入る割合。速さのため2画素おきに数える。"""
    width, height, size, pixels = read_png(path)
    counts = {"location": 0, "part": 0, "cut": 0}
    total = 0
    for y in range(0, height, 2):
        for x in range(0, min(width, CANVAS_WIDTH), 2):
            i = (y * width + x) * size
            r, g, b = pixels[i], pixels[i + 1], pixels[i + 2]
            total += 1
            counts["location"] += is_location(r, g, b)
            counts["part"] += is_part(r, g, b)
            counts["cut"] += is_cut(r, g, b)
    return {key: value / total for key, value in counts.items()}


# ---- 撮影 ----

@dataclass(frozen=True)
class Scene:
    name: str
    fragment: str
    # 画素の割合と、同じfixtureで先に撮った状態の割合から、満たされなかった項目を返す。
    expect: Callable[[dict[str, float], dict[str, dict[str, float]]], list[str]]


def scenes(model: dict, checks: list[dict]) -> list[Scene]:
    """fixtureごとの表示状態と、画面に現れるべきもの。"""
    failing = next(i for i, check in enumerate(checks) if check["status"] == "fail")
    parts = ",".join(part["id"] for part in model["parts"])
    # 断面はz方向の中央。部品の内側の面が切り口から見える。
    top = max(f["shape"]["max"][2] for part in model["parts"] for f in part["features"] if f["shape"]["kind"] == "box")
    return [
        Scene("overview", "", lambda f, _: _require(f, part=0.05, location=0.0005)),
        Scene("selected", f"#check={failing}&ghost={parts}", lambda f, _: _require(f, part=0.02, location=0.0005)),
        # 注視では検出箇所を寄せて半透明の赤で塗る。線だけの状態より赤が十分に多い。
        Scene("focus", f"#check={failing}&loc=0", lambda f, seen: _require(f, location=3 * seen["selected"]["location"])),
        Scene("section", f"#clip=z:{top / 2:.3f}&view=35,60,1", lambda f, _: _require(f, part=0.05, cut=0.001)),
    ]


def _require(found: dict[str, float], **minimum: float) -> list[str]:
    return [f"{key} {found[key]:.4f} < {value}" for key, value in minimum.items() if found[key] < value]


def chromium_command(args: argparse.Namespace, directory: Path) -> tuple[list[str], Path]:
    """Chromiumを起動するcommandの前半と、Chromiumから見たdirectoryのpath。"""
    if args.chromium:
        return [args.chromium, *CHROMIUM_FLAGS], directory.resolve()
    user = f"{os.getuid()}:{os.getgid()}"
    return [
        "docker", "run", "--rm", "--network", "none", "--user", user, "--env", "HOME=/tmp",
        "--volume", f"{directory.resolve()}:/work", args.image,
    ], Path("/work")


def shoot(command: list[str], inside: Path, html: str, fragment: str, png: str) -> None:
    subprocess.run(
        [*command, f"--window-size={WIDTH},{HEIGHT}", "--virtual-time-budget=5000",
         f"--screenshot={inside / png}", f"file://{inside / html}{fragment}"],
        check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=120,
    )


def run(args: argparse.Namespace) -> list[str]:
    """撮影して検査し、満たされなかった項目を返す。"""
    output = args.output
    output.mkdir(parents=True, exist_ok=True)
    command, inside = chromium_command(args, output)
    problems = []
    for factory, _ in ASSEMBLY_DEFECTS:
        seen: dict[str, dict[str, float]] = {}
        model = factory()
        result = build(model)
        html = f"{factory.__name__}.html"
        write_viewer_figure(result, output / html)
        for scene in scenes(json.loads(result.model_json), result.report["checks"]):
            png = f"{factory.__name__}--{scene.name}.png"
            shoot(command, inside, html, scene.fragment, png)
            found = fractions(output / png)
            missing = scene.expect(found, seen)
            seen[scene.name] = found
            print(f"{png}: " + ", ".join(f"{key} {value:.4f}" for key, value in found.items()) + (" FAIL" if missing else ""))
            problems += [f"{png}: {item}" for item in missing]
    if args.docs:
        args.docs.mkdir(parents=True, exist_ok=True)
        for fixture, scene in DOCS:
            shutil.copyfile(output / f"{fixture}--{scene}.png", args.docs / f"{fixture}--{scene}.png")
    return problems


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--output", type=Path, default=Path(".work/viewer-check"))
    parser.add_argument("--image", default="typedsolid-viewer", help="docker/viewerから作ったimageのtag")
    parser.add_argument("--chromium", help="containerを使わず、手元のChromiumのbinaryで撮影する")
    parser.add_argument("--docs", type=Path, help="docsに置く画像の複製先 (docs/assets/viewer)")
    problems = run(parser.parse_args(argv))
    for problem in problems:
        print(problem, file=sys.stderr)
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
