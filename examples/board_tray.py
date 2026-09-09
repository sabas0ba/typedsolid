"""上面から基板へアクセスできるトレイ。寸法は説明用で実基板の仕様ではない。"""

import argparse
from pathlib import Path

from typedsolid import Box, Feature, Keepout, Model, Part
from typedsolid.cadquery import export


def board_tray() -> Model:
    features = (
        Feature("floor", Box((0, 0, 0), (60, 40, 2)), "base"),
        Feature("left", Box((0, 0, 0), (2, 40, 20)), "wall"),
        Feature("right", Box((58, 0, 0), (60, 40, 20)), "wall"),
        Feature("front", Box((0, 0, 0), (60, 2, 20)), "wall"),
        Feature("back", Box((0, 38, 0), (60, 40, 20)), "wall"),
        Feature("pad_a", Box((6, 6, 0), (12, 12, 4)), "mount"),
        Feature("pad_b", Box((48, 6, 0), (54, 12, 4)), "mount"),
        Feature("pad_c", Box((6, 28, 0), (12, 34, 4)), "mount"),
        Feature("pad_d", Box((48, 28, 0), (54, 34, 4)), "mount"),
    )
    return Model(
        parts=(Part("board_tray", features),),
        # 支持面への接触を許可するため一様clearanceは0。周囲の壁とは6 mm以上離す。
        keepouts=(Keepout("pcb", Box((8, 8, 4), (52, 32, 7)), 0.0, "plus_z"),),
    )


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, default=Path(".work/board-tray"))
    args = parser.parse_args()
    manifest = export(board_tray(), args.output)
    print(f"Exported board_tray.stl and board_tray.step to {args.output}")
    print("Unevaluated: " + ", ".join(c["rule"] for c in manifest["report"]["checks"] if c["status"] == "not_evaluated"))
