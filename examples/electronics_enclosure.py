"""穴付きboss、コネクタ開口、通気スリットを持つ上面開放の筐体。

寸法は説明用で、特定の基板やコネクタの仕様ではない。
"""

import argparse
from pathlib import Path
import sys

from typedsolid import Box, Clearance, Feature, Keepout, Model, Part, boss, hole
from typedsolid.cadquery import DEFAULT_TIMEOUT_S, export

LENGTH_MM, WIDTH_MM, HEIGHT_MM = 80.0, 55.0, 24.0
WALL_MM = 2.0
# bossの上面。基板下面はこの高さに接する。
BOARD_Z_MM = 6.0
BOSS_DIAMETER_MM = 8.0
HOLE_DIAMETER_MM = 3.0
BOSS_CENTERS = {"a": (10.0, 10.0), "b": (70.0, 10.0), "c": (10.0, 45.0), "d": (70.0, 45.0)}
VENT_X_MM = (22.0, 30.0, 38.0, 46.0, 54.0)


def electronics_enclosure() -> Model:
    # 全Addの和から全Cutを引くため、床と壁を個別にAddして内部を空ける。
    features = [
        Feature("floor", Box((0, 0, 0), (LENGTH_MM, WIDTH_MM, WALL_MM)), "base"),
        Feature("left", Box((0, 0, 0), (WALL_MM, WIDTH_MM, HEIGHT_MM)), "wall"),
        Feature("right", Box((LENGTH_MM - WALL_MM, 0, 0), (LENGTH_MM, WIDTH_MM, HEIGHT_MM)), "wall"),
        Feature("front", Box((0, 0, 0), (LENGTH_MM, WALL_MM, HEIGHT_MM)), "wall"),
        Feature("back", Box((0, WIDTH_MM - WALL_MM, 0), (LENGTH_MM, WIDTH_MM, HEIGHT_MM)), "wall"),
        # 前面の幅20 mmのコネクタ用切り欠き。z=7から上端まで開け、壁を貫くよう前後と上へ1 mm延ばす。
        # 上端を閉じた窓にすると上辺が幅20 mmのbridgeになり、既定のbridge_max_mm (5 mm) で
        # support_freeが落ちる。印刷機がそれ以上を渡せる場合はPolicyで指定して窓にできる。
        Feature("connector", Box((30, -1, 7), (50, WALL_MM + 1, HEIGHT_MM + 1)), operation="cut"),
    ]
    for name, (x, y) in BOSS_CENTERS.items():
        # bossは床へ1 mm重ね、穴は床とbossを貫通する。
        features.append(boss(f"boss_{name}", "z", (x, y), BOSS_DIAMETER_MM, (1.0, BOARD_Z_MM)))
        features.append(hole(f"hole_{name}", "z", (x, y), HOLE_DIAMETER_MM, (-1.0, BOARD_Z_MM + 1.0)))
    for index, x in enumerate(VENT_X_MM):
        features.append(Feature(
            f"vent_{index}", Box((x, WIDTH_MM - WALL_MM - 1, 10), (x + 3, WIDTH_MM + 1, 20)), operation="cut",
        ))
    return Model(
        parts=(Part("electronics_enclosure", tuple(features)),),
        # 基板はbossの上面に接するため、下面のclearanceだけを0とする。
        keepouts=(
            Keepout(
                "pcb", Box((10, 10, BOARD_Z_MM), (70, 45, BOARD_Z_MM + 3)),
                Clearance(default=0.5, minus_z=0.0), access=("plus_z",),
            ),
        ),
    )


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, default=Path(".work/electronics-enclosure"))
    parser.add_argument("--timeout", type=float, default=DEFAULT_TIMEOUT_S, help="打ち切りまでの秒数")
    parser.add_argument("--cache", type=Path, default=None, help="部品単位の結果を置く場所")
    args = parser.parse_args()
    manifest = export(
        electronics_enclosure(), args.output, timeout_s=args.timeout, cache_dir=args.cache,
        progress=lambda stage: print(stage, file=sys.stderr),
    )
    print(f"Exported electronics_enclosure.stl and electronics_enclosure.step to {args.output}")
    print("Unevaluated: " + ", ".join(c["rule"] for c in manifest["report"]["checks"] if c["status"] == "not_evaluated"))
