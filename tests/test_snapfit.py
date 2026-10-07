"""snap fitのhelper、IR、ひずみと積層方向、backendの形状検査。"""

from dataclasses import replace
import json
from pathlib import Path
import unittest

from typedsolid import (
    Assembly, Box, Clearance, Feature, Keepout, Material, Model, Move, Part, Policy, Step, snap_fit,
)
from typedsolid.cadquery import build

# snap fitと分解だけを見る。梁を積層面に沿わせるため、印刷方向は梁の長さ (z) と直交させる。
POLICY = Policy(
    voxel_mm=0.5, build_direction="plus_y",
    required=("valid_solid", "single_solid", "part_interference", "disassembly_path", "disassembly_separation", "snap_fit"),
)
# testのための値であり、特定の材料の値ではない。
PLA = Material("test_pla", "test PLA", "test fixture", allowable_strain=0.02)
OPEN_LID = Step("open_lid", ("lid",), (Move("plus_z"),))
DROP_BASE = Step("drop_base", ("base",), (Move("minus_z"),))


def clip(**overrides):
    """蓋の下面 (z=30) から垂れる長さ20 mm、厚み1.5 mmの梁。先端のフックは+Xへ1 mm張り出し、
    右壁の内側の爪 (z=12〜13) の下に掛かる。外すときは-Xへたわむ。"""
    arguments = dict(
        part="lid", mate="base", step="open_lid", root=(35.25, 10.0, 30.0),
        length_direction="minus_z", deflection="minus_x", length_mm=20.0, thickness_mm=1.5,
        width_mm=10.0, hook_mm=1.0, hook_length_mm=2.0,
    )
    return snap_fit("clip", **{**arguments, **overrides})


def model(fixing=None, *, ledge: bool = True, base_extra=(), lid_extra=(), steps=(OPEN_LID,), policy=POLICY) -> Model:
    fixing = fixing or clip()
    base = Part("base", (
        Feature("floor", Box((0, 0, 0), (40, 20, 2))),
        Feature("wall", Box((38, 0, 0), (40, 20, 30))),
    ) + ((Feature("ledge", Box((36, 5, 12), (38, 15, 13))),) if ledge else ()) + base_extra)
    lid = Part("lid", (Feature("panel", Box((0, 0, 30), (40, 20, 32))),) + fixing.features + lid_extra, material="test_pla")
    return Model(
        parts=(base, lid), policy=policy, assembly=Assembly(steps),
        materials=(PLA,), snap_fits=(fixing.snap,),
    )


def snap_checks(result) -> dict[str, dict]:
    return {c["target"].split("/")[1]: c for c in result.report["checks"] if c["rule"] == "snap_fit"}


def box(low, high) -> list[dict]:
    return [{"min": list(low), "max": list(high)}]


def failing(result, rule: str = "snap_fit") -> set[str]:
    return {c["target"] for c in result.report["checks"] if c["rule"] == rule and c["status"] != "pass"}


class HelperTests(unittest.TestCase):
    def test_beam_and_hook_boxes(self):
        beam, hook = clip().features
        self.assertEqual((beam.shape.min, beam.shape.max), ((34.5, 5.0, 10.0), (36.0, 15.0, 30.0)))
        self.assertEqual((hook.shape.min, hook.shape.max), ((36.0, 5.0, 10.0), (37.0, 15.0, 12.0)))
        self.assertEqual(clip().snap.deflection_mm, 1.0)
        self.assertEqual(clip(deflection_mm=1.2).snap.deflection_mm, 1.2)

    def test_hook_sits_opposite_the_deflection(self):
        _, hook = clip(deflection="plus_x", root=(10.0, 10.0, 30.0)).features
        # 梁はx=9.25〜10.75。+Xへたわんで外れるため、フックは-X側に付く。
        self.assertEqual((hook.shape.min[0], hook.shape.max[0]), (8.25, 9.25))

    def test_invalid_arguments_are_rejected(self):
        for overrides in (dict(deflection="plus_z"), dict(hook_length_mm=0.0), dict(hook_length_mm=20.0)):
            with self.subTest(overrides), self.assertRaises(ValueError):
                clip(**overrides)

    def test_materials_and_part_material_serialize(self):
        data = json.loads(model().to_json())
        self.assertEqual(data["materials"], [
            {"id": "test_pla", "name": "test PLA", "source": "test fixture", "allowable_strain": 0.02},
        ])
        self.assertEqual(data["parts"][1]["material"], "test_pla")
        self.assertNotIn("material", data["parts"][0])
        self.assertEqual(data["snap_fits"][0]["beam"], "clip_beam")

    def test_ir_rejects_a_part_without_strain(self):
        cases = {
            "no material": lambda m: replace(m, parts=(m.parts[0], replace(m.parts[1], material=None))),
            "no allowable strain": lambda m: replace(m, materials=(replace(PLA, allowable_strain=None),)),
            "step moves the mate too": lambda m: replace(m, assembly=Assembly((Step("open_lid", ("lid", "base"), (Move("plus_z"),)),))),
        }
        for name, edit in cases.items():
            with self.subTest(name), self.assertRaises(ValueError):
                edit(model()).to_json()


class SnapFitTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        Path(".work").mkdir(exist_ok=True)

    def test_clip_passes_and_releases(self):
        result = build(model())
        checks = snap_checks(result)
        self.assertEqual(set(checks), {"strain", "layer", "beam", "deflection_space", "retention"})
        self.assertEqual(failing(result), set())
        # L=30-12=18 mm、t=1.5 mm、y=1 mm: ε = 1.5·1.5·1/18²。
        self.assertIn("0.006944", checks["strain"]["message"])
        self.assertEqual(failing(result, "disassembly_path"), set())

    def test_beam_along_the_build_direction_fails_the_layer_check(self):
        result = build(model(policy=replace(POLICY, build_direction="plus_z")))
        self.assertEqual(failing(result), {"clip/layer"})

    def test_excess_deflection_fails_the_strain(self):
        result = build(model(clip(deflection_mm=3.0)))
        self.assertEqual(failing(result), {"clip/strain"})

    def test_without_the_ledge_nothing_is_retained(self):
        result = build(model(ledge=False))
        self.assertEqual(failing(result), {"clip/retention"})
        # 保持しないフックそのものを検出箇所とする。
        self.assertEqual(snap_checks(result)["retention"]["locations"], box((36, 5, 10), (37, 15, 12)))

    def test_obstacle_beside_the_beam_blocks_the_deflection(self):
        post = Feature("post", Box((33, 5, 15), (34, 15, 20)))
        result = build(model(base_extra=(post,)))
        self.assertEqual(failing(result), {"clip/deflection_space"})
        self.assertIn("hits base", snap_checks(result)["deflection_space"]["message"])
        # 梁の包絡 (x≥33.5) と柱の共通部分。
        self.assertEqual(snap_checks(result)["deflection_space"]["locations"], box((33.5, 5, 15), (34, 15, 20)))

    def test_keepout_beside_the_beam_blocks_the_deflection(self):
        # 柱と同じ位置の基板領域。取付先がsnap fitを持つ蓋でも、相手の筐体でも、梁はたわめない。
        for attached_to in ("base", "lid", None):
            with self.subTest(attached_to=attached_to):
                board = Keepout("pcb", Box((33, 5, 15), (34, 15, 20)), Clearance(default=0.5), attached_to=attached_to)
                result = build(replace(model(), keepouts=(board,)))
                self.assertIn("clip/deflection_space", failing(result))
                self.assertIn("keepout:pcb", snap_checks(result)["deflection_space"]["message"])

    def test_too_little_deflection_leaves_the_hook_caught(self):
        result = build(model(clip(deflection_mm=0.5)))
        self.assertEqual(failing(result), set())
        self.assertEqual(failing(result, "disassembly_path"), {"open_lid/lid/0/base"})

    def test_moving_the_mate_instead_of_the_part(self):
        result = build(model(clip(step="drop_base"), steps=(DROP_BASE,)))
        self.assertEqual(failing(result), set())
        self.assertEqual(failing(result, "disassembly_path"), set())
        self.assertIn("against base along step drop_base", snap_checks(result)["retention"]["message"])

    def test_bent_beam_is_swept_along_the_path(self):
        # 蓋を-Xへ3 mm滑らせてから上へ抜く。柱は、たわんだ梁の包絡 (x=30.5〜) だけが掃引で通る位置にある。
        # 初期位置でのたわむ空間には入らず、たわまない梁と動かしたフックの掃引にも掛からない。
        post = Feature("post", Box((30.6, 5, 12), (31.4, 15, 20)))
        slide = Step("open_lid", ("lid",), (Move("minus_x", 3.0), Move("plus_z")))
        result = build(model(base_extra=(post,), steps=(slide,)))
        self.assertNotIn("clip/deflection_space", failing(result))
        self.assertIn("open_lid/lid/0/base", failing(result, "disassembly_path"))

    def test_two_clips_on_one_part_release_together(self):
        # 左壁にも爪を設け、-Xへ張り出すフックを+Xへたわませて外す。
        left = clip(root=(4.75, 10.0, 30.0), deflection="plus_x")
        right = clip()
        base_extra = (
            Feature("left_wall", Box((0, 0, 0), (2, 20, 30))),
            Feature("left_ledge", Box((2, 5, 12), (4, 15, 13))),
        )
        two = model(right, base_extra=base_extra)
        lid = replace(two.parts[1], features=two.parts[1].features + tuple(
            replace(f, id=f.id.replace("clip", "left")) for f in left.features
        ))
        snap = replace(left.snap, id="left", beam="left_beam", hook="left_hook")
        result = build(replace(two, parts=(two.parts[0], lid), snap_fits=(right.snap, snap)))
        self.assertEqual(failing(result), set())
        self.assertEqual(failing(result, "disassembly_path"), set())

    def test_cut_through_the_beam_is_reported(self):
        slot = Feature("slot", Box((30, 0, 20), (40, 20, 21)), operation="cut")
        result = build(model(lid_extra=(slot,)))
        self.assertIn("clip/beam", failing(result))
        self.assertEqual(snap_checks(result)["beam"]["locations"], box((34.5, 5, 20), (36, 15, 21)))


if __name__ == "__main__":
    unittest.main()
