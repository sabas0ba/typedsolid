"""外部のSTL/STEPに最終形状のruleを適用する経路。

第三者のファイルはリポジトリに置かない。ここでは既知の欠陥を持つ筐体fixtureを
自前のIRから出力し、IRを介さない経路でも同じruleが落ちることを確かめる。
"""

import base64
import contextlib
import io
import json
from pathlib import Path
import re
import struct
import subprocess
import sys
import tempfile
import unittest

import cadquery as cq

from tests.test_enclosure_fixtures import FIXTURES, POLICY, baseline, thin_wall
from typedsolid.cadquery import build
from typedsolid.external import inspect_file, main

VOXEL_RULES = frozenset({"final_wall_thickness", "neck_section", "closed_cavity", "support_free"})


def failing(checks: list[dict]) -> frozenset[str]:
    return frozenset(c["rule"] for c in checks if c["status"] == "fail")


class ExternalShapeTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        Path(".work/tmp").mkdir(parents=True, exist_ok=True)
        cls.directory = tempfile.TemporaryDirectory(dir=".work/tmp")
        cls.root = Path(cls.directory.name)
        cls.shapes = {}
        for fixture, _ in FIXTURES:
            result = build(fixture())
            cls.shapes[fixture.__name__] = result.shapes["enclosure"]

    @classmethod
    def tearDownClass(cls):
        cls.directory.cleanup()

    def export(self, name: str, suffix: str) -> Path:
        path = self.root / f"{name}{suffix}"
        if suffix == ".stl":
            cq.exporters.export(self.shapes[name], str(path), tolerance=0.01, angularTolerance=0.1)
        else:
            cq.exporters.export(self.shapes[name], str(path))
        return path

    def test_stl_fails_the_same_voxel_rules_as_the_ir(self):
        for fixture, expected in FIXTURES:
            with self.subTest(fixture=fixture.__name__):
                checks = inspect_file(self.export(fixture.__name__, ".stl"), POLICY)
                self.assertEqual({c["rule"] for c in checks}, VOXEL_RULES)
                self.assertEqual(failing(checks), expected & VOXEL_RULES, checks)

    def test_step_is_tessellated_and_evaluated(self):
        for fixture in (baseline, thin_wall):
            with self.subTest(fixture=fixture.__name__):
                checks = inspect_file(self.export(fixture.__name__, ".step"), POLICY)
                expected = dict(FIXTURES)[fixture] & VOXEL_RULES
                self.assertEqual(failing(checks), expected, checks)

    def test_target_defaults_to_the_file_stem(self):
        checks = inspect_file(self.export("baseline", ".stl"), POLICY)
        self.assertEqual({c["target"] for c in checks}, {"baseline"})

    def test_open_mesh_is_refused(self):
        path = self.root / "open.stl"
        path.write_text(
            "solid open\nfacet normal 0 0 1\nouter loop\n"
            "vertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid open\n"
        )
        with self.assertRaisesRegex(ValueError, "not closed"):
            inspect_file(path, POLICY)

    def test_unknown_format_is_refused(self):
        with self.assertRaisesRegex(ValueError, "対応する形式"):
            inspect_file(self.root / "case.obj", POLICY)

    def test_viewer_draws_the_judged_triangles(self):
        """viewerは判定に使ったSTLの三角形をそのまま埋め込み、checkと検出箇所を持つ。"""
        path = self.export("thin_wall", ".stl")
        figures = self.root / "thin_wall_figures"
        checks = inspect_file(path, POLICY, figures=figures)
        html = (figures / "viewer.html").read_text(encoding="utf-8")
        data = json.loads(re.search(r'id="viewer-data">(.*?)</script>', html, re.S).group(1))
        (part,) = data["parts"]
        self.assertEqual(part["id"], "thin_wall")
        self.assertNotIn("indices", part)
        stl = path.read_bytes()
        (count,) = struct.unpack("<I", stl[80:84])
        self.assertEqual(len(base64.b64decode(part["positions"])), count * 36)
        self.assertEqual(part["bounds"], {"min": [0.0, 0.0, 0.0], "max": [36.0, 26.0, 14.0]})
        self.assertEqual([(c["rule"], c["status"]) for c in data["checks"]], [(c["rule"], c["status"]) for c in checks])
        thin = next(c for c in data["checks"] if c["rule"] == "final_wall_thickness")
        self.assertTrue(thin["locations"])

    def test_cli_reports_the_viewer_size_on_stderr(self):
        path = self.export("baseline", ".stl")
        stdout, stderr = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            main([str(path), "--min-neck-mm", "2.0", "--json", "--figures", str(self.root / "cli_figures")])
        self.assertEqual(len(json.loads(stdout.getvalue())), 4)
        self.assertRegex(stderr.getvalue(), r"viewer: .*viewer\.html \(\d+ triangles, \d+\.\d MB\)")

    def test_stl_viewer_does_not_need_cadquery(self):
        """外部のSTLの検査と図は、CadQueryを読み込まずに書ける。"""
        path = self.export("baseline", ".stl")
        script = (
            "import sys\n"
            "from typedsolid.external import inspect_file\n"
            "from typedsolid import Policy\n"
            f"inspect_file({str(path)!r}, Policy(min_neck_mm=2.0), figures={str(self.root / 'plain_figures')!r})\n"
            "assert 'cadquery' not in sys.modules, 'cadquery was imported'\n"
        )
        subprocess.run([sys.executable, "-c", script], check=True)
        self.assertTrue((self.root / "plain_figures" / "viewer.html").exists())

    def test_cli_exit_status_follows_the_result(self):
        passing = self.export("baseline", ".stl")
        failing_path = self.export("thin_wall", ".stl")
        self.assertEqual(main([str(passing), "--min-neck-mm", "2.0"]), 0)
        self.assertEqual(main([str(failing_path), "--min-neck-mm", "2.0"]), 1)

    def test_cli_resin_requires_every_property(self):
        """光造形の特性は機種と樹脂に依る。FDMの既定値で補わず、省いた値を示して拒否する。"""
        path = self.export("baseline", ".stl")
        stderr = io.StringIO()
        with contextlib.redirect_stderr(stderr), self.assertRaises(SystemExit) as caught:
            main([str(path), "--process", "resin", "--min-drain-mm", "3.0"])
        self.assertEqual(caught.exception.code, 2)
        self.assertIn("--min-wall-mm, --overhang-angle-deg, --bridge-max-mm", stderr.getvalue())
        with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
            main([str(path), "--min-drain-mm", "3.0"])
        resin = [
            str(path), "--min-neck-mm", "2.0", "--json", "--process", "resin", "--min-wall-mm", "1.2",
            "--overhang-angle-deg", "30", "--bridge-max-mm", "5", "--min-drain-mm", "3.0",
        ]
        with contextlib.redirect_stdout(io.StringIO()) as stdout:
            main(resin)
        rules = [check["rule"] for check in json.loads(stdout.getvalue())]
        self.assertEqual(rules[-2:], ["resin_drain", "resin_suction"])


if __name__ == "__main__":
    unittest.main()
