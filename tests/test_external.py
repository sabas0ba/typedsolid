"""外部のSTL/STEPに最終形状のruleを適用する経路。

第三者のファイルはリポジトリに置かない。ここでは既知の欠陥を持つ筐体fixtureを
自前のIRから出力し、IRを介さない経路でも同じruleが落ちることを確かめる。
"""

from pathlib import Path
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

    def test_cli_exit_status_follows_the_result(self):
        passing = self.export("baseline", ".stl")
        failing_path = self.export("thin_wall", ".stl")
        self.assertEqual(main([str(passing), "--min-neck-mm", "2.0"]), 0)
        self.assertEqual(main([str(failing_path), "--min-neck-mm", "2.0"]), 1)


if __name__ == "__main__":
    unittest.main()
