"""説明用基板スペース、穴付きboss、矩形開口を持つ開放筐体。実基板の仕様ではない。"""

import argparse
from pathlib import Path

from typedsolid import Box, Cylinder, Feature, Keepout, Model, Part
from typedsolid.cadquery import export


def electronics_enclosure() -> Model:
    # 全Addから全Cutを引くため、floorとwallを個別にAddして内部空間を空ける。
    features = [
        Feature("floor", Box((0, 0, 0), (80, 55, 2)), "base"),
        Feature("left", Box((0, 0, 0), (2, 55, 24)), "wall"),
        Feature("right", Box((78, 0, 0), (80, 55, 24)), "wall"),
        Feature("front", Box((0, 0, 0), (80, 2, 24)), "wall"),
        Feature("back", Box((0, 53, 0), (80, 55, 24)), "wall"),
        Feature("connector", Box((30, -1, 7), (50, 3, 15)), operation="cut"),
    ]
    for name, x, y in (("a", 10, 10), ("b", 70, 10), ("c", 10, 45), ("d", 70, 45)):
        features.append(Feature(f"boss_{name}", Cylinder((x, y, 1), 4, 5), "mount"))
        features.append(Feature(f"hole_{name}", Cylinder((x, y, -1), 1.5, 8), operation="cut"))
    for index, x in enumerate((22, 30, 38, 46, 54)):
        features.append(Feature(f"vent_{index}", Box((x, 52, 10), (x + 3, 56, 20)), operation="cut"))
    return Model(
        (Part("electronics_enclosure", tuple(features)),),
        keepouts=(Keepout("pcb", Box((10, 10, 6), (70, 45, 9)), 0, "plus_z"),),
    )


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, default=Path(".work/electronics-enclosure"))
    args = parser.parse_args()
    report = export(electronics_enclosure(), args.output)
    print(f"Exported electronics_enclosure.stl and .step to {args.output}")
    print("Unevaluated: " + ", ".join(c["rule"] for c in report["report"]["checks"]
                                    if c["status"] == "not_evaluated"))
