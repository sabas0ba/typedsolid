from dataclasses import replace
import json
from pathlib import Path
import struct
import tempfile
import unittest
from unittest.mock import patch

import cadquery as cq

from examples.board_tray import board_tray
from typedsolid import Box, Feature, Keepout, Model, Part, Policy
from typedsolid.cadquery import build, export


def block() -> Model:
    return Model((Part("block", (Feature("body", Box((0, 0, 0), (10, 10, 10))),)),))


def add_feature(model: Model, feature: Feature) -> Model:
    part = model.parts[0]
    return replace(model, parts=(replace(part, features=(*part.features, feature)), *model.parts[1:]))


def failures(result, rule):
    return [c for c in result.report["checks"] if c["rule"] == rule and c["status"] == "fail"]


class CadQueryTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        Path(".work").mkdir(exist_ok=True)

    def test_board_tray(self):
        result = build(board_tray())
        self.assertTrue(result.export_allowed, result.report)
        self.assertEqual(len(result.shapes["board_tray"].Solids()), 1)
        self.assertEqual({c["rule"] for c in result.report["checks"] if c["status"] == "not_evaluated"}, {"final_wall_thickness", "support_free", "strength", "thermal"})

    def test_disconnected_feature(self):
        result = build(add_feature(block(), Feature("island", Box((20, 0, 0), (25, 5, 5)))))
        self.assertFalse(result.export_allowed)
        self.assertTrue(failures(result, "single_solid"))

    def test_cut_can_disconnect_final_geometry(self):
        result = build(add_feature(block(), Feature("slot", Box((4, -1, -1), (6, 11, 11)), operation="cut")))
        self.assertFalse(result.export_allowed)
        self.assertTrue(failures(result, "single_solid"))

    def test_complete_removal_fails(self):
        result = build(add_feature(block(), Feature("remove", Box((-1, -1, -1), (11, 11, 11)), operation="cut")))
        self.assertFalse(result.export_allowed)
        self.assertTrue(failures(result, "valid_solid"))

    def test_thin_additive_feature(self):
        model = Model((Part("plate", (Feature("thin", Box((0, 0, 0), (10, 10, 0.4))),)),))
        self.assertTrue(failures(build(model), "feature_thickness"))

    def test_post_cut_thickness_remains_unknown(self):
        model = add_feature(block(), Feature("pocket", Box((0.1, 0.1, 0.1), (9.9, 9.9, 11)), operation="cut"))
        policy = replace(model.policy, required=(*model.policy.required, "final_wall_thickness"))
        self.assertFalse(build(replace(model, policy=policy)).export_allowed)

    def test_keepout_collision(self):
        keepout = Keepout("pcb", Box((1, 1, 1), (2, 2, 2)))
        self.assertTrue(failures(build(replace(block(), keepouts=(keepout,))), "keepout_clearance"))

    def test_margin_is_measured_on_final_geometry(self):
        keepout = Keepout("pcb", Box((10.25, 1, 1), (12, 2, 2)), 0.5)
        self.assertTrue(failures(build(replace(block(), keepouts=(keepout,))), "keepout_clearance"))
        self.assertTrue(build(replace(block(), keepouts=(replace(keepout, clearance_mm=0),))).export_allowed)

    def test_access_blocked_even_when_keepout_is_empty(self):
        model = board_tray()
        lid = Part("lid", (Feature("cover", Box((0, 0, 20), (60, 40, 22))),))
        result = build(replace(model, parts=(*model.parts, lid)))
        self.assertFalse(result.export_allowed)
        self.assertFalse(failures(result, "keepout_clearance"))
        self.assertTrue(failures(result, "access_clearance"))

    def test_overlapping_parts(self):
        model = block()
        other = replace(model.parts[0], id="other")
        self.assertTrue(failures(build(replace(model, parts=(*model.parts, other))), "part_interference"))

    def test_touching_parts_allowed(self):
        other = Part("other", (Feature("body", Box((10, 0, 0), (20, 10, 10))),))
        self.assertTrue(build(replace(block(), parts=(*block().parts, other))).export_allowed)

    def test_unsupported_required_rule_blocks_export(self):
        for rule in ("support_free", "strength", "thermal"):
            with self.subTest(rule=rule):
                model = replace(block(), policy=Policy(required=(rule,)))
                self.assertFalse(build(model).export_allowed)

    def test_kernel_exception_is_not_a_pass(self):
        with patch("typedsolid.cadquery._box", side_effect=RuntimeError("kernel failed")):
            result = build(block())
        self.assertFalse(result.export_allowed)
        self.assertEqual(result.shapes, {})

    def test_export_rejects_before_writing(self):
        model = add_feature(block(), Feature("island", Box((20, 0, 0), (25, 5, 5))))
        with tempfile.TemporaryDirectory(dir=".work") as root:
            output = Path(root) / "bad"
            with self.assertRaises(ValueError):
                export(model, output)
            self.assertFalse(output.exists())

    def test_exports_one_file_per_part_and_round_trips_step(self):
        other = Part("other", (Feature("body", Box((20, 0, 0), (30, 10, 10))),))
        model = replace(block(), parts=(*block().parts, other))
        with tempfile.TemporaryDirectory(dir=".work") as root:
            output = Path(root) / "good"
            manifest = export(model, output)
            self.assertEqual(sorted(p.name for p in output.glob("*.stl")), ["block.stl", "other.stl"])
            for name in ("block", "other"):
                shape = cq.importers.importStep(str(output / f"{name}.step")).val()
                self.assertTrue(shape.isValid())
                self.assertEqual(len(shape.Solids()), 1)
                self.assertAlmostEqual(shape.Volume(), 1000, places=5)
                content = (output / f"{name}.stl").read_bytes()
                triangles = struct.unpack_from("<I", content, 80)[0]
                self.assertGreater(triangles, 0)
                self.assertEqual(len(content), 84 + 50 * triangles)
            self.assertEqual(json.loads((output / "report.json").read_text()), manifest)
            before = (output / "block.stl").read_bytes()
            with self.assertRaises(FileExistsError):
                export(model, output)
            self.assertEqual(before, (output / "block.stl").read_bytes())

    def test_public_build_mutation_does_not_bypass_export(self):
        model = replace(block(), policy=Policy(required=("support_free",)))
        result = build(model)
        result.report["required"].clear()
        with tempfile.TemporaryDirectory(dir=".work") as root, self.assertRaises(ValueError):
            export(model, Path(root) / "bad")
