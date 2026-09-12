from dataclasses import replace
import hashlib
import json
import math
from pathlib import Path
import struct
import tempfile
import unittest
from unittest.mock import patch

import cadquery as cq

from examples.board_tray import board_tray
from typedsolid import Box, Cylinder, Feature, Keepout, Model, Part, Policy
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

    def test_minimum_size_box_is_a_valid_solid(self):
        # 実装上の最小box寸法0.001 mmの立方体は1e-9 mm³であり、
        # 交差判定の許容差1e-7 mm³を空判定に流用すると無効と扱われる。
        model = Model(
            (Part("speck", (Feature("body", Box((0, 0, 0), (0.001, 0.001, 0.001))),)),),
            policy=Policy(min_feature_mm=0.001),
        )
        result = build(model)
        self.assertFalse(failures(result, "valid_solid"), result.report)
        self.assertTrue(result.export_allowed, result.report)

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

    def test_model_sha256_matches_the_saved_file(self):
        with tempfile.TemporaryDirectory(dir=".work") as root:
            output = Path(root) / "hashed"
            manifest = export(block(), output)
            saved = (output / "model.json").read_bytes()
            self.assertEqual(manifest["model_sha256"], hashlib.sha256(saved).hexdigest())
            self.assertEqual(json.loads(saved), json.loads(block().to_json()))

    def test_public_build_mutation_does_not_bypass_export(self):
        model = replace(block(), policy=Policy(required=("support_free",)))
        result = build(model)
        result.report["required"].clear()
        with tempfile.TemporaryDirectory(dir=".work") as root, self.assertRaises(ValueError):
            export(model, Path(root) / "bad")

    def test_cylinder_volume_and_bounds(self):
        model = Model((Part("pin", (Feature("body", Cylinder((3, 4, 5), 2, 6)),)),))
        result = build(model)
        self.assertTrue(result.export_allowed, result.report)
        shape = result.shapes["pin"]
        self.assertAlmostEqual(shape.Volume(), math.pi * 4 * 6, places=6)
        bounds = shape.BoundingBox()
        for actual, expected in zip(
            (bounds.xmin, bounds.xmax, bounds.ymin, bounds.ymax, bounds.zmin, bounds.zmax),
            (1, 5, 2, 6, 5, 11),
        ):
            self.assertAlmostEqual(actual, expected, places=6)

    def test_cylindrical_hole_volume_and_feature_order(self):
        hole = Feature("hole", Cylinder((5, 5, -1), 2, 12), operation="cut")
        model = add_feature(block(), hole)
        expected = 1000 - math.pi * 4 * 10
        for features in (model.parts[0].features, tuple(reversed(model.parts[0].features))):
            with self.subTest(features=features):
                result = build(replace(model, parts=(replace(model.parts[0], features=features),)))
                self.assertTrue(result.export_allowed, result.report)
                self.assertAlmostEqual(result.shapes["block"].Volume(), expected, places=5)

    def test_cylindrical_hole_clears_keepout(self):
        model = add_feature(block(), Feature("hole", Cylinder((5, 5, -1), 2, 12), operation="cut"))
        keepout = Keepout("pin", Box((4, 4, 0), (6, 6, 10)), 0, "plus_z")
        self.assertTrue(build(replace(model, keepouts=(keepout,))).export_allowed)
        larger = replace(keepout, bounds=Box((3, 3, 0), (7, 7, 10)))
        self.assertTrue(failures(build(replace(model, keepouts=(larger,))), "keepout_clearance"))

    def test_cylinder_above_keepout_blocks_access(self):
        cap = Part("cap", (Feature("body", Cylinder((5, 5, 20), 3, 2)),))
        space = Keepout("pin", Box((4, 4, 12), (6, 6, 14)), 0, "plus_z")
        result = build(replace(block(), parts=(*block().parts, cap), keepouts=(space,)))
        self.assertFalse(result.export_allowed)
        self.assertFalse(failures(result, "keepout_clearance"))
        self.assertTrue(failures(result, "access_clearance"))

    def test_disconnected_and_overlapping_cylinders(self):
        pin = Feature("pin", Cylinder((20, 20, 0), 2, 4))
        self.assertTrue(failures(build(add_feature(block(), pin)), "single_solid"))
        other = Part("other", (replace(pin, bounds=Cylinder((5, 5, 0), 2, 4)),))
        self.assertTrue(failures(
            build(replace(block(), parts=(*block().parts, other))), "part_interference",
        ))

    def test_cylindrical_cut_can_remove_everything(self):
        result = build(add_feature(
            block(), Feature("remove", Cylinder((5, 5, -1), 8, 12), operation="cut"),
        ))
        self.assertFalse(result.export_allowed)
        self.assertTrue(failures(result, "valid_solid"))

    def test_cylinder_backend_exception_blocks_export(self):
        model = Model((Part("pin", (Feature("body", Cylinder((0, 0, 0), 2, 3)),)),))
        with patch("typedsolid.cadquery.cq.Solid.makeCylinder", side_effect=RuntimeError("kernel failed")):
            result = build(model)
        self.assertFalse(result.export_allowed)
        self.assertEqual(result.shapes, {})

    def test_mounting_plate_export_and_post_cut_thickness(self):
        from examples.mounting_plate import mounting_plate
        model = mounting_plate()
        expected = 40 * 30 * 2 + 4 * math.pi * 3**2 * 5 - 4 * math.pi * 1.2**2 * 7
        with tempfile.TemporaryDirectory(dir=".work") as root:
            output = Path(root) / "plate"
            manifest = export(model, output)
            shape = cq.importers.importStep(str(output / "mounting_plate.step")).val()
            self.assertTrue(shape.isValid())
            self.assertEqual(len(shape.Solids()), 1)
            self.assertAlmostEqual(shape.Volume(), expected, places=5)
            content = (output / "mounting_plate.stl").read_bytes()
            triangles = struct.unpack_from("<I", content, 80)[0]
            self.assertGreater(triangles, 0)
            self.assertEqual(len(content), 84 + 50 * triangles)
            self.assertEqual(manifest["model_sha256"],
                             hashlib.sha256((output / "model.json").read_bytes()).hexdigest())
        result = build(replace(model, policy=Policy(required=("final_wall_thickness",))))
        self.assertFalse(result.export_allowed)

    def test_minimum_size_cylinder_is_a_valid_solid(self):
        model = Model(
            (Part("pin", (Feature("body", Cylinder((0, 0, 0), 0.0005, 0.001)),)),),
            policy=Policy(min_feature_mm=0.001),
        )
        result = build(model)
        self.assertTrue(result.export_allowed, result.report)
        self.assertAlmostEqual(result.shapes["pin"].Volume() / (math.pi * 0.0005**2 * 0.001),
                               1, places=6)
