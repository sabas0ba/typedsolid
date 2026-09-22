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

from typedsolid import Box, Clearance, Feature, Keepout, Model, Part, Policy
from typedsolid.cadquery import build

WALL_MM = 3.0
LENGTH_MM, WIDTH_MM, HEIGHT_MM = 36.0, 26.0, 14.0

# min_neck_mmを既定より大きく取り、断面不足を肉厚不足と切り離して観測する。
# 両者が同じ値だと、断面が足りない箇所は必ず肉厚も足りず、ruleを区別できない。
# 壁3 mmはerosion半径1.1 mmに耐えるため、欠陥のない筐体はneck_sectionを通る。
POLICY = Policy(min_neck_mm=2.0)

# 内部に確保する基板領域。上面が開いていることをaccessで検査する。
KEEPOUT = Keepout(
    "payload",
    Box((11.0, 7.0, WALL_MM), (25.0, 19.0, 7.0)),
    Clearance(default=0.5, minus_z=0.0),
    ("plus_z",),
)


def shell() -> tuple[Feature, ...]:
    """上面が開いた箱。床から立ち上がる4枚の壁を持つ。"""
    return (
        Feature("floor", Box((0, 0, 0), (LENGTH_MM, WIDTH_MM, WALL_MM)), "base"),
        Feature("left", Box((0, 0, 0), (WALL_MM, WIDTH_MM, HEIGHT_MM)), "wall"),
        Feature("right", Box((LENGTH_MM - WALL_MM, 0, 0), (LENGTH_MM, WIDTH_MM, HEIGHT_MM)), "wall"),
        Feature("front", Box((0, 0, 0), (LENGTH_MM, WALL_MM, HEIGHT_MM)), "wall"),
        Feature("back", Box((0, WIDTH_MM - WALL_MM, 0), (LENGTH_MM, WIDTH_MM, HEIGHT_MM)), "wall"),
    )


def enclosure(*extra: Feature) -> Model:
    """基準の筐体に、欠陥を表すfeatureを加えたモデルを返す。"""
    return Model(
        parts=(Part("enclosure", shell() + extra),),
        keepouts=(KEEPOUT,),
        policy=POLICY,
    )


def baseline() -> Model:
    return enclosure()


def thin_wall() -> Model:
    """右壁の内側を削り、床より上を残り1 mmとする。

    cutは最小feature寸法ruleの対象外なので、薄さは最終形状の肉厚だけが捉える。
    床と前後の壁は削らない。削ると薄壁が本体から分離し、single_solidも落ちる。
    """
    return enclosure(
        Feature(
            "thinning",
            Box((33.0, WALL_MM, WALL_MM), (35.0, WIDTH_MM - WALL_MM, HEIGHT_MM + 1.0)),
            operation="cut",
        ),
    )


def _slit(gap: tuple[float, float] | None) -> tuple[Feature, ...]:
    """x=27..28で床と前後の壁を断つスリット。gapを与えるとその範囲だけ材料を残す。"""
    top = HEIGHT_MM + 1.0
    if gap is None:
        return (
            Feature("slit", Box((27.0, -1.0, -1.0), (28.0, WIDTH_MM + 1.0, top)), operation="cut"),
        )
    front, back = gap
    return (
        Feature("slit_front", Box((27.0, -1.0, -1.0), (28.0, front, top)), operation="cut"),
        Feature("slit_back", Box((27.0, back, -1.0), (28.0, WIDTH_MM + 1.0, top)), operation="cut"),
    )


def narrow_neck() -> Model:
    """床と前後の壁を断つスリットを2本入れ、幅1.6 mmの桟だけで左右を繋ぐ。

    桟の肉厚1.6 mmはmin_wall_mm 1.2に格子1つ分を足した要求を満たすため、
    最終肉厚は通る。min_neck_mm 2.0のerosionでは桟が消え、左右が別成分として
    残るため断面不足だけが落ちる。

    床の上に薄いslabを置く形では首にならない。slabは床と融合して1つの厚い
    塊になり、erosionを通過する。
    """
    return enclosure(*_slit(gap=(11.2, 12.8)))


def severed_corner() -> Model:
    """narrow_neckと同じスリットを桟を残さずに通し、右側を切り離す。

    切り離された側も造形板に接するためsupport_freeは通る。分離した2つの塊は
    erosion後も2つ残るため、single_solidとneck_sectionが同時に落ちる。
    形状が2つに分かれている以上、断面が保たれないのは不可分な帰結である。
    """
    return enclosure(*_slit(gap=None))


def cantilever() -> Model:
    """左壁から内側へ6 mm張り出す棚。下に支えが無く、両端も支持されない。"""
    return enclosure(
        Feature("ledge", Box((WALL_MM, 5.0, 10.0), (9.0, 21.0, 13.0)), "rib"),
    )


def sealed_void() -> Model:
    """床に載る塊の内部に、外へ通じない空洞を開ける。

    空洞の天井は2 mm幅でbridge_max_mm 5.0以内のため、support_freeは通る。
    """
    return enclosure(
        Feature("block", Box((26.0, 15.0, WALL_MM), (32.0, 21.0, 11.0))),
        Feature("void", Box((28.0, 17.0, 5.0), (30.0, 19.0, 9.0)), operation="cut"),
    )


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
