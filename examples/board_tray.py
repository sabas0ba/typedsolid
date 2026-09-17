"""上面から基板へアクセスできるトレイ。寸法は説明用で実基板の仕様ではない。"""

import argparse
from pathlib import Path

from typedsolid import Box, Clearance, Feature, Keepout, Model, Part, boss, hole
from typedsolid.cadquery import export

# 4隅の支持pad中心。基板確保領域の四隅に合わせる。
PAD_CENTRES = ((9, 9), (51, 9), (9, 31), (51, 31))
PAD_DIAMETER_MM = 6.0
# M3ネジの下穴。実機のネジ・インサート仕様は未確定である。
SCREW_DIAMETER_MM = 2.5


def board_tray() -> Model:
    shell = (
        Feature("floor", Box((0, 0, 0), (60, 40, 2)), "base"),
        Feature("left", Box((0, 0, 0), (2, 40, 20)), "wall"),
        Feature("right", Box((58, 0, 0), (60, 40, 20)), "wall"),
        Feature("front", Box((0, 0, 0), (60, 2, 20)), "wall"),
        Feature("back", Box((0, 38, 0), (60, 40, 20)), "wall"),
    )
    # padは円柱、ネジ穴は底板を貫通するcut。Cutは全Addを結合した後に差し引く。
    pads = tuple(
        boss(f"pad_{name}", "z", centre, PAD_DIAMETER_MM, (0, 4))
        for name, centre in zip("abcd", PAD_CENTRES)
    )
    screws = tuple(
        hole(f"screw_{name}", "z", centre, SCREW_DIAMETER_MM, (-1, 4))
        for name, centre in zip("abcd", PAD_CENTRES)
    )
    return Model(
        parts=(Part("board_tray", shell + pads + screws),),
        # 支持面への接触を許可するため下面のclearanceは0。他の面は0.5を確保する。
        keepouts=(
            Keepout(
                "pcb",
                Box((8, 8, 4), (52, 32, 7)),
                Clearance(default=0.5, minus_z=0.0),
                ("plus_z",),
            ),
        ),
    )


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, default=Path(".work/board-tray"))
    args = parser.parse_args()
    manifest = export(board_tray(), args.output)
    print(f"Exported board_tray.stl and board_tray.step to {args.output}")
    print("Unevaluated: " + ", ".join(c["rule"] for c in manifest["report"]["checks"] if c["status"] == "not_evaluated"))
