"""OCCTで判定するruleの検出箇所と投影図。部品間の欠陥fixtureで、判定、箇所、図を確かめる。"""

from dataclasses import replace
import importlib.util
import json
from pathlib import Path
import shutil
import tempfile
import unittest
import xml.etree.ElementTree as ET

import cadquery as cq

from examples.assembly_defects import ASSEMBLY_DEFECTS, lid_overlap, post_in_keepout
from examples.defects import baseline
from typedsolid import Box, Feature, Model, Part, _native
from typedsolid.cadquery import _check, build, export, write_projection_figures
from typedsolid.projection import LOCATION_STYLE, VIEWS, Projection, write_projections

SCRIPT = Path(__file__).resolve().parents[1] / "scripts" / "render-projections.py"
_spec = importlib.util.spec_from_file_location("render_projections", SCRIPT)
render_projections = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(render_projections)

NS = "{http://www.w3.org/2000/svg}"
LOCATION_COLOUR = LOCATION_STYLE.split('"')[1]

# fixtureごとの検出箇所。共通部分の外接boxであり、手計算の値と一致する。
EXPECTED_LOCATIONS = {
    "lid_overlap": ((0, 0, 14), (40, 30, 15)),
    "blocked_cable": ((38, 10, 5), (40, 20, 10)),
    "post_in_keepout": ((10, 13, 2), (15, 18, 6.5)),
    "sliding_lid": ((38, 2, 11), (40, 28, 13)),
}


def strokes(path: Path) -> list[str]:
    """SVGのpathの線の色。XMLとして読めることも確かめる。"""
    return [element.get("stroke") for element in ET.parse(path).getroot().iter(f"{NS}path")]


class TemporaryDirectoryTest(unittest.TestCase):
    def setUp(self):
        Path(".work/tmp").mkdir(parents=True, exist_ok=True)
        self.root = Path(tempfile.mkdtemp(dir=".work/tmp"))
        self.addCleanup(shutil.rmtree, self.root, ignore_errors=True)


class LocationTests(unittest.TestCase):
    def test_each_defect_fails_exactly_its_rule_at_the_expected_box(self):
        for factory, expected in ASSEMBLY_DEFECTS:
            with self.subTest(fixture=factory.__name__):
                checks = build(factory()).report["checks"]
                failing = [c for c in checks if c["status"] == "fail"]
                self.assertEqual({(c["rule"], c["target"]) for c in failing}, expected)
                low, high = EXPECTED_LOCATIONS[factory.__name__]
                self.assertEqual(failing[0]["locations"], [{"min": list(low), "max": list(high)}])
                self.assertTrue(failing[0]["message"].endswith("; 1 location(s)"), failing[0]["message"])

    def test_passing_checks_have_no_locations(self):
        checks = build(baseline()).report["checks"]
        self.assertFalse([c for c in checks if c.get("locations")])

    def test_locations_are_ordered_by_volume_and_capped(self):
        located = [(float(i), {"min": [i, 0, 0], "max": [i + 1, 1, 1]}) for i in range(25)]
        check = _check("part_interference", "a/b", False, "overlap", located)
        self.assertEqual(len(check["locations"]), _native.MAX_LOCATIONS)
        self.assertEqual(check["locations"][0]["min"], [24, 0, 0])
        self.assertTrue(check["message"].endswith("; 25 location(s)"))
        self.assertNotIn("locations", _check("part_interference", "a/b", True, "overlap", located))


class ProjectionTests(TemporaryDirectoryTest):
    def test_projected_box_matches_the_hidden_line_output(self):
        """boxの投影と、同じboxをOCCTが隠線処理した線の範囲が一致する。図に重ねる箇所の位置の根拠。"""
        low, high = (1.0, 2.0, 3.0), (7.0, 5.0, 4.0)
        projection = Projection([cq.Solid.makeBox(6, 3, 1, pnt=cq.Vector(*low))])
        for view, (visible, _) in zip(VIEWS, projection.lines):
            with self.subTest(view=view.name):
                corners = [
                    view.project([high[a] if corner >> a & 1 else low[a] for a in range(3)]) for corner in range(8)
                ]
                points = [point for line in visible for point in line]
                for index in range(2):
                    self.assertAlmostEqual(min(p[index] for p in points), min(c[index] for c in corners), places=6)
                    self.assertAlmostEqual(max(p[index] for p in points), max(c[index] for c in corners), places=6)

    def test_overview_and_one_figure_per_failing_check(self):
        result = build(lid_overlap())
        names = write_projections(result.shapes, result.report["checks"], self.root)
        self.assertEqual(names, ["overview.svg", "part_interference--tray_lid.svg"])
        self.assertNotIn(LOCATION_COLOUR, strokes(self.root / "overview.svg"))
        # 4方向それぞれに検出箇所の線を描く。
        self.assertEqual(strokes(self.root / names[1]).count(LOCATION_COLOUR), len(VIEWS))

    def test_output_is_reproducible(self):
        result = build(post_in_keepout())
        first = write_projections(result.shapes, result.report["checks"], self.root / "a")
        second = write_projections(build(post_in_keepout()).shapes, result.report["checks"], self.root / "b")
        self.assertEqual(first, second)
        for name in first:
            self.assertEqual((self.root / "a" / name).read_bytes(), (self.root / "b" / name).read_bytes())

    def test_colliding_names_are_numbered(self):
        location = {"min": [0, 0, 0], "max": [1, 1, 1]}
        checks = [
            {"rule": "part_interference", "status": "fail", "target": target, "message": "", "locations": [location]}
            for target in ("a/b", "a_b")
        ]
        shapes = {"block": cq.Solid.makeBox(1, 1, 1)}
        names = write_projections(shapes, checks, self.root, overview=False)
        self.assertEqual(names, ["part_interference--a_b.svg", "part_interference--a_b--2.svg"])

    def test_no_shapes_writes_nothing(self):
        self.assertEqual(write_projections({}, [], self.root / "none"), [])
        self.assertFalse((self.root / "none").exists())

    def test_render_script_writes_one_figure_per_defect(self):
        written = render_projections.render(self.root)
        self.assertEqual(len(written), len(ASSEMBLY_DEFECTS))
        self.assertTrue(all(LOCATION_COLOUR in strokes(path) for path in written))


class ExportTests(TemporaryDirectoryTest):
    def test_part_without_material_still_leaves_the_rejection_report(self):
        """cutで材料が残らない部品は投影できない。投影図を省き、reportと断面図は残す。"""
        removed = Model((Part("block", (
            Feature("body", Box((0, 0, 0), (10, 10, 10))),
            Feature("remove", Box((-1, -1, -1), (11, 11, 11)), operation="cut"),
        )),))
        with self.assertRaises(ValueError):
            export(removed, self.root / "empty", isolated=False)
        report = json.loads((self.root / "empty.rejected" / "report.json").read_text())
        self.assertFalse([name for name in report["figures"] if name.startswith("projection/")])
        self.assertIn("block--overview.svg", report["figures"])

    def test_part_without_material_is_left_out_of_the_assembly_projection(self):
        empty = Part("ghost", (
            Feature("body", Box((20, 0, 0), (25, 5, 5))),
            Feature("remove", Box((19, -1, -1), (26, 6, 6)), operation="cut"),
        ))
        model = replace(lid_overlap(), parts=(*lid_overlap().parts, empty))
        names = write_projection_figures(build(model), self.root)
        self.assertIn("overview.svg", names)


    def test_export_lists_the_projection_overview(self):
        manifest = export(baseline(), self.root / "ok", isolated=False)
        self.assertIn("projection/overview.svg", manifest["figures"])
        self.assertTrue((self.root / "ok" / "figures" / "projection" / "overview.svg").exists())

    def test_rejected_export_keeps_the_projection_of_the_failing_check(self):
        with self.assertRaises(ValueError):
            export(lid_overlap(), self.root / "bad")
        rejected = self.root / "bad.rejected"
        report = json.loads((rejected / "report.json").read_text())
        self.assertIn("projection/part_interference--tray_lid.svg", report["figures"])
        self.assertTrue((rejected / "figures" / "projection" / "part_interference--tray_lid.svg").exists())


if __name__ == "__main__":
    unittest.main()
