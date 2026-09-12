"""公開CLIからCAD出力・再読込までを検証する。"""

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


class EnclosureEndToEndTests(unittest.TestCase):
    def test_exported_enclosure_matches_design(self):
        Path(".work").mkdir(exist_ok=True)
        with tempfile.TemporaryDirectory(dir=".work") as root:
            output = Path(root) / "enclosure"
            command = [sys.executable, "examples/electronics_enclosure.py", "--output", str(output)]
            result = subprocess.run(command, capture_output=True, text=True, check=False)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(
                {path.name for path in output.iterdir()},
                {"electronics_enclosure.stl", "electronics_enclosure.step", "model.json", "report.json"},
            )
            manifest = json.loads((output / "report.json").read_bytes())
            model = json.loads((output / "model.json").read_bytes())
            self.assertEqual(model["keepouts"][0]["bounds"]["min"], [10, 10, 6])
            self.assertEqual(manifest["model_sha256"],
                             hashlib.sha256((output / "model.json").read_bytes()).hexdigest())
            for name, digest in manifest["files_sha256"].items():
                self.assertEqual(hashlib.sha256((output / name).read_bytes()).hexdigest(), digest)
            checks = manifest["report"]["checks"]
            for rule in manifest["report"]["required"]:
                applicable = [c for c in checks if c["rule"] == rule]
                self.assertTrue(applicable, rule)
                self.assertTrue(all(c["status"] == "pass" for c in applicable), applicable)
            self.assertEqual({c["rule"] for c in checks if c["status"] == "not_evaluated"},
                             {"final_wall_thickness", "support_free", "strength", "thermal"})
            shape = cq.importers.importStep(str(output / "electronics_enclosure.step")).val()
            self.assertTrue(shape.isValid())
            self.assertEqual(len(shape.Solids()), 1)
            bounds = shape.BoundingBox()
            for actual, expected in zip((bounds.xlen, bounds.ylen, bounds.zlen), (80, 55, 24)):
                self.assertAlmostEqual(actual, expected, places=6)
            # shell - connector - five vents + four bosses above floor - four through holes
            expected_volume = 80 * 55 * 24 - 76 * 51 * 22 - 320 - 300 + 202 * math.pi
            self.assertAlmostEqual(shape.Volume(), expected_volume, places=5)
            # STEP再読込後も穴・矩形開口の空間が空いていることを確認する。
            for x, y in ((10, 10), (70, 10), (10, 45), (70, 45)):
                self.assertFalse(shape.isInside(cq.Vector(x, y, 1)))
                self.assertTrue(shape.isInside(cq.Vector(x + 3, y, 4)))
            self.assertFalse(shape.isInside(cq.Vector(40, 1, 11)))
            self.assertTrue(shape.isInside(cq.Vector(28, 1, 11)))
            for x in (22, 30, 38, 46, 54):
                self.assertFalse(shape.isInside(cq.Vector(x + 1, 54, 15)))
            stl = (output / "electronics_enclosure.stl").read_bytes()
            count = struct.unpack_from("<I", stl, 80)[0]
            self.assertGreater(count, 0)
            self.assertEqual(len(stl), 84 + count * 50)
            vertices = [
                struct.unpack_from("<3f", stl, 84 + triangle * 50 + 12 + vertex * 12)
                for triangle in range(count) for vertex in range(3)
            ]
            self.assertTrue(all(math.isfinite(v) for point in vertices for v in point))
            for axis, maximum in enumerate((80, 55, 24)):
                self.assertAlmostEqual(min(p[axis] for p in vertices), 0, places=6)
                self.assertAlmostEqual(max(p[axis] for p in vertices), maximum, places=6)
            before = {p.name: p.read_bytes() for p in output.iterdir()}
            repeated = subprocess.run(command, capture_output=True, text=True, check=False)
            self.assertNotEqual(repeated.returncode, 0)
            self.assertEqual({p.name: p.read_bytes() for p in output.iterdir()}, before)
