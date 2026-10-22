"""強度の解析ケースを持つL字の金具。縦の脚の下端を固定し、腕の先端を下へ押す。

腕の付け根の内角は丸める。鋭い内角は応力の特異点となり、細分するほど応力が増えて評価できない。
部材の厚さ12 mmは、解析の最も粗い格子 (格子間隔1 mmの4倍) でも3 cellあり、丸みの効果が
3つの格子に現れる。金具は横倒しにし、側面を造形板に置いてFDMで造形する。L字の形が層の面に
収まり、腕の下面が張り出さない。材料の値は説明用の値であり、特定の樹脂の値ではない。
"""

from typedsolid import (
    Analyses, Box, Elastic, Fdm, Feature, Force, ManufacturingPlan, Material, Model, Orientation, Part, Policy,
    StructuralCase, inner_fillet,
)

THICKNESS_MM = 12.0
WIDTH_MM = 18.0
HEIGHT_MM = 60.0
REACH_MM = 60.0
RADIUS_MM = 6.0
LOAD_N = 30.0
SIDEWAYS = ManufacturingPlan("fdm_sideways", Fdm(), "typedsolid default FDM values", orientation=Orientation("minus_y"))
MATERIAL = Material(
    "pla_example", "example PLA", "example value for the typedsolid example, not a material specification",
    elastic=Elastic(youngs_modulus_mpa=3500.0, poisson_ratio=0.35),
    allowable_stress_mpa=40.0,
    interlayer_allowable_stress_mpa=15.0,
)


def bracket() -> Model:
    """腕の付け根を丸めたL字の金具。応力とたわみは3つの格子で収束して上限を満たす。層間の
    引張は小さく、格子の間で相対的に大きく変わるため評価しない。"""
    arm_low = HEIGHT_MM - THICKNESS_MM
    part = Part("bracket", (
        Feature("leg", Box((0, 0, 0), (THICKNESS_MM, WIDTH_MM, HEIGHT_MM)), "base"),
        Feature("arm", Box((0, 0, arm_low), (REACH_MM, WIDTH_MM, HEIGHT_MM)), "rib"),
        inner_fillet("corner", "y", (THICKNESS_MM, arm_low), RADIUS_MM, (0, WIDTH_MM), ("plus_x", "minus_z")),
    ), material=MATERIAL.id, manufacturing=(SIDEWAYS,))
    load = StructuralCase(
        "arm_load", "bracket",
        supports=(Box((-1, -1, -0.1), (THICKNESS_MM + 1, WIDTH_MM + 1, 0.1)),),
        forces=(Force(Box((REACH_MM - 0.1, -1, arm_low - 1), (REACH_MM + 0.1, WIDTH_MM + 1, HEIGHT_MM + 1)), (0, 0, -LOAD_N)),),
        safety_factor=2.0, max_deflection_mm=1.0, voxel_mm=1.0,
    )
    return Model((part,), policy=Policy(voxel_mm=0.5), materials=(MATERIAL,), analyses=Analyses(structural=(load,)))
