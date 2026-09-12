"""貫通穴付き円柱bossを4個持つ取付板。寸法は説明用で実機の仕様ではない。"""

import argparse
from pathlib import Path

from typedsolid import Box, Cylinder, Feature, Model, Part
from typedsolid.cadquery import export


def mounting_plate() -> Model:
    features = [Feature("plate", Box((0, 0, 0), (40, 30, 2)), "base")]
    for name, x, y in (("a", 6, 6), ("b", 34, 6), ("c", 6, 24), ("d", 34, 24)):
        # bossを底板へ1 mm重ねる。穴は底板とbossの両方を貫通する。
        features.append(Feature(f"boss_{name}", Cylinder((x, y, 1), 3, 6), "mount"))
        features.append(Feature(f"hole_{name}", Cylinder((x, y, -1), 1.2, 9), operation="cut"))
    return Model((Part("mounting_plate", tuple(features)),))


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, default=Path(".work/mounting-plate"))
    args = parser.parse_args()
    manifest = export(mounting_plate(), args.output)
    print(f"Exported mounting_plate.stl and mounting_plate.step to {args.output}")
    print("Unevaluated: " + ", ".join(c["rule"] for c in manifest["report"]["checks"]
                                    if c["status"] == "not_evaluated"))
