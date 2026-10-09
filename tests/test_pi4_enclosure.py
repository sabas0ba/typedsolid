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
                # failしたcheckは、いずれも検出箇所を持つ。
                self.assertTrue(all(c.get("locations") for c in failing), failing)

    def test_normal_case_exercises_every_assembly_rule(self):
        """正常版は部品間の検査をすべて実際に評価する。対象のない合格 (model) で通っていない。"""
        checks = build(pi4_enclosure()).report["checks"]
        for rule in ("fastener_fit", "fastener_release", "connector_fit", "access_clearance",
                     "keepout_clearance", "part_interference", "disassembly_path", "disassembly_separation"):
            with self.subTest(rule=rule):
                targets = {c["target"] for c in checks if c["rule"] == rule}
                self.assertTrue(targets and targets != {"model"}, targets)


if __name__ == "__main__":
    unittest.main()
