"""catalogの基板寸法から組み立てる、上面アクセスのトレイ。"""

import argparse
from pathlib import Path
import sys

from typedsolid import Box, Clearance, Feature, Model, Part, board
from typedsolid.cadquery import DEFAULT_TIMEOUT_S, export

TRAY = (60.0, 40.0, 20.0)
WALL_MM = 2.0
FLOOR_MM = 2.0
# 支持padの上面。基板下面はこの高さに接する。
BOARD_Z_MM = 4.0
PAD_DIAMETER_MM = 5.0
# Pico 2の取付穴はφ2.1でM2相当。padへのネジ下穴として1.6を開ける。
SCREW_DIAMETER_MM = 1.6
# データシートは部品高さを与えないため、作例側で確保する高さを決める。
BOARD_HEIGHT_MM = 5.0


def board_tray() -> Model:
    pico = board("raspberry_pi_pico_2")
    length, width, height = TRAY
    # 基板を平面の中央に置く。originは基板座標系の原点に対応するmodel座標。
    origin = ((length - pico.length_mm) / 2.0, (width - pico.width_mm) / 2.0, BOARD_Z_MM)

    shell = (
        Feature("floor", Box((0, 0, 0), (length, width, FLOOR_MM)), "base"),
        Feature("left", Box((0, 0, 0), (WALL_MM, width, height)), "wall"),
        Feature("right", Box((length - WALL_MM, 0, 0), (length, width, height)), "wall"),
        Feature("front", Box((0, 0, 0), (length, WALL_MM, height)), "wall"),
        Feature("back", Box((0, width - WALL_MM, 0), (length, width, height)), "wall"),
    )
    # padは円柱、ネジ穴は底板を貫通するcut。Cutは全Addを結合した後に差し引く。
    pads = pico.bosses((0.0, BOARD_Z_MM), PAD_DIAMETER_MM, origin)
    screws = pico.pilot_holes((-1.0, BOARD_Z_MM), SCREW_DIAMETER_MM, origin)

    return Model(
        parts=(Part("board_tray", shell + pads + screws),),
        # 支持面への接触を許可するため下面のclearanceは0。他の面は0.5を確保する。
        keepouts=(
            pico.keepout(
                "pcb",
                origin,
                height_mm=BOARD_HEIGHT_MM,
                clearance=Clearance(default=0.5, minus_z=0.0),
                access=("plus_z",),
            ),
        ),
    )


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, default=Path(".work/board-tray"))
    parser.add_argument("--timeout", type=float, default=DEFAULT_TIMEOUT_S, help="打ち切りまでの秒数")
    parser.add_argument("--cache", type=Path, default=Path(".work/cache"), help="部品単位の結果を置く場所")
    args = parser.parse_args()
    manifest = export(
        board_tray(), args.output, timeout_s=args.timeout, cache_dir=args.cache,
        progress=lambda stage: print(stage, file=sys.stderr),
    )
    print(f"Exported board_tray.stl and board_tray.step to {args.output}")
    print("Unevaluated: " + ", ".join(c["rule"] for c in manifest["report"]["checks"] if c["status"] == "not_evaluated"))
