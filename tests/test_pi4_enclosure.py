"""Raspberry Pi 4用の2部品の筐体。正常版が採用した製造案ですべてのruleを通り、欠陥版が宣言した集合だけで落ちる。"""

import unittest

from examples.pi4_enclosure import PI4_CASES, pi4_enclosure
from typedsolid.cadquery import build


class Pi4EnclosureTests(unittest.TestCase):
    def test_each_case_fails_exactly_the_declared_checks(self):
        for factory, adopted, alternatives in PI4_CASES:
            with self.subTest(case=factory.__name__):
                checks = build(factory()).report["checks"]
                failing = [c for c in checks if c["status"] == "fail"]
                self.assertEqual({(c["rule"], c["target"]) for c in failing if c.get("adopted", True)}, adopted)
                self.assertEqual(
                    {(c["rule"], c["target"], c["plan"]) for c in failing if not c.get("adopted", True)}, alternatives,
                )
                # failしたcheckは、寸法だけで決まるsnap fitの積層方向を除き、検出箇所を持つ。
                located = [c for c in failing if not c["target"].endswith("/layer")]
                self.assertTrue(all(c.get("locations") for c in located), located)

    def test_tilted_lid_passes_the_snap_fit_layer_check(self):
        """傾けた蓋の製造案は、梁を積層方向から離し、支持なしで積める。"""
        checks = build(pi4_enclosure()).report["checks"]
        tilted = [c for c in checks if c.get("plan") == "fdm_tilted"]
        self.assertEqual({c["rule"] for c in tilted}, {"final_wall_thickness", "support_free", "snap_fit"})
        self.assertTrue(all(c["status"] == "pass" for c in tilted), tilted)
        layer = next(c for c in tilted if c["target"] == "clip_0/layer")
        self.assertIn("50.0°", layer["message"])

    def test_board_heat_is_solved_on_three_grids(self):
        """底の熱のケースは3つの格子で収束し、上限との比較で合格する。強度のケースは宣言しない。"""
        checks = build(pi4_enclosure()).report["checks"]
        heat = next(c for c in checks if c["target"] == "board_heat/temperature")
        self.assertEqual(heat["status"], "pass", heat["message"])
        self.assertIn("(grid 2 mm)", heat["message"])
        strength = [c for c in checks if c["rule"] == "strength"]
        self.assertEqual([c["status"] for c in strength], ["not_evaluated"])

    def test_normal_case_exercises_every_assembly_rule(self):
        """正常版は部品間の検査をすべて実際に評価する。対象のない合格 (model) で通っていない。"""
        checks = build(pi4_enclosure()).report["checks"]
        for rule in ("fastener_fit", "fastener_release", "connector_fit", "access_clearance", "snap_fit",
                     "keepout_clearance", "part_interference", "disassembly_path", "disassembly_separation"):
            with self.subTest(rule=rule):
                targets = {c["target"] for c in checks if c["rule"] == rule}
                self.assertTrue(targets and targets != {"model"}, targets)


if __name__ == "__main__":
    unittest.main()
