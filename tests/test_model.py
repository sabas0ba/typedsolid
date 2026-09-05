from dataclasses import replace
import json
import unittest

from typedsolid import Box, Feature, Keepout, Model, Part, Policy
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
