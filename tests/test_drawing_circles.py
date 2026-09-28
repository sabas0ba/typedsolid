import importlib.util
import math
from pathlib import Path
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / "scripts" / "drawing-circles.py"
_spec = importlib.util.spec_from_file_location("drawing_circles", SCRIPT)
drawing_circles = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(drawing_circles)

# cairoが出力する円と同じく、4本の3次Bézier曲線で半径rの円を描く。
K = 0.5522847498


def circle_path(cx: float, cy: float, r: float) -> str:
    k = K * r
    return (
        f"M {cx + r} {cy} "
        f"C {cx + r} {cy + k} {cx + k} {cy + r} {cx} {cy + r} "
        f"C {cx - k} {cy + r} {cx - r} {cy + k} {cx - r} {cy} "
        f"C {cx - r} {cy - k} {cx - k} {cy - r} {cx} {cy - r} "
        f"C {cx + k} {cy - r} {cx + r} {cy - k} {cx + r} {cy} Z"
    )


def svg(body: str) -> Path:
    handle = tempfile.NamedTemporaryFile("w", suffix=".svg", delete=False)
    with handle:
        handle.write(f'<svg xmlns="http://www.w3.org/2000/svg">{body}</svg>')
    return Path(handle.name)


class CircleTests(unittest.TestCase):
    def test_circle_under_nested_transforms(self):
        path = svg(
            '<g transform="translate(10, 20)">'
            f'<path transform="matrix(2, 0, 0, 2, 1, 1)" d="{circle_path(3.0, 4.0, 1.5)}"/>'
            "</g>"
        )
        found = drawing_circles.circles(path)
        self.assertEqual(len(found), 1)
        cx, cy = found[0].center
        self.assertAlmostEqual(cx, 10 + 1 + 2 * 3.0)
        self.assertAlmostEqual(cy, 20 + 1 + 2 * 4.0)
        self.assertAlmostEqual(found[0].diameter, 2 * 2 * 1.5)

    def test_rectangle_and_ellipse_are_not_circles(self):
        rectangle = "M 0 0 L 4 0 L 4 4 L 0 4 Z"
        ellipse = circle_path(0.0, 0.0, 1.0)
        path = svg(
            f'<path d="{rectangle}"/>'
            f'<path transform="scale(3, 1)" d="{ellipse}"/>'
        )
        self.assertEqual(drawing_circles.circles(path), [])

    def test_rotation_keeps_the_diameter(self):
        """回転しても直径は変わらない。端点の外接矩形で測ると√2倍小さくなる。"""
        c = math.cos(math.radians(45.0))
        path = svg(
            f'<path transform="matrix({c}, {c}, {-c}, {c}, 5, 7)" d="{circle_path(0.0, 0.0, 1.0)}"/>'
        )
        found = drawing_circles.circles(path)
        self.assertEqual(len(found), 1)
        self.assertAlmostEqual(found[0].diameter, 2.0, places=3)
        self.assertAlmostEqual(found[0].center[0], 5.0)
        self.assertAlmostEqual(found[0].center[1], 7.0)

    def test_curves_with_cardinal_endpoints_but_not_on_a_circle(self):
        """端点は円と同じでも、制御点が四角へ張り出した曲線は円ではない。"""
        k = 1.0
        d = (
            f"M 1 0 C 1 {k} {k} 1 0 1 C {-k} 1 -1 {k} -1 0 "
            f"C -1 {-k} {-k} -1 0 -1 C {k} -1 1 {-k} 1 0 Z"
        )
        self.assertEqual(drawing_circles.circles(svg(f'<path d="{d}"/>')), [])

    def test_glyph_definitions_are_ignored(self):
        path = svg(f'<defs><path d="{circle_path(0.0, 0.0, 1.0)}"/></defs>')
        self.assertEqual(drawing_circles.circles(path), [])

    def test_several_subpaths_in_one_path(self):
        d = circle_path(0.0, 0.0, 1.0) + " " + circle_path(10.0, 0.0, 2.0)
        found = drawing_circles.circles(svg(f'<path d="{d}"/>'))
        self.assertEqual(sorted(round(item.diameter, 6) for item in found), [2.0, 4.0])
        self.assertTrue(math.isclose(max(item.center[0] for item in found), 10.0))

    def test_relative_commands_are_rejected(self):
        self.assertEqual(drawing_circles.subpaths("m 0 0 l 1 1 z"), [])


if __name__ == "__main__":
    unittest.main()
