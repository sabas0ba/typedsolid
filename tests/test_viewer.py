"""3D viewerのHTMLの書き出しと、撮影した画面を検査するscriptのPNG読み取り。

描画そのものはscripts/check-viewer.pyがheadless Chromiumで確かめる。JavaScriptの処理は
tests/js/のnode:testが確かめる。
"""

import base64
import importlib.util
import json
from pathlib import Path
import re
import shutil
import struct
import tempfile
import unittest
import zlib

from examples.assembly_defects import lid_overlap
from examples.defects import baseline
from typedsolid import Box, Feature, Model, Part
from typedsolid.cadquery import build, export, write_viewer_figure
from typedsolid.viewer import render_viewer

SCRIPT = Path(__file__).resolve().parents[1] / "scripts" / "check-viewer.py"
_spec = importlib.util.spec_from_file_location("check_viewer", SCRIPT)
check_viewer = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(check_viewer)


def embedded(html: str) -> dict:
    return json.loads(re.search(r'<script type="application/json" id="viewer-data">(.*?)</script>', html, re.S).group(1))


def png(width: int, height: int, rows: list[bytes], filters: list[int]) -> bytes:
    """RGBの行と行ごとのfilterの種類からPNGを作る。read_pngの検査に使う。"""
    size = 3
    encoded = bytearray()
    previous = bytes(width * size)
    for row, kind in zip(rows, filters):
        out = bytearray()
        for i, value in enumerate(row):
            left = row[i - size] if i >= size else 0
            up = previous[i]
            upper_left = previous[i - size] if i >= size else 0
            predictor = (0, left, up, (left + up) >> 1, check_viewer._paeth(left, up, upper_left))[kind]
            out.append((value - predictor) & 0xFF)
        encoded += bytes([kind]) + out
        previous = row

    def chunk(kind: bytes, body: bytes) -> bytes:
        return struct.pack(">I", len(body)) + kind + body + struct.pack(">I", zlib.crc32(kind + body))

    header = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header) + chunk(b"IDAT", zlib.compress(bytes(encoded))) + chunk(b"IEND", b"")


class TemporaryDirectoryTest(unittest.TestCase):
    def setUp(self):
        Path(".work/tmp").mkdir(parents=True, exist_ok=True)
        self.root = Path(tempfile.mkdtemp(dir=".work/tmp"))
        self.addCleanup(shutil.rmtree, self.root, ignore_errors=True)


class ViewerHtmlTests(TemporaryDirectoryTest):
    def test_parts_checks_and_keepouts_are_embedded(self):
        result = build(lid_overlap())
        path = write_viewer_figure(result, self.root / "viewer.html")
        data = embedded(path.read_text(encoding="utf-8"))
        self.assertEqual([part["id"] for part in data["parts"]], ["tray", "lid"])
        for part in data["parts"]:
            positions = base64.b64decode(part["positions"])
            indices = struct.unpack(f"<{len(base64.b64decode(part['indices'])) // 4}I", base64.b64decode(part["indices"]))
            self.assertEqual(len(positions) % 12, 0)
            self.assertEqual(len(indices) % 3, 0)
            self.assertLess(max(indices), len(positions) // 12)
        self.assertEqual(len(data["checks"]), len(result.report["checks"]))
        failing = [c for c in data["checks"] if c["status"] == "fail"]
        self.assertEqual(failing[0]["locations"], [{"min": [0, 0, 14], "max": [40, 30, 15]}])
        self.assertEqual(data["parts"][0]["bounds"], {"min": [0, 0, 0], "max": [40, 30, 15]})

    def test_text_cannot_close_the_script_or_inject_markup(self):
        hostile = '</script><script>alert(1)</script> & {{APP}}'
        checks = [{"rule": "part_interference", "target": hostile, "status": "fail", "message": hostile}]
        html = render_viewer([], checks, title=hostile)
        self.assertNotIn("<script>alert(1)", html)
        self.assertIn("<title>&lt;/script&gt;", html)
        self.assertEqual(html.count("</script>"), 3)
        data = embedded(html)
        self.assertEqual(data["checks"][0]["message"], hostile)
        self.assertEqual(data["title"], hostile)

    def test_model_without_drawable_parts_still_lists_the_checks(self):
        removed = Model((Part("block", (
            Feature("body", Box((0, 0, 0), (10, 10, 10))),
            Feature("remove", Box((-1, -1, -1), (11, 11, 11)), operation="cut"),
        )),))
        result = build(removed)
        data = embedded(write_viewer_figure(result, self.root / "viewer.html").read_text(encoding="utf-8"))
        self.assertEqual(data["parts"], [])
        self.assertIn("valid_solid", {c["rule"] for c in data["checks"] if c["status"] == "fail"})

    def test_output_is_reproducible(self):
        first = write_viewer_figure(build(lid_overlap()), self.root / "a.html").read_bytes()
        second = write_viewer_figure(build(lid_overlap()), self.root / "b.html").read_bytes()
        self.assertEqual(first, second)


class ExportTests(TemporaryDirectoryTest):
    def test_export_writes_the_viewer(self):
        manifest = export(baseline(), self.root / "ok", isolated=False)
        self.assertIn("viewer.html", manifest["figures"])
        self.assertTrue((self.root / "ok" / "figures" / "viewer.html").exists())

    def test_viewer_shows_the_mesh_checks_of_the_final_report(self):
        """meshの検査は書き出し後に決まる。viewerの状態はreport.jsonと一致する。"""
        manifest = export(baseline(), self.root / "mesh", isolated=False)
        data = embedded((self.root / "mesh" / "figures" / "viewer.html").read_text(encoding="utf-8"))
        statuses = lambda checks: [(c["rule"], c["target"], c["status"]) for c in checks]
        self.assertEqual(statuses(data["checks"]), statuses(manifest["report"]["checks"]))
        self.assertIn(("mesh_manifold", "pass"), {(c["rule"], c["status"]) for c in data["checks"]})

    def test_rejected_export_keeps_the_viewer(self):
        with self.assertRaises(ValueError):
            export(lid_overlap(), self.root / "bad")
        rejected = self.root / "bad.rejected"
        self.assertIn("viewer.html", json.loads((rejected / "report.json").read_text())["figures"])
        data = embedded((rejected / "figures" / "viewer.html").read_text(encoding="utf-8"))
        self.assertTrue(any(c["status"] == "fail" for c in data["checks"]))


class ScreenshotCheckTests(TemporaryDirectoryTest):
    def test_png_rows_with_every_filter_are_decoded(self):
        width, height = 5, 5
        rows = [bytes((x * 40 + y * 7 + c * 3) % 256 for x in range(width) for c in range(3)) for y in range(height)]
        path = self.root / "filters.png"
        path.write_bytes(png(width, height, rows, [0, 1, 2, 3, 4]))
        self.assertEqual(check_viewer.read_png(path), (width, height, 3, b"".join(rows)))

    def test_blank_screen_fails_every_scene(self):
        """scriptやWebGLが失敗すると画面は白く残る。どの状態の検査も落ちる。"""
        width, height = check_viewer.CANVAS_WIDTH, 4
        path = self.root / "white.png"
        path.write_bytes(png(width, height, [b"\xff" * width * 3] * height, [0] * height))
        found = check_viewer.fractions(path)
        self.assertEqual(found, {"location": 0.0, "part": 0.0, "cut": 0.0})
        viewer = check_viewer.Viewer("lid_overlap", ("tray", "lid"), [{"status": "pass"}, {"status": "fail"}], 17.0)
        seen = {"selected": {"location": 0.01}}
        for scene in check_viewer.scenes(viewer):
            with self.subTest(scene=scene.name):
                self.assertTrue(scene.expect(found, seen))

    def test_colour_classes(self):
        self.assertTrue(check_viewer.is_location(214, 38, 40))
        self.assertTrue(check_viewer.is_location(120, 45, 50))
        self.assertFalse(check_viewer.is_location(204, 173, 122))
        self.assertTrue(check_viewer.is_part(110, 125, 145))
        self.assertFalse(check_viewer.is_part(255, 255, 255))
        self.assertTrue(check_viewer.is_cut(89, 94, 102))


if __name__ == "__main__":
    unittest.main()
