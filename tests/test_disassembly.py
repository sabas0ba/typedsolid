"""分解stepの着脱検査と、掃引体積の構成の検証。"""

from dataclasses import replace
import math
from pathlib import Path
import unittest

import cadquery as cq

from typedsolid import Assembly, Box, Feature, Model, Move, Part, Policy, Step
from typedsolid.cadquery import _laterally_expanded, _swept, build

# 着脱検査だけを見る。voxel評価は判定に関わらないため、格子を粗くして時間を抑える。
POLICY = Policy(
    voxel_mm=0.5,
    required=("valid_solid", "single_solid", "part_interference", "disassembly_path", "disassembly_separation"),
)


def volume(shape: cq.Shape) -> float:
    return sum(abs(solid.Volume()) for solid in shape.Solids())


def tray() -> Part:
    """30×20×10 mm、壁と床2 mmの上面が開いた箱。"""
    return Part("tray", (
        Feature("floor", Box((0, 0, 0), (30, 20, 2))),
        Feature("left", Box((0, 0, 0), (2, 20, 10))),
        Feature("right", Box((28, 0, 0), (30, 20, 10))),
        Feature("front", Box((0, 0, 0), (30, 2, 10))),
        Feature("back", Box((0, 18, 0), (30, 20, 10))),
    ))


def lipped_lid(gap: float) -> Part:
    """箱の上に載る板と、壁の内側へ3 mm差し込む縁。縁と壁の隙間はgap。"""
    inner = 2.0 + gap
    return Part("lid", (
        Feature("panel", Box((0, 0, 10), (30, 20, 12))),
        Feature("lip", Box((inner, inner, 7), (30 - inner, 20 - inner, 10))),
    ))


def model(*parts: Part, steps: tuple[Step, ...] = (), clearance: float = 0.0) -> Model:
    return Model(parts=parts, assembly=Assembly(steps, clearance), policy=POLICY)


def failing(result, rule: str) -> list[dict]:
    return [c for c in result.report["checks"] if c["rule"] == rule and c["status"] == "fail"]


def statuses(result, rule: str) -> set[str]:
    return {c["status"] for c in result.report["checks"] if c["rule"] == rule}


LIFT = Step("open_lid", ("lid",), (Move("plus_z"),))


class SweptVolumeTests(unittest.TestCase):
    """掃引体積を解析値と照合する。face prismの和が掃引体積に一致することの確認。"""

    def setUp(self):
        # 10 mm角、厚さ2 mmの板の中央に、z方向の直径4 mmの穴。
        self.plate = cq.Solid.makeBox(10, 10, 2).cut(cq.Solid.makeCylinder(2, 4, pnt=cq.Vector(5, 5, -1)))
        self.hole = [{"shape": {"kind": "cylinder", "axis": "z", "center": [5, 5], "radius": 2, "span": [-1, 3]}}]

    def test_sweep_along_the_hole_keeps_the_hole(self):
        swept = _swept(self.plate, 2, 5.0, [])
        self.assertAlmostEqual(volume(swept), (100 - 4 * math.pi) * 7, places=6)

    def test_sweep_across_the_hole_fills_it(self):
        # 穴を横切るx方向の掃引では、穴の影は材料で覆われる。
        for distance in (5.0, -5.0):
            with self.subTest(distance=distance):
                swept = _swept(self.plate, 0, distance, [5.0])
                self.assertEqual(len(swept.Solids()), 1)
                self.assertAlmostEqual(volume(swept), 15 * 10 * 2, places=6)

    def test_cylinder_swept_across_its_axis(self):
        rod = cq.Solid.makeCylinder(2, 10, pnt=cq.Vector(0, 0, 0), dir=cq.Vector(0, 1, 0))
        swept = _swept(rod, 0, 5.0, [0.0])
        self.assertAlmostEqual(volume(swept), 40 * math.pi + 200, places=6)

    def test_lateral_expansion_is_a_square_minkowski_sum(self):
        block = cq.Solid.makeBox(10, 10, 2)
        expanded = _laterally_expanded(block, 2, 0.5, [], [0.0, 0.0, 0.0])
        self.assertAlmostEqual(volume(expanded), 11 * 11 * 2, places=6)
        bounds = expanded.BoundingBox()
        self.assertAlmostEqual(bounds.zmin, 0.0, places=9)
        self.assertAlmostEqual(bounds.zmax, 2.0, places=9)

    def test_post_through_a_hole_is_clear_only_when_it_fits(self):
        lifted = _swept(self.plate, 2, 15.0, [])
        for radius, expected_clear in ((1.5, True), (1.999, True), (2.5, False)):
            with self.subTest(radius=radius):
                post = cq.Solid.makeCylinder(radius, 30, pnt=cq.Vector(5, 5, -10))
                self.assertEqual(volume(lifted.intersect(post)) <= 1e-7, expected_clear)


class DisassemblyTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        Path(".work").mkdir(exist_ok=True)

    def test_flat_lid_lifts_off(self):
        lid = Part("lid", (Feature("panel", Box((0, 0, 10), (30, 20, 12))),))
        result = build(model(tray(), lid, steps=(LIFT,)))
        self.assertTrue(result.export_allowed, result.report)
        self.assertEqual(statuses(result, "disassembly_path"), {"pass"})
        self.assertEqual(statuses(result, "disassembly_separation"), {"pass"})

    def test_lip_gap_is_compared_with_the_fit_clearance(self):
        cases = (
            # (縁の隙間, 要求する隙間, 通るか)
            (0.3, 0.0, True),
            (0.3, 0.2, True),
            (0.3, 0.4, False),
            (0.0, 0.0, True),   # 接するだけなら硬い干渉ではない
            (0.0, 0.2, False),
        )
        for gap, clearance, passes in cases:
            with self.subTest(gap=gap, clearance=clearance):
                result = build(model(tray(), lipped_lid(gap), steps=(LIFT,), clearance=clearance))
                self.assertEqual(not failing(result, "disassembly_path"), passes, result.report)

    def test_resting_contact_does_not_count_against_the_clearance(self):
        """移動方向の手前にある載置面は、隙間の要求で落とさない。"""
        lid = Part("lid", (Feature("panel", Box((0, 0, 10), (30, 20, 12))),))
        result = build(model(tray(), lid, steps=(LIFT,), clearance=0.5))
        self.assertFalse(failing(result, "disassembly_path"), result.report)

    def test_step_clearance_overrides_the_assembly_value(self):
        strict = replace(LIFT, fit_clearance_mm=0.4)
        result = build(model(tray(), lipped_lid(0.3), steps=(strict,), clearance=0.0))
        self.assertTrue(failing(result, "disassembly_path"))
        self.assertIn("fit clearance 0.4 mm", failing(result, "disassembly_path")[0]["message"])

    def test_sideways_removal_hits_the_wall(self):
        sideways = Step("open_lid", ("lid",), (Move("plus_x"),))
        result = build(model(tray(), lipped_lid(0.3), steps=(sideways,)))
        blocked = failing(result, "disassembly_path")
        self.assertEqual([c["target"] for c in blocked], ["open_lid/lid/0/tray"])
        self.assertFalse(result.export_allowed)

    def test_hooked_lid_needs_to_slide_before_lifting(self):
        # 右端の柱から張り出す爪が蓋の右端4 mmを上から押さえる。
        base = Part("base", (
            Feature("floor", Box((0, 0, 0), (30, 20, 2))),
            Feature("post", Box((26, 0, 0), (30, 20, 8))),
            Feature("hook", Box((22, 0, 6), (30, 20, 8))),
        ))
        lid = Part("lid", (Feature("panel", Box((0, 0, 2), (26, 20, 4))),))
        cases = {
            "lift only": ((Move("plus_z"),), {"disassembly_path"}),
            "slide then lift": ((Move("minus_x", 4.0), Move("plus_z")), set()),
            # 爪の下から出ずに持ち上げを止めると、経路は通るが外れない。
            "slide too little": ((Move("minus_x", 2.0), Move("plus_z", 1.0)), {"disassembly_separation"}),
        }
        for name, (path, expected) in cases.items():
            with self.subTest(name):
                result = build(model(base, lid, steps=(Step("open_lid", ("lid",), path),)))
                # 爪は印刷時の張り出しでもあり、support_freeは別に落ちる。ここでは着脱だけを比べる。
                failed = {
                    c["rule"] for c in result.report["checks"]
                    if c["status"] == "fail" and c["rule"].startswith("disassembly_")
                }
                self.assertEqual(failed, expected, result.report)

    def test_order_matters_for_a_part_under_the_lid(self):
        board = Part("board", (Feature("pcb", Box((5, 5, 2), (25, 15, 4))),))
        lid = Part("lid", (Feature("panel", Box((0, 0, 10), (30, 20, 12))),))
        lift_board = Step("take_board", ("board",), (Move("plus_z"),))
        wrong = build(model(tray(), board, lid, steps=(lift_board, LIFT)))
        self.assertEqual([c["target"] for c in failing(wrong, "disassembly_path")], ["take_board/board/0/lid"])
        right = build(model(tray(), board, lid, steps=(LIFT, lift_board)))
        self.assertFalse(failing(right, "disassembly_path"), right.report)

    def test_parts_listed_together_move_as_one(self):
        lid = Part("lid", (Feature("panel", Box((0, 0, 10), (30, 20, 12))),))
        knob = Part("knob", (Feature("grip", Box((10, 5, 12), (20, 15, 16))),))
        together = Step("open_lid", ("lid", "knob"), (Move("plus_z"),))
        result = build(model(tray(), lid, knob, steps=(together,)))
        self.assertEqual([c["target"] for c in result.report["checks"] if c["rule"] == "disassembly_path"],
                         ["open_lid/lid+knob/0/tray"])
        self.assertFalse(failing(result, "disassembly_path"), result.report)

    def test_last_part_removed_has_nothing_to_hit(self):
        lid = Part("lid", (Feature("panel", Box((0, 0, 10), (30, 20, 12))),))
        steps = (LIFT, Step("take_tray", ("tray",), (Move("minus_z"),)))
        result = build(model(tray(), lid, steps=steps))
        messages = [c["message"] for c in result.report["checks"] if c["target"] == "take_tray/tray/0"]
        self.assertEqual(len(messages), 1)
        self.assertIn("no remaining parts", messages[0])
        self.assertTrue(result.export_allowed, result.report)

    def test_without_steps_the_rules_have_no_targets(self):
        result = build(model(tray()))
        for rule in ("disassembly_path", "disassembly_separation"):
            with self.subTest(rule=rule):
                checks = [c for c in result.report["checks"] if c["rule"] == rule]
                self.assertEqual([(c["status"], c["message"]) for c in checks], [("pass", "no applicable declared targets")])

    def test_cut_geometry_is_swept_as_cut(self):
        """蓋の穴を柱が通る場合、穴を無視すると誤って干渉と判定する。"""
        base = Part("base", (
            Feature("floor", Box((0, 0, 0), (30, 20, 2))),
            Feature("post", Box((13, 8, 0), (17, 12, 20))),
        ))
        lid = Part("lid", (
            Feature("panel", Box((0, 0, 2), (30, 20, 4))),
            Feature("slot", Box((12, 7, 1), (18, 13, 5)), operation="cut"),
        ))
        result = build(model(base, lid, steps=(LIFT,)))
        self.assertFalse(failing(result, "disassembly_path"), result.report)
        # 穴が柱に対して狭ければ干渉する。
        tight = replace(lid, features=(lid.features[0], Feature("slot", Box((14, 9, 1), (16, 11, 5)), operation="cut")))
        blocked = build(model(base, tight, steps=(LIFT,)))
        self.assertTrue(failing(blocked, "part_interference") or failing(blocked, "disassembly_path"))


if __name__ == "__main__":
    unittest.main()
