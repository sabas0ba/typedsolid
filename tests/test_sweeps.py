"""工具・ケーブル・keepoutの掃引と、分解stepの後の状態での評価。"""

from dataclasses import replace
import json
from pathlib import Path
import unittest

from typedsolid import (
    Assembly, Box, Clearance, Cylinder, Feature, Keepout, Model, Move, Part, Policy, Step, Sweep, boss,
)
from typedsolid.cadquery import build

# 掃引だけを見る。voxel評価は判定に関わらないため、格子を粗くして時間を抑える。
POLICY = Policy(
    voxel_mm=0.5,
    required=("valid_solid", "single_solid", "keepout_clearance", "access_clearance", "part_interference"),
)

OPEN_LID = Step("open_lid", ("lid",), (Move("plus_z"),))
# 基板の確保領域。下面は支持padに接する。
PCB = Keepout("pcb", Box((5, 5, 8), (35, 25, 10)), Clearance(default=0.5, minus_z=0.0))


def tray(opening: bool = True) -> Part:
    """40×30×15 mm、壁と床2 mmの箱。右壁にUSBの開口、内部にネジ受けのboss。"""
    features = (
        Feature("floor", Box((0, 0, 0), (40, 30, 2))),
        Feature("left", Box((0, 0, 0), (2, 30, 15))),
        Feature("right", Box((38, 0, 0), (40, 30, 15))),
        Feature("front", Box((0, 0, 0), (40, 2, 15))),
        Feature("back", Box((0, 28, 0), (40, 30, 15))),
        boss("post", "z", (10, 15), 6.0, (2, 8)),
    )
    if opening:
        features += (Feature("usb_port", Box((37, 10, 5), (41, 20, 10)), operation="cut"),)
    return Part("tray", features)


def lid() -> Part:
    return Part("lid", (Feature("panel", Box((0, 0, 15), (40, 30, 17))),))


def model(*sweeps: Sweep, parts: tuple[Part, ...] | None = None, keepouts=(PCB,)) -> Model:
    return Model(
        parts=parts if parts is not None else (tray(), lid()),
        keepouts=keepouts,
        policy=POLICY,
        assembly=Assembly((OPEN_LID,)),
        sweeps=sweeps,
    )


def access_checks(result) -> list[dict]:
    return [c for c in result.report["checks"] if c["rule"] == "access_clearance"]


def failing_targets(result) -> list[str]:
    return [c["target"] for c in access_checks(result) if c["status"] == "fail"]


# 右壁の外から開口へ差し込むプラグの包絡。指で掴む分を含める。
PLUG = Sweep("usb_plug", "minus_x", shape=Box((42, 11, 6), (55, 19, 9)), distance_mm=10.0)
# ネジ受けの真上から降ろすドライバ軸。上へ抜き取る向きで掃引する。
DRIVER = Sweep("driver", "plus_z", shape=Cylinder("z", (10, 15), 1.5, (8, 12)))


class SerializationTests(unittest.TestCase):
    def test_access_expands_to_the_same_sweep_as_writing_it(self):
        sugar = model(keepouts=(replace(PCB, access=("plus_z",)),))
        explicit = model(Sweep("pcb_plus_z", "plus_z", keepout="pcb"))
        self.assertEqual(sugar.to_json(), explicit.to_json())
        data = json.loads(sugar.to_json())
        self.assertNotIn("access", data["keepouts"][0])
        self.assertEqual(data["sweeps"], [
            {"id": "pcb_plus_z", "keepout": "pcb", "direction": "plus_z", "distance_mm": "exit"},
        ])

    def test_invalid_sweeps_are_rejected(self):
        cases = {
            "unknown step": Sweep("a", "plus_z", keepout="pcb", after_step="ghost"),
            "unknown keepout": Sweep("a", "plus_z", keepout="ghost"),
            "shape and keepout": Sweep("a", "plus_z", shape=Box((0, 0, 0), (1, 1, 1)), keepout="pcb"),
            "neither": Sweep("a", "plus_z"),
            "zero distance": Sweep("a", "plus_z", keepout="pcb", distance_mm=0.0),
        }
        for name, sweep in cases.items():
            with self.subTest(name), self.assertRaises(ValueError):
                model(sweep).to_json()

    def test_access_and_an_explicit_sweep_may_not_share_an_id(self):
        clashing = model(Sweep("pcb_plus_z", "plus_x", keepout="pcb"), keepouts=(replace(PCB, access=("plus_z",)),))
        with self.assertRaises(ValueError):
            clashing.to_json()


class SweepTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        Path(".work").mkdir(exist_ok=True)

    def test_plug_passes_through_the_opening(self):
        # 開口の上辺は幅10 mmの張り出しでsupport_freeが別に落ちる。ここでは掃引だけを見る。
        result = build(model(PLUG))
        self.assertEqual(failing_targets(result), [])
        self.assertEqual({c["target"] for c in access_checks(result)}, {"usb_plug/tray", "usb_plug/lid"})

    def test_plug_is_blocked_without_the_opening(self):
        result = build(model(PLUG, parts=(tray(opening=False), lid())))
        self.assertEqual(failing_targets(result), ["usb_plug/tray"])

    def test_driver_needs_the_lid_removed(self):
        closed = build(model(DRIVER))
        self.assertEqual(failing_targets(closed), ["driver/lid"])
        opened = build(model(replace(DRIVER, after_step="open_lid")))
        self.assertEqual(failing_targets(opened), [])
        self.assertEqual([c["target"] for c in access_checks(opened)], ["driver/tray"])
        self.assertIn("after open_lid", access_checks(opened)[0]["message"])

    def test_board_is_taken_out_after_the_lid(self):
        closed = build(model(keepouts=(replace(PCB, access=("plus_z",)),)))
        self.assertEqual(failing_targets(closed), ["pcb_plus_z/lid"])
        opened = build(model(Sweep("pcb_out", "plus_z", keepout="pcb", after_step="open_lid")))
        self.assertEqual(failing_targets(opened), [])
        self.assertEqual([c["target"] for c in access_checks(opened)], ["pcb_out/tray"])

    def test_finite_distance_stops_short_of_the_lid(self):
        for distance, blocked in ((2.0, False), (5.0, True)):
            with self.subTest(distance=distance):
                result = build(model(replace(DRIVER, distance_mm=distance)))
                self.assertEqual(failing_targets(result), ["driver/lid"] if blocked else [])

    def test_sweep_after_everything_is_removed_has_nothing_to_hit(self):
        steps = (OPEN_LID, Step("take_tray", ("tray",), (Move("minus_z"),)))
        base = model(Sweep("late", "plus_z", keepout="pcb", after_step="take_tray"))
        result = build(replace(base, assembly=Assembly(steps)))
        checks = access_checks(result)
        self.assertEqual([(c["target"], c["status"]) for c in checks], [("late", "pass")])
        self.assertIn("no remaining parts", checks[0]["message"])


if __name__ == "__main__":
    unittest.main()
