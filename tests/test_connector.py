"""コネクタ開口のhelper、catalogのコネクタ位置、IR、connector_fitとbackendの掃引検査。"""

from dataclasses import replace
import json
import unittest

from typedsolid import (
    Box, BoardConnector, Fdm, Feature, ManufacturingPlan, Model, Part, PlugSource, Policy, board,
    connector_opening,
)
from typedsolid.catalog import Board, Source
from typedsolid.cadquery import build

# 開口の上縁は幅9.6 mmのbridgeになり、既定のbridge_max_mm (5 mm) を超える。
# 本testは開口の検査だけを見るため、bridgeの許容長を広げる。
POLICY = Policy(
    voxel_mm=0.5,
    required=("valid_solid", "single_solid", "access_clearance", "connector_fit"),
)
PLAN = ManufacturingPlan("fdm", Fdm(bridge_max_mm=12.0), "test fixture")
# testのための値であり、特定の部品の寸法ではない。
SOURCE = PlugSource("measured", "test fixture")


def usb(**overrides):
    """y=0..2の前壁に開ける開口。プラグは嵌合状態でy=1..12を占め、-yへ抜く。"""
    arguments = dict(
        part="case", direction="minus_y", center=(20.0, 7.0), plug_mm=(9.0, 3.5),
        plug_span=(1.0, 12.0), wall_span=(-1.0, 3.0), clearance_mm=0.3, source=SOURCE,
    )
    return connector_opening("usb", **{**arguments, **overrides})


def model(opening=None, *, extra=()) -> Model:
    opening = opening or usb()
    case = Part("case", (
        Feature("floor", Box((0, 0, 0), (40, 30, 2)), "base"),
        Feature("front", Box((0, 0, 0), (40, 2, 14)), "wall"),
        opening.feature,
    ) + extra)
    return Model(
        parts=(case,), policy=POLICY, sweeps=(opening.sweep,), connectors=(opening.connector,),
        default_manufacturing=PLAN,
    )


def checks(result, rule: str) -> list[dict]:
    return [c for c in result.report["checks"] if c["rule"] == rule]


class HelperTests(unittest.TestCase):
    def test_opening_and_plug_boxes(self):
        opening = usb()
        self.assertEqual(opening.feature.operation, "cut")
        self.assertEqual(opening.feature.id, "usb_opening")
        shape = opening.feature.shape
        self.assertEqual(shape.min, (20.0 - 4.8, -1.0, 7.0 - 2.05))
        self.assertEqual(shape.max, (20.0 + 4.8, 3.0, 7.0 + 2.05))
        plug = opening.sweep.shape
        self.assertEqual((plug.min, plug.max), ((15.5, 1.0, 5.25), (24.5, 12.0, 8.75)))
        self.assertEqual((opening.sweep.direction, opening.sweep.distance_mm), ("minus_y", "exit"))
        self.assertEqual(opening.connector.opening, "usb_opening")
        self.assertEqual(opening.connector.sweep, "usb_plug")

    def test_center_follows_the_axis_order_of_cylinders(self):
        shape = usb(direction="plus_x", center=(10.0, 5.0), plug_span=(0.0, 4.0), wall_span=(3.0, 6.0)).feature.shape
        self.assertEqual((shape.min[1], shape.min[2]), (10.0 - 4.8, 5.0 - 2.05))
        self.assertEqual((shape.min[0], shape.max[0]), (3.0, 6.0))

    def test_invalid_arguments(self):
        for overrides in [
            dict(plug_span=(12.0, 1.0)), dict(wall_span=(3.0, 3.0)),
            dict(plug_mm=(0.0, 3.5)), dict(clearance_mm=-0.1),
        ]:
            with self.subTest(overrides=overrides), self.assertRaises(ValueError):
                usb(**overrides)


class IrTests(unittest.TestCase):
    def test_connector_and_source_are_serialized(self):
        data = json.loads(model().to_json())
        self.assertEqual(data["schema_version"], 11)
        self.assertEqual(data["connectors"], [{
            "id": "usb", "part": "case", "opening": "usb_opening", "sweep": "usb_plug",
            "plug_mm": [9.0, 3.5], "clearance_mm": 0.3,
            "source": {"kind": "measured", "reference": "test fixture"},
        }])

    def test_reference_errors_are_rejected_by_rust(self):
        broken = replace(usb().connector, sweep="ghost")
        with self.assertRaisesRegex(ValueError, "unknown sweep"):
            replace(model(), connectors=(broken,)).to_json()

    def test_default_policy_requires_connector_fit(self):
        self.assertIn("connector_fit", Policy().required)


class CatalogTests(unittest.TestCase):
    def test_pi_4_connector_positions(self):
        entry = board("raspberry_pi_4_model_b")
        positions = {item.id: (item.edge, item.offset_mm) for item in entry.connectors}
        self.assertEqual(positions, {
            "usb_c_power": ("minus_y", 11.2), "micro_hdmi_0": ("minus_y", 26.0),
            "micro_hdmi_1": ("minus_y", 39.5), "usb_a_0": ("plus_x", 9.0),
            "usb_a_1": ("plus_x", 27.0), "ethernet": ("plus_x", 45.75),
        })

    def test_connector_center_follows_the_origin(self):
        entry = board("raspberry_pi_4_model_b")
        self.assertEqual(entry.connector_center("usb_c_power", (5.0, 3.0, 4.0), 6.0), (16.2, 6.0))
        self.assertEqual(entry.connector_center("ethernet", (5.0, 3.0, 4.0), 12.0), (48.75, 12.0))
        self.assertEqual(entry.edge_position("plus_x", (5.0, 3.0, 4.0)), 90.0)
        self.assertEqual(entry.edge_position("minus_y", (5.0, 3.0, 4.0)), 3.0)

    def test_unknown_connector_lists_the_registered_ones(self):
        with self.assertRaises(KeyError) as caught:
            board("raspberry_pi_zero_2_w").connector("hdmi")
        self.assertIn("mini_hdmi", str(caught.exception))
        with self.assertRaisesRegex(KeyError, "なし"):
            board("raspberry_pi_5").connector("usb")

    def test_connector_positions_are_validated(self):
        source = Source("t", "https://example.invalid/d.pdf", "rev", "sec")
        for connectors in [
            (BoardConnector("a", "minus_y", 11.0),),
            (BoardConnector("a", "plus_z", 1.0),),
            (BoardConnector("a", "foo_y", 1.0),),
            (BoardConnector("a", "x", 1.0),),
            (BoardConnector("a", "minus_x", 1.0), BoardConnector("a", "plus_x", 2.0)),
        ]:
            with self.subTest(connectors=connectors), self.assertRaises(ValueError):
                Board("t", "t", 10.0, 5.0, (), source, connectors=connectors)


class BackendTests(unittest.TestCase):
    def test_opening_with_clearance_passes_and_exports(self):
        result = build(model())
        fit = checks(result, "connector_fit")
        self.assertEqual([c["status"] for c in fit], ["pass"], fit)
        self.assertIn("test fixture", fit[0]["message"])
        self.assertTrue(all(c["status"] == "pass" for c in checks(result, "access_clearance")))
        self.assertTrue(result.export_allowed, result.report)

    def test_opening_narrowed_by_hand_fails_both_checks(self):
        """開口を手で狭めると、寸法の整合と掃引の干渉の両方で検出する。"""
        opening = usb()
        narrow = replace(opening.feature, shape=replace(opening.feature.shape, max=(24.0, 3.0, 9.05)))
        result = build(model(replace(opening, feature=narrow)))
        self.assertEqual([c["status"] for c in checks(result, "connector_fit")], ["fail"])
        self.assertIn("fail", {c["status"] for c in checks(result, "access_clearance")})
        self.assertFalse(result.export_allowed)

    def test_obstacle_outside_the_wall_blocks_the_plug(self):
        """寸法が整合していても、抜く経路上の部品は掃引の検査で落ちる。"""
        # 前壁の外面に接する柱。単一solidを保ったまま開口の前を塞ぐ。
        post = Feature("post", Box((16, -6, 0), (24, 0, 10)), "rib")
        result = build(model(extra=(post,)))
        self.assertEqual([c["status"] for c in checks(result, "connector_fit")], ["pass"])
        self.assertIn("fail", {c["status"] for c in checks(result, "access_clearance")})
        self.assertFalse(result.export_allowed)


if __name__ == "__main__":
    unittest.main()
