from dataclasses import replace
import json
import unittest

from typedsolid import Box, Cylinder, Feature, Keepout, Model, Part, Policy
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
                replace(block(), keepouts=(Keepout("pcb", Box((0, 0, 0), (1, 1, 1)), value),)).to_json()

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
        for model in [replace(block(), units="in"), replace(block(), schema_version=2)]:
            with self.assertRaises(ValueError):
                model.to_json()

    def test_box_json_remains_compatible(self):
        bounds = json.loads(block().to_json())["parts"][0]["features"][0]["bounds"]
        self.assertEqual(bounds, {"min": [0, 0, 0], "max": [10, 10, 10]})

    def test_cylinder_round_trip_and_diameter_policy(self):
        model = Model((Part("pin", (Feature("body", Cylinder((1, 2, 3), 0.6, 2)),)),))
        data = json.loads(model.to_json())
        self.assertEqual(data["parts"][0]["features"][0]["bounds"],
                         {"center": [1, 2, 3], "radius_mm": 0.6, "height_mm": 2})
        self.assertEqual(model.to_json(), _native.normalize_model(model.to_json()))
        self.assertEqual(model.preflight()["checks"][0]["status"], "pass")

    def test_invalid_cylinder_dimensions_and_extent(self):
        for cylinder in (
            Cylinder((0, 0, 0), -1, 2),
            Cylinder((0, 0, 0), 0.0004, 2),
            Cylinder((0, 0, 0), 2, 0.0009),
            Cylinder((1e6, 0, 0), 2, 2),
            Cylinder((0, 0, 999999), 2, 2),
            Cylinder((0, 0, 0), float("nan"), 2),
            Cylinder((0, 0, 0), 2, float("inf")),
        ):
            with self.subTest(cylinder=cylinder), self.assertRaises(ValueError):
                Model((Part("pin", (Feature("body", cylinder),)),)).to_json()

    def test_ambiguous_geometry_and_cylinder_keepout_are_rejected(self):
        data = json.loads(block().to_json())
        data["parts"][0]["features"][0]["bounds"].update(
            center=[0, 0, 0], radius_mm=1, height_mm=2,
        )
        with self.assertRaises(ValueError):
            _native.normalize_model(json.dumps(data))
        with self.assertRaises(ValueError):
            replace(block(), keepouts=(Keepout("space", Cylinder((0, 0, 0), 1, 2)),)).to_json()

    def test_cylindrical_cut_is_excluded_from_primitive_thickness(self):
        part = block().parts[0]
        hole = Feature("hole", Cylinder((5, 5, -1), 0.1, 12), operation="cut")
        report = replace(block(), parts=(replace(part, features=(*part.features, hole)),)).preflight()
        targets = [c["target"] for c in report["checks"] if c["rule"] == "feature_thickness"]
        self.assertEqual(targets, ["block/body"])
