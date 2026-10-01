"""検出箇所の位置情報と断面図。判定と同じ格子から描き、検出箇所を塗ることを確かめる。"""

import importlib.util
import json
from pathlib import Path
import shutil
import tempfile
import unittest
import xml.etree.ElementTree as ET

from examples.defects import DEFECTS, baseline, sealed_void, thin_wall
from typedsolid import _native
from typedsolid.cadquery import export
from typedsolid.external import inspect_file
from typedsolid.figures import FIGURES_PER_CHECK, HIGHLIGHT, SOLID_COLOUR, plan, write_model_figures

SCRIPT = Path(__file__).resolve().parents[1] / "scripts" / "render-figures.py"
_spec = importlib.util.spec_from_file_location("render_figures", SCRIPT)
render_figures = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(render_figures)

NS = "{http://www.w3.org/2000/svg}"


def voxel_checks(model_json: str) -> list[dict]:
    return json.loads(_native.evaluate_voxels(model_json))


def fills(path: Path) -> set[str]:
    """SVGの塗りの色。XMLとして読めることも確かめる。"""
    root = ET.parse(path).getroot()
    return {rect.get("fill") for rect in root.iter(f"{NS}rect")}


class TemporaryDirectoryTest(unittest.TestCase):
    def setUp(self):
        Path(".work/tmp").mkdir(parents=True, exist_ok=True)
        self.root = Path(tempfile.mkdtemp(dir=".work/tmp"))
        self.addCleanup(shutil.rmtree, self.root, ignore_errors=True)


class PlanTests(unittest.TestCase):
    def test_overview_for_every_part_and_figures_for_failing_locations(self):
        model = json.loads(sealed_void().to_json())
        figures = plan(model, voxel_checks(sealed_void().to_json()))
        names = [f.name for f in figures]
        self.assertEqual(names, ["enclosure--overview", "enclosure--closed_cavity--1"])
        self.assertIsNone(figures[0].location)
        self.assertEqual(figures[1].rule, "closed_cavity")

    def test_figures_per_check_are_capped(self):
        location = {"min": [0, 0, 0], "max": [1, 1, 1]}
        checks = [{
            "rule": "support_free", "status": "fail", "target": "p", "message": "", "locations": [location] * 5,
        }]
        figures = plan({"parts": [{"id": "p"}]}, checks)
        self.assertEqual(len(figures), 1 + FIGURES_PER_CHECK)
        self.assertIn("location 1 of 5", figures[1].title)

    def test_connector_figures_belong_to_the_connector_part(self):
        location = {"min": [0, 0, 0], "max": [1, 1, 1]}
        model = {"parts": [{"id": "case"}], "connectors": [{"id": "usb", "part": "case"}]}
        checks = [{"rule": "connector_fit", "status": "fail", "target": "usb", "message": "", "locations": [location]}]
        self.assertEqual([f.name for f in plan(model, checks)][1], "case--connector_fit--1")

    def test_passing_and_unknown_rules_get_no_location_figures(self):
        checks = [
            {"rule": "closed_cavity", "status": "pass", "target": "p", "message": ""},
            {"rule": "part_interference", "status": "fail", "target": "p/q", "message": "",
             "locations": [{"min": [0, 0, 0], "max": [1, 1, 1]}]},
        ]
        self.assertEqual(len(plan({"parts": [{"id": "p"}]}, checks)), 1)


class RenderTests(TemporaryDirectoryTest):
    def test_each_defect_is_painted_in_its_rule_colour(self):
        expected = {
            "thin_wall": ("final_wall_thickness",), "narrow_neck": ("neck_section",),
            "severed_corner": ("neck_section",), "cantilever": ("support_free",),
            "sealed_void": ("closed_cavity",),
        }
        for factory in DEFECTS:
            with self.subTest(fixture=factory.__name__):
                model_json = factory().to_json()
                names = write_model_figures(model_json, voxel_checks(model_json), self.root / factory.__name__)
                for rule in expected[factory.__name__]:
                    path = self.root / factory.__name__ / f"enclosure--{rule}--1.svg"
                    self.assertIn(path.name, names)
                    colours = fills(path)
                    self.assertIn(HIGHLIGHT[rule][1], colours)
                    self.assertIn(SOLID_COLOUR, colours)
                    dashed = [r for r in ET.parse(path).getroot().iter(f"{NS}rect") if r.get("stroke-dasharray")]
                    self.assertEqual(len(dashed), 3)

    def test_baseline_overview_has_material_only(self):
        model_json = baseline().to_json()
        names = write_model_figures(model_json, voxel_checks(model_json), self.root)
        self.assertEqual(names, ["enclosure--overview.svg"])
        highlight = {colour for _, colour, _ in HIGHLIGHT.values()}
        self.assertFalse(fills(self.root / names[0]) & highlight)

    def test_render_script_writes_one_figure_per_defect(self):
        written = render_figures.render(self.root, defects_only=True)
        self.assertEqual(len(written), len(DEFECTS))
        self.assertFalse(any(path.name.endswith("--overview.svg") for path in written))


class ExportTests(TemporaryDirectoryTest):
    def test_export_writes_figures_and_lists_them(self):
        manifest = export(baseline(), self.root / "ok", isolated=False)
        self.assertEqual(manifest["figures"], ["enclosure--overview.svg"])
        self.assertTrue((self.root / "ok" / "figures" / "enclosure--overview.svg").exists())

    def test_export_can_skip_figures(self):
        manifest = export(baseline(), self.root / "plain", isolated=False, figures=False)
        self.assertEqual(manifest["figures"], [])
        self.assertFalse((self.root / "plain" / "figures").exists())

    def test_rejected_export_keeps_report_and_figures_only(self):
        with self.assertRaises(ValueError) as caught:
            export(thin_wall(), self.root / "bad")
        self.assertFalse((self.root / "bad").exists())
        rejected = self.root / "bad.rejected"
        self.assertIn(str(rejected), "\n".join(getattr(caught.exception, "__notes__", [])))
        self.assertEqual(sorted(p.name for p in rejected.iterdir()), ["figures", "report.json"])
        report = json.loads((rejected / "report.json").read_text())
        self.assertTrue(report["rejected"])
        self.assertIn("enclosure--final_wall_thickness--1.svg", report["figures"])
        failing = [c for c in report["report"]["checks"] if c["rule"] == "final_wall_thickness"]
        self.assertEqual(failing[0]["status"], "fail")
        self.assertTrue(failing[0]["locations"])

    def test_existing_rejected_directory_is_not_replaced(self):
        (self.root / "bad.rejected").mkdir()
        with self.assertRaises(ValueError) as caught:
            export(thin_wall(), self.root / "bad", isolated=False)
        self.assertIn("already exists", "\n".join(caught.exception.__notes__))
        self.assertEqual(list((self.root / "bad.rejected").iterdir()), [])


class ExternalTests(TemporaryDirectoryTest):
    def test_external_figures_use_the_mesh_grid(self):
        import cadquery as cq
        from typedsolid.cadquery import build

        shape = build(sealed_void()).shapes["enclosure"]
        stl = self.root / "case.stl"
        cq.exporters.export(shape, str(stl), tolerance=0.01, angularTolerance=0.1)
        checks = inspect_file(stl, sealed_void().policy, figures=self.root / "figures")
        cavity = next(c for c in checks if c["rule"] == "closed_cavity")
        self.assertEqual(cavity["status"], "fail")
        written = sorted(p.name for p in (self.root / "figures").iterdir())
        self.assertEqual(written, ["case--closed_cavity--1.svg", "case--overview.svg"])
        self.assertIn(HIGHLIGHT["closed_cavity"][1], fills(self.root / "figures" / "case--closed_cavity--1.svg"))


if __name__ == "__main__":
    unittest.main()
