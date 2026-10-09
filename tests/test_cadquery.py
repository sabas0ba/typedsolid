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
from typedsolid import (
    Box, Clearance, Cylinder, Fdm, Feature, Keepout, ManufacturingPlan, Model, Orientation, Part, Policy, Resin,
    hole,
)
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
        # meshはbuildでは評価できない。exportがSTLを書き出してから判定する。
        self.assertEqual({c["rule"] for c in result.report["checks"] if c["status"] == "not_evaluated"}, {"mesh_manifold", "mesh_volume", "strength", "thermal"})

    def test_disconnected_feature(self):
        result = build(add_feature(block(), Feature("island", Box((20, 0, 0), (25, 5, 5)))))
        self.assertFalse(result.export_allowed)
        self.assertTrue(failures(result, "single_solid"))
        # solidごとの外接boxを体積の大きい順に並べる。
        self.assertEqual(failures(result, "single_solid")[0]["locations"], [
            {"min": [0, 0, 0], "max": [10, 10, 10]}, {"min": [20, 0, 0], "max": [25, 5, 5]},
        ])

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
        # voxel格子 (下限0.01 mm) より小さいため最終形状は評価できず、
        # neck_sectionは別途failする。exportはそれを理由に拒否される。
        model = Model(
            (Part("speck", (Feature("body", Box((0, 0, 0), (0.001, 0.001, 0.001))),)),),
            policy=Policy(min_feature_mm=0.001),
        )
        result = build(model)
        self.assertFalse(failures(result, "valid_solid"), result.report)
        self.assertFalse(failures(result, "single_solid"), result.report)

    def test_thin_additive_feature(self):
        model = Model((Part("plate", (Feature("thin", Box((0, 0, 0), (10, 10, 0.4))),)),))
        self.assertTrue(failures(build(model), "feature_thickness"))

    def test_post_cut_thickness_is_measured(self):
        # primitiveはどれも厚い。壁が薄くなるのはcutの後だけである。
        model = add_feature(block(), Feature("pocket", Box((0.4, 0.4, 0.4), (9.6, 9.6, 11)), operation="cut"))
        result = build(model)
        self.assertFalse(failures(result, "feature_thickness"))
        self.assertTrue(failures(result, "final_wall_thickness"), result.report)
        self.assertFalse(result.export_allowed)

    def test_thick_shell_passes_wall_thickness(self):
        model = add_feature(block(), Feature("pocket", Box((2, 2, 2), (8, 8, 11)), operation="cut"))
        result = build(model)
        self.assertFalse(failures(result, "final_wall_thickness"), result.report)
        self.assertTrue(result.export_allowed, result.report)

    def test_support_free_follows_the_build_direction(self):
        # 片持ちの棚。+Zに積むと棚の下が未支持になる。
        shelf = Model((Part("shelf", (
            Feature("post", Box((0, 0, 0), (4, 4, 20))),
            Feature("deck", Box((4, 0, 14), (16, 4, 18))),
        )),), policy=Policy(min_neck_mm=1.0), default_manufacturing=ManufacturingPlan("fdm", Fdm(min_wall_mm=1.0), "test"))
        self.assertTrue(failures(build(shelf), "support_free"))
        # 寝かせて積むと同じ形状が支持される。
        plan = replace(shelf.default_manufacturing, orientation=Orientation("plus_x"))
        laid = replace(shelf, default_manufacturing=plan)
        self.assertFalse(failures(build(laid), "support_free"))

    def test_other_plans_are_reported_without_blocking_export(self):
        """採用していない製造案のfailは、比較として載るが出力を止めない。"""
        # 上面が開いた箱。FDMで上向きに積めば通り、UV樹脂で同じ向きに積むと吸盤になる。
        box = Model((Part("box", (
            Feature("block", Box((0, 0, 0), (20, 20, 12))),
            Feature("inside", Box((2, 2, 2), (18, 18, 13)), operation="cut"),
        ), manufacturing=(
            ManufacturingPlan("fdm", Fdm(), "test"),
            ManufacturingPlan("resin", Resin(0.8, 45.0, 5.0, 2.0), "test"),
        ), adopted="fdm"),), policy=Policy(voxel_mm=0.5))
        result = build(box)
        self.assertTrue(result.export_allowed, result.report)
        suction = [c for c in result.report["checks"] if c["rule"] == "resin_suction"]
        self.assertEqual([(c["plan"], c["status"], c["adopted"]) for c in suction], [("resin", "fail", False)])
        adopted = replace(box, parts=(replace(box.parts[0], adopted="resin"),))
        self.assertFalse(build(adopted).export_allowed)

    def test_sealed_cavity_blocks_export(self):
        model = add_feature(block(), Feature("void", Box((3, 3, 3), (7, 7, 7)), operation="cut"))
        result = build(model)
        self.assertTrue(failures(result, "closed_cavity"), result.report)
        self.assertFalse(result.export_allowed)

    def test_narrow_neck_blocks_export(self):
        # 5 mm角の塊2つを1 mm角の首で繋ぐ。単一solidだが断面が足りない。
        dumbbell = Model((Part("bar", (
            Feature("left", Box((0, 0, 0), (5, 5, 5))),
            Feature("neck", Box((5, 2, 2), (7, 3, 3))),
            Feature("right", Box((7, 0, 0), (12, 5, 5))),
        )),), default_manufacturing=ManufacturingPlan("fdm", Fdm(min_wall_mm=0.5), "test"))
        result = build(dumbbell)
        self.assertFalse(failures(result, "single_solid"), result.report)
        self.assertTrue(failures(result, "neck_section"), result.report)

    def test_keepout_collision(self):
        keepout = Keepout("pcb", Box((1, 1, 1), (2, 2, 2)))
        self.assertTrue(failures(build(replace(block(), keepouts=(keepout,))), "keepout_clearance"))

    def test_margin_is_measured_on_final_geometry(self):
        keepout = Keepout("pcb", Box((10.25, 1, 1), (12, 2, 2)), Clearance(default=0.5))
        self.assertTrue(failures(build(replace(block(), keepouts=(keepout,))), "keepout_clearance"))
        relaxed = replace(keepout, clearance_mm=Clearance(default=0))
        self.assertTrue(build(replace(block(), keepouts=(relaxed,))).export_allowed)

    def test_clearance_applies_per_face(self):
        # blockはx=0..10。-X側に0.5 mmの間隔しかないkeepoutを置く。
        keepout = Keepout("pcb", Box((10.5, 1, 1), (12, 2, 2)), Clearance(default=1.0))
        self.assertTrue(failures(build(replace(block(), keepouts=(keepout,))), "keepout_clearance"))
        # 接する面だけを0にすれば、他の面の要求は残したまま通る。
        relaxed = replace(keepout, clearance_mm=Clearance(default=1.0, minus_x=0.0))
        self.assertTrue(build(replace(block(), keepouts=(relaxed,))).export_allowed)

    def test_cylinder_volume_matches_the_analytic_value(self):
        model = Model((Part("post", (Feature("stem", Cylinder("z", (0, 0), 2.0, (0, 10))),)),))
        result = build(model)
        self.assertTrue(result.export_allowed, result.report)
        volume = sum(s.Volume() for s in result.shapes["post"].Solids())
        self.assertAlmostEqual(volume, math.pi * 4.0 * 10.0, places=6)

    def test_cylinder_axes_follow_the_declared_direction(self):
        for axis, expected in (("x", (0, 1)), ("y", (1, 0)), ("z", (2, 0))):
            with self.subTest(axis=axis):
                model = Model((Part("post", (Feature("stem", Cylinder(axis, (0, 0), 2.0, (0, 10))),)),))
                box = build(model).shapes["post"].BoundingBox()
                lengths = (box.xlen, box.ylen, box.zlen)
                self.assertAlmostEqual(lengths[expected[0]], 10.0, places=6)
                self.assertAlmostEqual(lengths[expected[1]], 4.0, places=6)

    def test_hole_removes_material(self):
        drilled = add_feature(block(), hole("bore", "z", (5, 5), 4.0, (-1, 11)))
        result = build(drilled)
        self.assertTrue(result.export_allowed, result.report)
        volume = sum(s.Volume() for s in result.shapes["block"].Solids())
        self.assertAlmostEqual(volume, 1000 - math.pi * 4.0 * 10.0, places=5)

    def test_access_is_checked_in_each_declared_direction(self):
        # blockの-X側に接するkeepout。-Xは開いており、+Xはblockが塞ぐ。
        keepout = Keepout("pcb", Box((-3, 1, 1), (-1, 2, 2)), Clearance(default=0.0))
        opened = build(replace(block(), keepouts=(replace(keepout, access=("minus_x",)),)))
        self.assertTrue(opened.export_allowed, opened.report)
        blocked = build(replace(block(), keepouts=(replace(keepout, access=("plus_x",)),)))
        self.assertTrue(failures(blocked, "access_clearance"))
        # accessは<keepout>_<direction>の掃引に展開される。
        self.assertEqual(failures(blocked, "access_clearance")[0]["target"], "pcb_plus_x/block")

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
        for rule in ("strength", "thermal"):
            with self.subTest(rule=rule):
                model = replace(block(), policy=Policy(required=(rule,)))
                self.assertFalse(build(model).export_allowed)

    def test_kernel_exception_is_not_a_pass(self):
        with patch("typedsolid.cadquery._solid", side_effect=RuntimeError("kernel failed")):
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

    def test_exported_mesh_is_inspected(self):
        with tempfile.TemporaryDirectory(dir=".work") as root:
            manifest = export(block(), Path(root) / "mesh")
            checks = {c["rule"]: c for c in manifest["report"]["checks"] if c["rule"].startswith("mesh_")}
            self.assertEqual({r: c["status"] for r, c in checks.items()}, {"mesh_manifold": "pass", "mesh_volume": "pass"})
            self.assertEqual({c["target"] for c in checks.values()}, {"block"})

    def test_cut_geometry_exports_a_closed_mesh(self):
        model = add_feature(block(), Feature("pocket", Box((2, 2, 2), (8, 8, 11)), operation="cut"))
        with tempfile.TemporaryDirectory(dir=".work") as root:
            manifest = export(model, Path(root) / "pocket")
            mesh = [c for c in manifest["report"]["checks"] if c["rule"].startswith("mesh_")]
            self.assertEqual([c["status"] for c in mesh], ["pass", "pass"], mesh)

    def test_broken_mesh_blocks_export(self):
        real = cq.exporters.export

        def truncated(shape, path, *args, **kwargs):
            real(shape, path, *args, **kwargs)
            if str(path).endswith(".stl"):
                # 三角形を1つ落とし、長さも整合させる。解析は通り閉じたmeshでなくなる。
                content = bytearray(Path(path).read_bytes())
                count = struct.unpack_from("<I", content, 80)[0] - 1
                struct.pack_into("<I", content, 80, count)
                Path(path).write_bytes(bytes(content[: 84 + 50 * count]))

        with tempfile.TemporaryDirectory(dir=".work") as root:
            output = Path(root) / "broken"
            # patchは子processに及ばないため、同一processで実行する。
            with patch("cadquery.exporters.export", truncated):
                with self.assertRaises(ValueError) as raised:
                    export(block(), output, isolated=False)
            self.assertIn("mesh_manifold", str(raised.exception))
            self.assertFalse(output.exists())

    def test_public_build_mutation_does_not_bypass_export(self):
        model = replace(block(), policy=Policy(required=("strength",)))
        result = build(model)
        result.report["required"].clear()
        with tempfile.TemporaryDirectory(dir=".work") as root, self.assertRaises(ValueError):
            export(model, Path(root) / "bad")
