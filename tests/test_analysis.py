from dataclasses import replace
import json
import unittest

from typedsolid import (
    Analyses, AtFastener, Box, Convection, Elastic, Feature, FixedTemperature, Force, HeatInput, ManufacturingPlan,
    Material, Model, Molding, Part, Policy, StructuralCase, Surface, ThermalCase, _native,
)
from typedsolid.cadquery import build

# 解析解と比べやすいよう、ポアソン比を0とした仮の材料。
RESIN = Material(
    "resin", "test resin", "test values",
    elastic=Elastic(youngs_modulus_mpa=1000.0, poisson_ratio=0.0),
    allowable_stress_mpa=100.0,
    thermal_conductivity_w_per_mk=1.0,
)
MOLDED = ManufacturingPlan("molded", Molding(min_wall_mm=0.5, max_wall_mm=100.0), "test values")


def end(x: float) -> Box:
    """x = xの端面を選ぶbox。"""
    return Box((x - 0.1, -1.0, -1.0), (x + 0.1, 100.0, 100.0))


# 長さ20 mm、断面6×6 mmの棒を360 Nで引く。σ = 10 MPa、δ = FL/(EA) = 0.2 mm。
PULL = StructuralCase(
    "pull", "bar", supports=(end(0.0),), forces=(Force(end(20.0), (360.0, 0.0, 0.0)),),
    safety_factor=2.0, max_deflection_mm=0.5,
)
# 一端から0.1 Wを入れ、他端を20 °Cに保つ。T(0) = 20 + QL/(kA) = 20 + 0.1·20/(1e-3·36) ≈ 75.6 °C。
HEAT = ThermalCase(
    "heat", "bar", max_temperature_c=100.0,
    heat_inputs=(HeatInput(end(0.0), 0.1),), fixed_temperatures=(FixedTemperature(end(20.0), 20.0),),
)


def bar(analyses: Analyses = Analyses()) -> Model:
    part = Part("bar", (Feature("body", Box((0, 0, 0), (20, 6, 6))),), material="resin", manufacturing=(MOLDED,))
    # 3つの格子 (0.5、1、2 mm) で断面6 mm角を表し、両端から4 mmの外に応力を判定するcellを残す。
    policy = Policy(voxel_mm=0.5, required=(*Policy().required, "strength", "thermal"))
    return Model((part,), policy=policy, materials=(RESIN,), analyses=analyses)


def by_target(result, target: str) -> dict:
    return next(c for c in result.report["checks"] if c["target"] == target)


class AnalysisModelTests(unittest.TestCase):
    def test_references_are_checked_by_rust(self):
        case = replace(PULL, supports=(Surface(), AtFastener("m3")))
        with self.assertRaises(ValueError) as raised:
            bar(Analyses(structural=(case,))).to_json()
        self.assertIn("unknown fastener m3", str(raised.exception))

    def test_cases_round_trip_and_omit_unset_values(self):
        data = json.loads(bar(Analyses(structural=(replace(PULL, max_deflection_mm=None),), thermal=(HEAT,))).to_json())
        structural = data["analyses"]["structural"][0]
        self.assertNotIn("max_deflection_mm", structural)
        self.assertEqual(structural["supports"][0]["kind"], "box")
        self.assertEqual(data["materials"][0]["elastic"], {"youngs_modulus_mpa": 1000.0, "poisson_ratio": 0.0})
        self.assertNotIn("interlayer_allowable_stress_mpa", data["materials"][0])
        self.assertEqual(data["analyses"]["thermal"][0]["fixed_temperatures"][0]["celsius"], 20.0)

    def test_models_without_cases_omit_analyses(self):
        self.assertNotIn("analyses", json.loads(bar().to_json()))

    def test_missing_material_constants_are_rejected(self):
        model = replace(bar(Analyses(thermal=(HEAT,))), materials=(replace(RESIN, thermal_conductivity_w_per_mk=None),))
        with self.assertRaises(ValueError) as raised:
            model.to_json()
        self.assertIn("thermal_conductivity_w_per_mk", str(raised.exception))

    def test_schema_12_cannot_declare_analyses(self):
        data = json.loads(bar(Analyses(thermal=(HEAT,))).to_json())
        data["schema_version"] = 12
        with self.assertRaises(ValueError):
            _native.normalize_model(json.dumps(data))


class AnalysisBuildTests(unittest.TestCase):
    def test_required_rules_without_cases_block_export(self):
        result = build(bar())
        self.assertFalse(result.export_allowed)
        for rule in ("strength", "thermal"):
            check = next(c for c in result.report["checks"] if c["rule"] == rule)
            self.assertEqual(check["status"], "not_evaluated")
            self.assertIn("no", check["message"])

    def test_declared_cases_are_solved(self):
        result = build(bar(Analyses(structural=(PULL,), thermal=(HEAT,))))
        for target, value in (
            ("pull/stress", "von Mises stress 10.0000 MPa"), ("pull/deflection", "deflection 0.2000 mm"),
            ("heat/temperature", "maximum temperature 75.5556 °C"),
        ):
            check = by_target(result, target)
            self.assertEqual(check["status"], "pass", check["message"])
            self.assertIn(value, check["message"])

    def test_stress_near_supports_and_loads_blocks_required_strength(self):
        """拘束と荷重の近傍の応力は判定せず未評価で残すため、strengthを必須にすると出力できない。"""
        result = build(bar(Analyses(structural=(PULL,), thermal=(HEAT,))))
        boundary = by_target(result, "pull/boundary_stress")
        self.assertEqual(boundary["status"], "not_evaluated")
        self.assertIn("within 4 mm", boundary["message"])
        self.assertFalse(result.export_allowed)
        thermal_only = replace(bar(Analyses(structural=(PULL,), thermal=(HEAT,))), policy=Policy(
            voxel_mm=0.5, required=(*Policy().required, "thermal"),
        ))
        self.assertTrue(build(thermal_only).export_allowed)

    def test_case_grid_overrides_the_policy(self):
        coarse = replace(HEAT, voxel_mm=1.0)
        message = by_target(build(bar(Analyses(thermal=(coarse,)))), "heat/temperature")["message"]
        self.assertIn("(grid 1 mm)", message)
        self.assertIn("(grid 4 mm)", message)

    def test_exceeding_a_limit_fails_with_locations(self):
        hot = replace(HEAT, max_temperature_c=50.0, convection=(Convection(Surface(), 5.0, 20.0),))
        result = build(bar(Analyses(structural=(replace(PULL, safety_factor=20.0),), thermal=(hot,))))
        self.assertFalse(result.export_allowed)
        for target in ("pull/stress", "heat/temperature"):
            check = by_target(result, target)
            self.assertEqual(check["status"], "fail", check["message"])
            self.assertTrue(check["locations"])


if __name__ == "__main__":
    unittest.main()
