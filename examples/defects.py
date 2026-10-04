"""開口を持つ筐体に既知の欠陥を1つずつ入れたモデル。

`tests/test_enclosure_fixtures.py`が判定を、`scripts/render-figures.py`が図を扱う。
CadQueryに依存しないため、Rust coreの検査と図だけを使う環境でも読み込める。
"""

from typedsolid import Box, Clearance, Feature, Keepout, Model, Part, Policy

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


# 欠陥を持つfixture。図の生成とtestが同じ一覧を使う。
DEFECTS = (thin_wall, narrow_neck, severed_corner, cantilever, sealed_void)
