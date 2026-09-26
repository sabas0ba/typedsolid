"""keepoutを分解の障害物とし、取付先の部品と一緒に動かす。"""

from dataclasses import replace
import json
from pathlib import Path
import unittest

from typedsolid import (
    Assembly, Box, Clearance, Feature, Keepout, Model, Move, Part, Policy, Step, Sweep,
)
from typedsolid.cadquery import build

# 分解だけを見る。voxel評価は判定に関わらないため、格子を粗くして時間を抑える。
POLICY = Policy(
    voxel_mm=0.5,
    required=("valid_solid", "single_solid", "part_interference", "disassembly_path", "disassembly_separation"),
)
PULL_TRAY = Step("pull_tray", ("tray",), (Move("minus_y"),))


def housing() -> Part:
    """前面 (y=0) が開いた外箱。上板の前端に、下へ垂れる幅2 mmの縁 (z=14〜20) を持つ。"""
    return Part("housing", (
        Feature("bottom", Box((0, 0, 0), (40, 40, 2))),
        Feature("left", Box((0, 0, 0), (2, 40, 22))),
        Feature("right", Box((38, 0, 0), (40, 40, 22))),
        Feature("back", Box((0, 38, 0), (40, 40, 22))),
        Feature("top", Box((0, 0, 20), (40, 40, 22))),
        Feature("bezel", Box((2, 0, 14), (38, 2, 20))),
    ))


def tray() -> Part:
    """外箱の底に載り、前へ引き出す引き出し。背面の壁は高さ12 mm。"""
    return Part("tray", (
        Feature("floor", Box((3, 3, 2), (37, 37, 4))),
        Feature("front", Box((3, 3, 2), (37, 5, 12))),
        Feature("rear", Box((3, 35, 2), (37, 37, 12))),
    ))


def board(top: float = 10.0, attached_to: str | None = "tray") -> Keepout:
    """引き出しの床に載る基板の確保領域。"""
    return Keepout("pcb", Box((8, 8, 4), (32, 32, top)), Clearance(default=0.5, minus_z=0.0), attached_to=attached_to)


def model(keepout: Keepout, sweeps=()) -> Model:
    return Model(
        parts=(housing(), tray()), keepouts=(keepout,), policy=POLICY,
        assembly=Assembly((PULL_TRAY,)), sweeps=sweeps,
    )


def failing(result, rule: str = "disassembly_path") -> set[str]:
    return {c["target"] for c in result.report["checks"] if c["rule"] == rule and c["status"] != "pass"}


class SerializationTests(unittest.TestCase):
    def test_attached_to_is_written_only_when_given(self):
        attached = json.loads(model(board()).to_json())["keepouts"][0]
        self.assertEqual(attached["attached_to"], "tray")
        fixed = json.loads(model(board(attached_to=None)).to_json())["keepouts"][0]
        self.assertNotIn("attached_to", fixed)

    def test_unknown_part_is_rejected(self):
        with self.assertRaises(ValueError):
            model(board(attached_to="ghost")).to_json()

    def test_sweep_after_the_board_left_is_rejected(self):
        late = Sweep("pcb_up", "plus_z", keepout="pcb", after_step="pull_tray")
        with self.assertRaises(ValueError):
            model(board(), sweeps=(late,)).to_json()
        # 外部に固定された基板は、引き出しを抜いた後も残る。
        model(board(attached_to=None), sweeps=(late,)).to_json()


class DisassemblyTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        Path(".work").mkdir(exist_ok=True)

    def test_board_leaves_with_its_tray(self):
        result = build(model(board()))
        self.assertEqual(failing(result), set())
        self.assertEqual(failing(result, "disassembly_separation"), set())
        # 基板は引き出しと一緒に動くため、障害物として現れない。
        targets = {c["target"] for c in result.report["checks"] if c["rule"] == "disassembly_path"}
        self.assertEqual(targets, {"pull_tray/tray/0/housing"})

    def test_fixed_board_blocks_the_tray(self):
        # 基板が外箱や外部に固定されていると、引き出しの背面の壁が基板を通る。
        for attached_to in (None, "housing"):
            with self.subTest(attached_to=attached_to):
                result = build(model(board(attached_to=attached_to)))
                self.assertEqual(failing(result), {"pull_tray/tray/0/keepout:pcb"})

    def test_tall_board_hits_the_bezel_on_the_way_out(self):
        result = build(model(board(top=16.0)))
        self.assertEqual(failing(result), {"pull_tray/tray/0/housing"})

    def test_clearance_is_not_part_of_the_obstacle(self):
        # 背面の壁 (z=12) と縁 (z=14) の間を、clearance 0.5 mmを除けば通る高さ13.8 mmの基板。
        tall = replace(board(top=13.8), clearance_mm=Clearance(default=0.5, minus_z=0.0))
        self.assertEqual(failing(build(model(tall))), set())


if __name__ == "__main__":
    unittest.main()
