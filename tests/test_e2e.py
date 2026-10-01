"""公開CLIから出力し、STEPの再読込とSTLの寸法まで検証する。"""

import hashlib
import json
import math
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
import unittest

import cadquery as cq

LENGTH_MM, WIDTH_MM, HEIGHT_MM = 80.0, 55.0, 24.0
BOSS_CENTERS = ((10, 10), (70, 10), (10, 45), (70, 45))
VENT_X_MM = (22, 30, 38, 46, 54)
OUTPUTS = {"electronics_enclosure.stl", "electronics_enclosure.step", "model.json", "report.json", "figures"}


class EnclosureEndToEndTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        Path(".work").mkdir(exist_ok=True)
        cls.root = tempfile.TemporaryDirectory(dir=".work")
        cls.output = Path(cls.root.name) / "enclosure"
        cls.command = [sys.executable, "examples/electronics_enclosure.py", "--output", str(cls.output)]
        cls.result = subprocess.run(cls.command, capture_output=True, text=True, check=False)

    @classmethod
    def tearDownClass(cls):
        cls.root.cleanup()

    def setUp(self):
        self.assertEqual(self.result.returncode, 0, self.result.stderr)

    def test_outputs_and_digests(self):
        self.assertEqual({path.name for path in self.output.iterdir()}, OUTPUTS)
        manifest = json.loads((self.output / "report.json").read_bytes())
        model = json.loads((self.output / "model.json").read_bytes())
        self.assertEqual(model["keepouts"][0]["shape"]["min"], [10, 10, 6.0])
        self.assertEqual(manifest["model_sha256"], hashlib.sha256((self.output / "model.json").read_bytes()).hexdigest())
        for name, digest in manifest["files_sha256"].items():
            self.assertEqual(hashlib.sha256((self.output / name).read_bytes()).hexdigest(), digest)

    def test_required_rules_pass_and_only_analysis_is_unevaluated(self):
        report = json.loads((self.output / "report.json").read_bytes())["report"]
        checks = report["checks"]
        for rule in report["required"]:
            applicable = [c for c in checks if c["rule"] == rule]
            self.assertTrue(applicable, rule)
            self.assertTrue(all(c["status"] == "pass" for c in applicable), applicable)
        self.assertEqual({c["rule"] for c in checks if c["status"] == "not_evaluated"}, {"strength", "thermal"})
        self.assertIn("Unevaluated: strength, thermal", self.result.stdout)

    def test_step_matches_the_design(self):
        shape = cq.importers.importStep(str(self.output / "electronics_enclosure.step")).val()
        self.assertTrue(shape.isValid())
        self.assertEqual(len(shape.Solids()), 1)
        bounds = shape.BoundingBox()
        for actual, expected in zip((bounds.xlen, bounds.ylen, bounds.zlen), (LENGTH_MM, WIDTH_MM, HEIGHT_MM)):
            self.assertAlmostEqual(actual, expected, places=6)
        # 外殻 - 前面の切り欠き (20×2×17) - スリット5本 (3×2×10) + 床より上のboss4個 - 貫通穴4本。
        shell = LENGTH_MM * WIDTH_MM * HEIGHT_MM - 76 * 51 * 22
        bosses = 4 * math.pi * 4**2 * 4
        holes = 4 * math.pi * 1.5**2 * 6
        self.assertAlmostEqual(shape.Volume(), shell - 680 - 300 + bosses - holes, places=5)
        # 穴、切り欠き、スリットが空いており、その脇に材料が残っている。
        for x, y in BOSS_CENTERS:
            self.assertFalse(shape.isInside(cq.Vector(x, y, 1)))
            self.assertTrue(shape.isInside(cq.Vector(x + 3, y, 4)))
        for z in (11, 23):
            self.assertFalse(shape.isInside(cq.Vector(40, 1, z)))
            self.assertTrue(shape.isInside(cq.Vector(28, 1, z)))
        for x in VENT_X_MM:
            self.assertFalse(shape.isInside(cq.Vector(x + 1.5, 54, 15)))
            self.assertTrue(shape.isInside(cq.Vector(x + 1.5, 54, 21)))

    def test_stl_extent_and_finite_vertices(self):
        stl = (self.output / "electronics_enclosure.stl").read_bytes()
        count = struct.unpack_from("<I", stl, 80)[0]
        self.assertGreater(count, 0)
        self.assertEqual(len(stl), 84 + count * 50)
        vertices = [
            struct.unpack_from("<3f", stl, 84 + triangle * 50 + 12 + vertex * 12)
            for triangle in range(count) for vertex in range(3)
        ]
        self.assertTrue(all(math.isfinite(v) for point in vertices for v in point))
        for axis, maximum in enumerate((LENGTH_MM, WIDTH_MM, HEIGHT_MM)):
            self.assertAlmostEqual(min(p[axis] for p in vertices), 0, places=5)
            self.assertAlmostEqual(max(p[axis] for p in vertices), maximum, places=5)

    def test_existing_output_is_not_overwritten(self):
        before = {path: path.read_bytes() for path in self.output.rglob("*") if path.is_file()}
        repeated = subprocess.run(self.command, capture_output=True, text=True, check=False)
        self.assertNotEqual(repeated.returncode, 0)
        self.assertIn("FileExistsError", repeated.stderr)
        self.assertEqual({path: path.read_bytes() for path in self.output.rglob("*") if path.is_file()}, before)


if __name__ == "__main__":
    unittest.main()
