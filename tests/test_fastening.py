"""ネジ固定のhelper、IR、backendのfastener_fit検査。"""

from dataclasses import replace
import json
from pathlib import Path
import unittest

from typedsolid import (
    Assembly, Box, Feature, InsertSpec, Material, Model, Part, Policy, Printer, Profile, Release, ScrewSpec,
    Step, Move, hole, screw_fixing,
)
from typedsolid import _native
from typedsolid.cadquery import build

# ネジ固定と掃引だけを見る。voxel評価は判定に関わらないため、格子を粗くして時間を抑える。
POLICY = Policy(
    voxel_mm=0.5,
    required=("valid_solid", "single_solid", "part_interference", "access_clearance", "fastener_fit"),
)

# testのための寸法であり、特定の製品の値ではない。
M2 = ScrewSpec(
    "m2x8", "test fixture", length_mm=8.0, major_mm=2.0, head_mm=3.8, through_mm=2.4, driver_mm=3.0,
    pilot_mm=1.6,
)
INSERT = InsertSpec("m2_insert", "test fixture", hole_mm=3.2, length_mm=4.0)


def fixing(screw: ScrewSpec = M2, *, boss_mm: float = 6.0, insert: InsertSpec | None = None, **overrides):
    """30×20 mmの床から立つbossへ、z=10〜12の蓋を上から締める。先端はz=4。"""
    arguments = dict(
        base="tray", clamp=("lid",), direction="minus_z", center=(15.0, 10.0),
        seat_mm=12.0, joint_mm=10.0, min_engagement_mm=4.0 if insert is None else 3.5,
        min_boss_wall_mm=1.5, tip_clearance_mm=1.0, boss_diameter_mm=boss_mm, boss_from_mm=0.0,
        insert=insert,
    )
    return screw_fixing("corner", screw, **{**arguments, **overrides})


def model(fix, *, base_features=None, clamp_features=None, fasteners=None) -> Model:
    tray = Part("tray", (Feature("floor", Box((0, 0, 0), (30, 20, 2))),) + (
        fix.base_features if base_features is None else base_features
    ))
    lid = Part("lid", (Feature("panel", Box((0, 0, 10), (30, 20, 12))),) + (
        fix.clamp_features if clamp_features is None else clamp_features
    ))
    return Model(
        parts=(tray, lid), policy=POLICY, sweeps=(fix.sweep,),
        fasteners=(fix.fastener,) if fasteners is None else fasteners,
    )


def fastener_checks(result) -> list[dict]:
    return [c for c in result.report["checks"] if c["rule"] == "fastener_fit"]


def locations(result, aspect: str) -> list[dict]:
    return next(c for c in fastener_checks(result) if c["target"] == f"corner/{aspect}")["locations"]


def box(low, high) -> list[dict]:
    return [{"min": list(low), "max": list(high)}]


def failing_aspects(result) -> set[str]:
    return {c["target"].split("/")[1] for c in fastener_checks(result) if c["status"] != "pass"}


class HelperTests(unittest.TestCase):
    def test_self_tapping_features_and_ir(self):
        fix = fixing()
        boss, bore = fix.base_features
        self.assertEqual((boss.id, boss.operation, boss.shape.radius, boss.shape.span), ("corner_boss", "add", 3.0, (0.0, 10.0)))
        # 境目の外0.5 mmから、先端 (z=4) の1 mm先まで。
        self.assertEqual((bore.id, bore.operation, bore.shape.radius, bore.shape.span), ("corner_bore", "cut", 0.8, (3.0, 10.5)))
        (through,) = fix.clamp_features
        self.assertEqual((through.shape.radius, through.shape.span), (1.2, (9.5, 12.5)))
        self.assertEqual(fix.sweep.direction, "plus_z")
        self.assertEqual((fix.sweep.shape.radius, fix.sweep.shape.span), (1.9, (12.0, 12.1)))
        data = json.loads(model(fix).to_json())["fasteners"][0]
        self.assertEqual(data["anchor"], {"kind": "self_tapping", "pilot_mm": 1.6})
        self.assertEqual(data["screw"], {"length_mm": 8.0, "major_mm": 2.0, "head_mm": 3.8})
        self.assertEqual(data["clamp"], ["lid"])

    def test_insert_hole_is_deep_enough_for_the_screw(self):
        fix = fixing(insert=INSERT, boss_mm=7.0)
        bore = fix.base_features[1]
        self.assertEqual((bore.shape.radius, bore.shape.span), (1.6, (3.0, 10.5)))
        data = json.loads(model(fix).to_json())["fasteners"][0]
        self.assertEqual(data["anchor"], {"kind": "insert", "hole_mm": 3.2, "length_mm": 4.0})

    def test_invalid_arguments_are_rejected(self):
        cases = {
            "negative tip clearance": dict(tip_clearance_mm=-0.1),
            "boss without origin": dict(boss_from_mm=None),
            "boss on the wrong side": dict(boss_from_mm=11.0),
            "screw stops before the joint": dict(seat_mm=20.0, tip_clearance_mm=0.0),
        }
        for name, overrides in cases.items():
            with self.subTest(name), self.assertRaises(ValueError):
                fixing(**overrides)
        with self.assertRaises(ValueError):
            fixing(replace(M2, pilot_mm=None))

    def test_ir_rejects_inconsistent_dimensions(self):
        fix = fixing()
        cases = {
            "pilot above major": replace(fix.fastener, anchor=replace(fix.fastener.anchor, pilot_mm=2.2)),
            "through above head": replace(fix.fastener, through_mm=4.0),
            "unknown base": replace(fix.fastener, base="ghost"),
            "clamp is the base": replace(fix.fastener, clamp=("tray",)),
        }
        for name, fastener in cases.items():
            with self.subTest(name), self.assertRaises(ValueError):
                model(fix, fasteners=(fastener,)).to_json()


OPEN_LID = Step("open_lid", ("lid",), (Move("plus_z"),))


def release_status(fix) -> tuple[str, str]:
    """蓋を上へ外す手順を持つモデルで、fastener_releaseの判定と理由を返す。"""
    data = replace(model(fix), assembly=Assembly((OPEN_LID,))).to_json()
    (check,) = json.loads(_native.evaluate_fastener_releases(data))
    return check["status"], check["message"]


class ReleaseTests(unittest.TestCase):
    def test_release_and_clamp_keepouts_serialize(self):
        fix = fixing(release=Release("open_lid"))
        data = json.loads(replace(model(fix), assembly=Assembly((OPEN_LID,))).to_json())
        self.assertEqual(data["fasteners"][0]["release"], {"after_step": "open_lid"})
        self.assertNotIn("clamp_keepouts", data["fasteners"][0])
        # ドライバはネジを外す状態で抜き差しする。
        self.assertEqual(fix.sweep.after_step, "open_lid")
        self.assertEqual(fixing(release=Release()).fastener.release, Release())

    def test_screw_must_come_out_before_the_lid(self):
        self.assertEqual(release_status(fixing(release=Release()))[0], "pass")
        for release in (None, Release("open_lid")):
            with self.subTest(release=release):
                status, message = release_status(fixing(release=release))
                self.assertEqual(status, "fail")
                self.assertIn("step open_lid moves lid away from tray", message)

    def test_release_rule_is_reported_by_build(self):
        Path(".work").mkdir(exist_ok=True)
        base = replace(model(fixing()), assembly=Assembly((OPEN_LID,)))
        checks = [c for c in build(base).report["checks"] if c["rule"] == "fastener_release"]
        self.assertEqual([(c["target"], c["status"]) for c in checks], [("corner/release", "fail")])


class ProfileTests(unittest.TestCase):
    PROFILE = Profile(
        material=Material("test_pla", "test PLA", "test fixture", allowable_strain=0.02),
        printer=Printer("test_printer", nozzle_mm=0.4, layer_mm=0.2, fit_clearance_mm=0.2),
        min_wall_mm=1.6, min_neck_mm=2.0, min_feature_mm=0.8, overhang_angle_deg=50.0,
        bridge_max_mm=8.0, build_direction="plus_z",
    )

    def test_policy_carries_the_profile_values(self):
        policy = self.PROFILE.policy(voxel_mm=0.5)
        self.assertEqual(
            (policy.min_wall_mm, policy.min_neck_mm, policy.min_feature_mm, policy.overhang_angle_deg,
             policy.bridge_max_mm, policy.build_direction, policy.voxel_mm),
            (1.6, 2.0, 0.8, 50.0, 8.0, "plus_z", 0.5),
        )
        self.assertIn("fastener_fit", policy.required)

    def test_assembly_uses_the_printer_fit_clearance(self):
        steps = (Step("open_lid", ("lid",), (Move("plus_z"),)),)
        assembly = self.PROFILE.assembly(steps)
        self.assertEqual((assembly.steps, assembly.fit_clearance_mm), (steps, 0.2))


class FastenerFitTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        Path(".work").mkdir(exist_ok=True)

    def test_self_tapping_fixing_passes(self):
        result = build(model(fixing()))
        checks = fastener_checks(result)
        self.assertEqual(
            [(c["target"], c["status"]) for c in checks],
            [(f"corner/{aspect}", "pass") for aspect in ("through", "bearing", "engagement", "clear_tip", "boss_wall")],
        )
        self.assertIn("engages 6 mm", checks[2]["message"])
        driver = [c for c in result.report["checks"] if c["rule"] == "access_clearance"]
        self.assertEqual({(c["target"], c["status"]) for c in driver}, {("corner_driver/tray", "pass"), ("corner_driver/lid", "pass")})

    def test_short_screw_lacks_engagement(self):
        result = build(model(fixing(replace(M2, length_mm=5.0))))
        self.assertEqual(failing_aspects(result), {"engagement"})
        # 検出箇所は要求されるかかり長さ4 mmの範囲のネジ。
        self.assertEqual(locations(result, "engagement"), box((14, 9, 6), (16, 11, 10)))

    def test_thin_boss_fails_the_wall(self):
        result = build(model(fixing(boss_mm=4.0)))
        self.assertEqual(failing_aspects(result), {"boss_wall"})
        # 下穴1.6 mmに肉1.5 mmを足した輪帯のうち、径4 mmのbossの外側。
        self.assertEqual(locations(result, "boss_wall"), box((12.7, 7.7, 4), (17.3, 12.3, 10)))

    def test_missing_through_hole_blocks_the_screw(self):
        result = build(model(fixing(), clamp_features=()))
        self.assertEqual(failing_aspects(result), {"through"})
        self.assertEqual(locations(result, "through"), box((13.8, 8.8, 10), (16.2, 11.2, 12)))

    def test_through_hole_wider_than_the_head_has_no_bearing(self):
        result = build(model(fixing(), clamp_features=(hole("wide", "z", (15, 10), 4.0, (9.5, 12.5)),)))
        self.assertEqual(failing_aspects(result), {"bearing"})
        self.assertEqual(locations(result, "bearing"), box((13.1, 8.1, 11.5), (16.9, 11.9, 12)))

    def test_shallow_bore_blocks_the_tip(self):
        fix = fixing()
        boss, _ = fix.base_features
        shallow = (boss, hole("corner_bore", "z", (15, 10), 1.6, (6.0, 10.5)))
        result = build(model(fix, base_features=shallow))
        self.assertEqual(failing_aspects(result), {"clear_tip"})
        self.assertEqual(locations(result, "clear_tip"), box((14.2, 9.2, 4), (15.8, 10.8, 6)))

    def test_bore_as_wide_as_the_thread_engages_nothing(self):
        fix = fixing()
        boss, _ = fix.base_features
        wide = (boss, hole("corner_bore", "z", (15, 10), 2.0, (3.0, 10.5)))
        result = build(model(fix, base_features=wide))
        self.assertEqual(failing_aspects(result), {"engagement", "boss_wall"})

    def test_insert_fixing_passes(self):
        result = build(model(fixing(insert=INSERT, boss_mm=7.0)))
        self.assertEqual(failing_aspects(result), set())
        engagement = next(c for c in fastener_checks(result) if c["target"] == "corner/engagement")
        self.assertIn("engages 4 mm", engagement["message"])

    def test_insert_needs_a_boss_wall_around_the_larger_hole(self):
        # 同じboss径でもセルフタップなら足りる。インサート穴は径が大きい。
        result = build(model(fixing(insert=INSERT)))
        self.assertEqual(failing_aspects(result), {"boss_wall"})

    def test_short_screw_barely_enters_the_insert(self):
        result = build(model(fixing(replace(M2, length_mm=5.0), insert=INSERT, boss_mm=7.0)))
        self.assertEqual(failing_aspects(result), {"engagement"})

    def test_unmodelled_clamp_skips_the_clamp_aspects(self):
        fix = fixing(clamp=(), seat_mm=10.0, screw=replace(M2, length_mm=6.0))
        result = build(model(fix, clamp_features=()))
        checks = {c["target"]: c for c in fastener_checks(result)}
        self.assertEqual(failing_aspects(result), set())
        self.assertIn("not modelled", checks["corner/through"]["message"])
        self.assertIn("not modelled", checks["corner/bearing"]["message"])

    def test_model_without_fasteners_has_nothing_to_check(self):
        fix = fixing()
        result = build(model(fix, fasteners=()))
        self.assertEqual([(c["target"], c["status"]) for c in fastener_checks(result)], [("model", "pass")])


if __name__ == "__main__":
    unittest.main()
