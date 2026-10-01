"""開口を持つ筐体に既知の欠陥を1つずつ入れ、想定どおりのruleだけが落ちることを見る。

roadmapのM1合格条件に対応する。個々のruleは単純形状のtestが検証済みである。
ここで確かめるのは次の2点である。

- 壁、床、開口、内部の造作が揃った形状でも同じ判定が成立すること
- 欠陥のない筐体が過検出を起こさないこと

各fixtureは失敗するruleの集合を完全一致で宣言する。検出漏れと過検出の
どちらもこの比較で落ちる。複数のruleが落ちる組み合わせは、欠陥の性質上
不可分なものに限り、理由をfixtureのコメントに書く。
"""

from dataclasses import replace
from pathlib import Path
import unittest

from examples.defects import (
    HEIGHT_MM, LENGTH_MM, POLICY, WALL_MM, WIDTH_MM, baseline, cantilever, enclosure, narrow_neck,
    sealed_void, severed_corner, thin_wall,
)
from typedsolid import Box, Feature, Policy
from typedsolid.cadquery import build

# 各fixtureと、落ちるべきruleの集合。
FIXTURES = (
    (baseline, frozenset()),
    (thin_wall, frozenset({"final_wall_thickness"})),
    (narrow_neck, frozenset({"neck_section"})),
    (severed_corner, frozenset({"single_solid", "neck_section"})),
    (cantilever, frozenset({"support_free"})),
    (sealed_void, frozenset({"closed_cavity"})),
)


def failing_rules(result) -> frozenset[str]:
    return frozenset(c["rule"] for c in result.report["checks"] if c["status"] == "fail")


def unsupported_mm3(result) -> float:
    """support_freeが報告する未支持体積。messageの先頭に置かれる。"""
    message = next(c["message"] for c in result.report["checks"] if c["rule"] == "support_free")
    return float(message.split(" mm³ unsupported")[0])


class EnclosureFixtureTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        Path(".work").mkdir(exist_ok=True)
        cls.results = {fixture.__name__: build(fixture()) for fixture, _ in FIXTURES}

    def test_each_defect_fails_exactly_the_expected_rules(self):
        for fixture, expected in FIXTURES:
            with self.subTest(fixture=fixture.__name__):
                result = self.results[fixture.__name__]
                self.assertEqual(failing_rules(result), expected, result.report)

    def test_only_the_baseline_may_be_exported(self):
        for fixture, expected in FIXTURES:
            with self.subTest(fixture=fixture.__name__):
                result = self.results[fixture.__name__]
                self.assertEqual(result.export_allowed, not expected)

    def test_baseline_leaves_nothing_unevaluated_but_the_documented_rules(self):
        """meshはexport後にしか判定できず、strength/thermalは未実装である。"""
        result = self.results["baseline"]
        unevaluated = {c["rule"] for c in result.report["checks"] if c["status"] == "not_evaluated"}
        self.assertEqual(unevaluated, {"mesh_manifold", "mesh_volume", "strength", "thermal"})

    def test_baseline_is_a_single_solid_with_an_open_top(self):
        result = self.results["baseline"]
        solids = result.shapes["enclosure"].Solids()
        self.assertEqual(len(solids), 1)
        # 上面が開いているため、体積は外形から内側の空洞を引いた分に一致する。
        cavity = (LENGTH_MM - 2 * WALL_MM) * (WIDTH_MM - 2 * WALL_MM) * (HEIGHT_MM - WALL_MM)
        expected = LENGTH_MM * WIDTH_MM * HEIGHT_MM - cavity
        self.assertAlmostEqual(solids[0].Volume(), expected, delta=expected * 1e-6)

    def test_unsupported_volume_covers_the_overhang_only(self):
        """未支持量は棚の下面1層分であり、棚全体ではない。

        直下に材料があれば支持済みとするため、棚の2層目以降は1層目に載る。
        この筐体を寝かせても解決しない。積層方向を+xにすると、今度は反対側の
        壁全体が宙に浮く。
        """
        self.assertEqual(unsupported_mm3(self.results["baseline"]), 0.0)
        reported = unsupported_mm3(self.results["cantilever"])
        ledge_mm3 = (9.0 - WALL_MM) * (21.0 - 5.0) * (13.0 - 10.0)
        self.assertGreater(reported, 0.0)
        self.assertLess(reported, ledge_mm3 * 0.2)

    def test_raising_min_neck_mm_reports_the_shell_itself(self):
        """min_neck_mmが壁厚を超えると、欠陥のない筐体でも断面不足となる。

        検査が要求値に追随することと、fixtureの2.0が壁3 mmに対して妥当である
        ことを示す。
        """
        demanding = replace(baseline(), policy=replace(POLICY, min_neck_mm=6.0))
        self.assertIn("neck_section", failing_rules(build(demanding)))


if __name__ == "__main__":
    unittest.main()
