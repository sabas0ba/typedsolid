from dataclasses import replace
import json
import unittest

from typedsolid import Assembly, Box, Clearance, Cylinder, Feature, Keepout, Model, Move, Part, Policy, Step, hole
from typedsolid import _native


def block() -> Model:
    return Model((Part("block", (Feature("body", Box((0, 0, 0), (10, 10, 10))),)),))


class ModelTests(unittest.TestCase):
    def test_normalized_model_is_stable(self):
        model = block()
        self.assertEqual(model.to_json(), _native.normalize_model(model.to_json()))

    def test_non_finite_coordinates(self):
        for value in [float("nan"), float("inf"), -float("inf")]:
            with self.subTest(value=value), self.assertRaises(ValueError):
                Model((Part("bad", (Feature("box", Box((0, 0, 0), (1, 1, value))),)),)).to_json()

    def test_duplicate_features(self):
        part = block().parts[0]
        with self.assertRaises(ValueError):
            Model((replace(part, features=part.features * 2),)).to_json()

    def test_invalid_clearance(self):
        for value in [-1, float("nan"), float("inf")]:
            with self.subTest(value=value), self.assertRaises(ValueError):
                keepout = Keepout("pcb", Box((0, 0, 0), (1, 1, 1)), Clearance(default=value))
                replace(block(), keepouts=(keepout,)).to_json()
            with self.subTest(face=value), self.assertRaises(ValueError):
                keepout = Keepout("pcb", Box((0, 0, 0), (1, 1, 1)), Clearance(minus_z=value))
                replace(block(), keepouts=(keepout,)).to_json()

    def test_unset_faces_are_omitted(self):
        keepout = Keepout("pcb", Box((0, 0, 0), (1, 1, 1)), Clearance(default=0.5, minus_z=0.0))
        data = json.loads(replace(block(), keepouts=(keepout,)).to_json())
        self.assertEqual(data["keepouts"][0]["clearance_mm"], {"default": 0.5, "minus_z": 0.0})

    def test_cylinder_round_trips(self):
        part = Part("post", (Feature("stem", Cylinder("z", (0, 0), 2.0, (0, 10))),))
        data = json.loads(Model((part,)).to_json())
        self.assertEqual(
            data["parts"][0]["features"][0]["shape"],
            {"kind": "cylinder", "axis": "z", "center": [0, 0], "radius": 2.0, "span": [0, 10]},
        )

    def test_hole_is_a_cut_cylinder(self):
        feature = hole("screw", "z", (5, 5), 3.0, (-1, 5))
        self.assertEqual(feature.operation, "cut")
        self.assertEqual(feature.shape.radius, 1.5)

    def test_schema_v1_is_accepted(self):
        v1 = {
            "schema_version": 1, "units": "mm",
            "parts": [{"id": "block", "features": [
                {"id": "body", "role": "generic", "operation": "add",
                 "bounds": {"min": [0, 0, 0], "max": [10, 10, 10]}}]}],
            "keepouts": [{"id": "pcb", "bounds": {"min": [1, 1, 1], "max": [2, 2, 2]},
                          "clearance_mm": 0.25, "access": "plus_z"}],
            "policy": {"min_feature_mm": 1.2, "required": ["single_solid"]},
        }
        upgraded = json.loads(_native.normalize_model(json.dumps(v1)))
        self.assertEqual(upgraded["schema_version"], 3)
        self.assertEqual(upgraded["assembly"], {"fit_clearance_mm": 0.0, "steps": []})
        self.assertEqual(upgraded["parts"][0]["features"][0]["shape"]["kind"], "box")
        self.assertEqual(upgraded["keepouts"][0]["clearance_mm"], {"default": 0.25})
        self.assertEqual(upgraded["keepouts"][0]["access"], ["plus_z"])

    def test_schema_v2_gains_an_empty_assembly(self):
        data = json.loads(block().to_json())
        data["schema_version"] = 2
        del data["assembly"]
        upgraded = json.loads(_native.normalize_model(json.dumps(data)))
        self.assertEqual(upgraded["schema_version"], 3)
        self.assertEqual(upgraded["assembly"], {"fit_clearance_mm": 0.0, "steps": []})

    def test_assembly_serializes_exit_and_omits_unset_clearance(self):
        lid = Part("lid", (Feature("panel", Box((0, 0, 10), (10, 10, 12))),))
        steps = (
            Step("open_lid", ("lid",), (Move("plus_x", 2.0), Move("plus_z"))),
            Step("lift_base", ("block",), (Move("plus_z", 5.0),), fit_clearance_mm=0.3),
        )
        model = replace(block(), parts=(*block().parts, lid), assembly=Assembly(steps, 0.2))
        data = json.loads(model.to_json())["assembly"]
        self.assertEqual(data["fit_clearance_mm"], 0.2)
        self.assertEqual(data["steps"][0]["path"], [
            {"direction": "plus_x", "distance_mm": 2.0},
            {"direction": "plus_z", "distance_mm": "exit"},
        ])
        self.assertNotIn("fit_clearance_mm", data["steps"][0])
        self.assertEqual(data["steps"][1]["fit_clearance_mm"], 0.3)

    def test_invalid_assembly_is_rejected(self):
        lid = Part("lid", (Feature("panel", Box((0, 0, 10), (10, 10, 12))),))
        two = replace(block(), parts=(*block().parts, lid))
        cases = {
            "unknown part": Assembly((Step("a", ("ghost",), (Move("plus_z"),)),)),
            "removed twice": Assembly((Step("a", ("lid",), (Move("plus_z"),)), Step("b", ("lid",), (Move("plus_z"),)))),
            "exit not last": Assembly((Step("a", ("lid",), (Move("plus_z"), Move("plus_x", 1.0))),)),
            "negative clearance": Assembly((Step("a", ("lid",), (Move("plus_z"),)),), -0.1),
            "zero distance": Assembly((Step("a", ("lid",), (Move("plus_z", 0.0),)),)),
        }
        for name, assembly in cases.items():
            with self.subTest(name), self.assertRaises(ValueError):
                replace(two, assembly=assembly).to_json()

    def test_preflight_does_not_approve_geometry(self):
        self.assertFalse(_native.export_allowed(json.dumps(block().preflight())))

    def test_required_unknown_rule_rejected(self):
        with self.assertRaises(ValueError):
            replace(block(), policy=Policy(required=("typo",))).to_json()

    def test_backend_independent_import(self):
        import subprocess
        import sys
        result = subprocess.run([sys.executable, "-c", "import sys; import typedsolid; assert 'cadquery' not in sys.modules"], check=False)
        self.assertEqual(result.returncode, 0)

    def test_unknown_fields_rejected(self):
        data = json.loads(block().to_json())
        data["unit"] = "inches"
        with self.assertRaises(ValueError):
            _native.normalize_model(json.dumps(data))

    def test_wrong_units_and_schema_rejected(self):
        for model in [replace(block(), units="in"), replace(block(), schema_version=4)]:
            with self.assertRaises(ValueError):
                model.to_json()
