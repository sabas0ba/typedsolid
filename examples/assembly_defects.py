"""部品間の関係に既知の欠陥を1つずつ入れたモデル。OCCTの境界演算で判定するruleを対象とする。

`tests/test_projection.py`が判定と検出箇所を、`scripts/render-projections.py`が投影図を扱う。
最終形状のruleを対象とする欠陥は`examples/defects.py`が持つ。
"""

from typedsolid import Assembly, Box, Clearance, Feature, Keepout, Model, Move, Part, Policy, Step, Sweep

# 部品間の判定だけを見る。voxel評価は判定に関わらないため、格子を粗くして時間を抑える。
POLICY = Policy(voxel_mm=0.5)
LENGTH_MM, WIDTH_MM, HEIGHT_MM, WALL_MM = 40.0, 30.0, 15.0, 2.0
# 基板の確保領域。下面は床に接する。
PCB = Keepout("pcb", Box((8.0, 8.0, WALL_MM), (32.0, 22.0, 6.0)), Clearance(default=0.5, minus_z=0.0))


def tray(*extra: Feature) -> Part:
    """上面が開いた箱。"""
    return Part("tray", (
        Feature("floor", Box((0, 0, 0), (LENGTH_MM, WIDTH_MM, WALL_MM))),
        Feature("left", Box((0, 0, 0), (WALL_MM, WIDTH_MM, HEIGHT_MM))),
        Feature("right", Box((LENGTH_MM - WALL_MM, 0, 0), (LENGTH_MM, WIDTH_MM, HEIGHT_MM))),
        Feature("front", Box((0, 0, 0), (LENGTH_MM, WALL_MM, HEIGHT_MM))),
        Feature("back", Box((0, WIDTH_MM - WALL_MM, 0), (LENGTH_MM, WIDTH_MM, HEIGHT_MM))),
    ) + extra)


def lid(bottom: float = HEIGHT_MM) -> Part:
    """箱の上に載る板。bottomが壁の上端より低いと壁に食い込む。"""
    return Part("lid", (Feature("panel", Box((0, 0, bottom), (LENGTH_MM, WIDTH_MM, bottom + 2.0))),))


def lid_overlap() -> Model:
    """蓋の板が壁の上端に1 mm食い込む。"""
    return Model(parts=(tray(), lid(bottom=HEIGHT_MM - 1.0)), policy=POLICY)


def blocked_cable() -> Model:
    """箱の内側から右壁を通して+xへ抜くケーブルの掃引。右壁に開口がない。"""
    cable = Sweep("usb_cable", "plus_x", Box((30.0, 10.0, 5.0), (36.0, 20.0, 10.0)))
    return Model(parts=(tray(),), sweeps=(cable,), policy=POLICY)


def post_in_keepout() -> Model:
    """床から立つ角柱が基板の確保領域に入る。

    円柱のbossは粗い格子では縁が薄肉と判定されるため、角柱とする。
    """
    post = Feature("post", Box((10.0, 13.0, WALL_MM), (15.0, 18.0, 10.0)), "mount")
    return Model(parts=(tray(post),), keepouts=(PCB,), policy=POLICY)


def sliding_lid() -> Model:
    """箱の内側に収まる板を+xへ引き抜く分解step。右壁が経路を塞ぐ。"""
    inner = Part("inner_lid", (
        Feature("panel", Box((WALL_MM, WALL_MM, 11.0), (LENGTH_MM - WALL_MM, WIDTH_MM - WALL_MM, 13.0))),
    ))
    step = Step("slide_out", ("inner_lid",), (Move("plus_x"),))
    return Model(parts=(tray(), inner), assembly=Assembly((step,)), policy=POLICY)


# 欠陥と、落ちるruleとtargetの組。testと図の生成が同じ一覧を使う。
ASSEMBLY_DEFECTS = (
    (lid_overlap, {("part_interference", "tray/lid")}),
    (blocked_cable, {("access_clearance", "usb_cable/tray")}),
    (post_in_keepout, {("keepout_clearance", "pcb/tray")}),
    (sliding_lid, {("disassembly_path", "slide_out/inner_lid/0/tray")}),
)
