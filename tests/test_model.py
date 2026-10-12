from dataclasses import replace
import json
import unittest

from typedsolid import (
    DEFAULT_PLAN, Assembly, Box, Clearance, Cylinder, Feature, Keepout, ManufacturingPlan, Milling, Model, Molding,
    Move, Orientation, Part, Policy, Resin, Step, hole,
)
from typedsolid import _native


def block() -> Model:
    return Model((Part("block", (Feature("body", Box((0, 0, 0), (10, 10, 10))),)),))


class ModelTests(unittest.TestCase):
    def test_parts_without_plans_get_the_default_plan(self):
        data = json.loads(block().to_json())
        part = data["parts"][0]
        self.assertEqual(part["adopted"], DEFAULT_PLAN.id)
        self.assertEqual(part["manufacturing"][0]["process"], {
            "kind": "fdm", "min_wall_mm": 1.2, "overhang_angle_deg": 45.0, "bridge_max_mm": 5.0,
        })
        self.assertNotIn("min_wall_mm", data["policy"])

    def test_several_plans_need_an_adopted_one(self):
        resin = ManufacturingPlan("resin", Resin(0.8, 30.0, 2.0, 3.0), "test", Orientation("plus_x"))
        part = replace(block().parts[0], manufacturing=(DEFAULT_PLAN, resin))
        with self.assertRaisesRegex(ValueError, "adopted is required"):
            replace(block(), parts=(part,)).to_json()
        data = json.loads(replace(block(), parts=(replace(part, adopted="resin"),)).to_json())
        self.assertEqual(data["parts"][0]["adopted"], "resin")
        self.assertEqual(data["parts"][0]["manufacturing"][1]["orientation"], {"up": "plus_x", "turn_deg": 0})
        with self.assertRaisesRegex(ValueError, "adopted plan"):
            replace(block(), parts=(replace(part, adopted="sla"),)).to_json()

    def test_milling_and_molding_plans_are_checked_by_rust(self):
        milling = ManufacturingPlan(
            "milling", Milling(1.0, 3.0, 20.0, (Orientation("minus_z"),)), "test",
        )
        molding = ManufacturingPlan("molding", Molding(1.0, 3.0), "test", Orientation("plus_y"))
        part = replace(block().parts[0], manufacturing=(DEFAULT_PLAN, milling, molding), adopted="fdm")
        data = json.loads(replace(block(), parts=(part,)).to_json())
        plans = data["parts"][0]["manufacturing"]
        self.assertEqual(plans[1]["process"]["additional_setups"], [{"up": "minus_z", "turn_deg": 0}])
        self.assertEqual(plans[2]["process"], {"kind": "molding", "min_wall_mm": 1.0, "max_wall_mm": 3.0})
        # 同じ向きの段取りと、最小肉厚以下の最大肉厚はRust coreが拒否する。
        repeated = replace(milling, process=Milling(1.0, 3.0, 20.0, (Orientation("plus_z"),)))
        inverted = replace(molding, process=Molding(3.0, 1.0))
        for plan in (repeated, inverted):
            with self.subTest(plan=plan.id), self.assertRaises(ValueError):
                replace(block(), parts=(replace(part, manufacturing=(DEFAULT_PLAN, plan)),)).to_json()

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
        self.assertEqual(upgraded["schema_version"], 11)
        # v9までのPolicyの製造の値は、部品ごとのFDMの製造案になる。
        self.assertEqual(upgraded["parts"][0]["adopted"], "v9_policy")
        self.assertEqual(upgraded["parts"][0]["manufacturing"][0]["process"]["kind"], "fdm")
        self.assertEqual(upgraded["assembly"], {"fit_clearance_mm": 0.0, "steps": []})
        self.assertEqual(upgraded["parts"][0]["features"][0]["shape"]["kind"], "box")
        self.assertEqual(upgraded["keepouts"][0]["clearance_mm"], {"default": 0.25})
        self.assertNotIn("access", upgraded["keepouts"][0])
        self.assertEqual(upgraded["sweeps"], [
            {"id": "pcb_plus_z", "keepout": "pcb", "direction": "plus_z", "distance_mm": "exit"},
        ])

    def test_schema_v2_gains_an_empty_assembly(self):
        data = json.loads(block().to_json())
        data["schema_version"] = 2
        del data["assembly"]
        del data["sweeps"]
        for key in ("fasteners", "materials", "snap_fits", "connectors"):
            del data[key]
        for keepout in data["keepouts"]:
            keepout["access"] = []
        # v9までは製造案を持たない。
        for part in data["parts"]:
            del part["manufacturing"], part["adopted"]
        upgraded = json.loads(_native.normalize_model(json.dumps(data)))
        self.assertEqual(upgraded["schema_version"], 11)
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

    def test_positional_arguments_keep_their_meaning(self):
        """assemblyの追加前と同じく、3番目の位置引数はpolicyに入る。"""
        policy = Policy(voxel_mm=0.5)
        model = Model(block().parts, (), policy)
        self.assertIs(model.policy, policy)
        self.assertEqual(model.assembly, Assembly())
        self.assertEqual(json.loads(model.to_json())["policy"]["voxel_mm"], 0.5)

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
        for model in [replace(block(), units="in"), replace(block(), schema_version=12)]:
            with self.assertRaises(ValueError):
                model.to_json()
