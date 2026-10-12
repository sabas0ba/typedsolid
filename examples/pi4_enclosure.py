"""Raspberry Pi 4 Model B用の、底と蓋の2部品からなる筐体。正常版と、複数の欠陥を入れた版を持つ。

基板の外形、取付穴、コネクタの辺上の位置は部品catalogの値 (公式の機械図) を使う。
catalogが持たない値、すなわち基板の高さ、PCBの厚み、コネクタの高さ、プラグの断面、
ネジの寸法、印刷機がbridgeで渡せる長さ、光造形・切削・射出成形の特性は、この作例が決めた説明用の値であり、
製品の仕様ではない。

蓋は四隅の柱へネジで締め、下面のlipを底の壁の内側へ差し込む。基板は底の4本のbossへ
ネジで締め、蓋を外してから外す。

製造案は部品ごとに持つ。蓋は裏返してFDMで造形する案を採用し、上面を造形板に置く案を
比較のために持つ。後者はlipの上の天板が支えを持たず、support_freeに落ちる。底はFDMの案を
採用し、光造形、切削、射出成形の案を比較のために持つ。光造形の案は開いた箱を上向きに
造形するため、resin_suctionに落ちる。切削の案は上面と開口のある2面から工具を下ろし、
どこへも届くが、ネジ柱と壁の間の内角と細い下穴を工具の径で削れずmilling_cornerに落ちる。射出成形の
案は壁の開口が型の開く向きに抜けずmold_undercutに、壁の角と一体になった四隅のネジ柱が
厚くmold_thick_wallに落ち、抜き勾配は評価しない。採用しない案のcheckは出力の可否に用いない。

snap fitは持たない。蓋から垂らす梁は、軸平行の造形姿勢ではsnap_fitの積層方向の検査と
support_freeを同時に満たさないため、任意の回転を扱えるようになるまで保留する。
"""

from dataclasses import replace

from typedsolid import (
    Assembly, Box, Clearance, Fdm, Feature, ManufacturingPlan, Milling, Model, Molding, Move, Orientation, Part,
    PlugSource, Policy, Release, Resin, ScrewSpec, Step, Sweep, board, boss, connector_opening, hole, screw_fixing,
)

PI4 = board("raspberry_pi_4_model_b")
WALL_MM = 2.0
# 壁と基板の隙間。四隅のネジ柱を基板のkeepoutの外に置くため広めに取る。
GAP_MM = 6.0
FLOOR_MM = 2.0
# 基板下面の高さ (bossの上面)。
BOARD_Z_MM = 7.0
# 以下は作例が決めた値。catalogの出典はPCBの厚みと部品の高さを与えない。
PCB_MM = 1.6
BOARD_HEIGHT_MM = 16.0
ORIGIN = (WALL_MM + GAP_MM, WALL_MM + GAP_MM, BOARD_Z_MM)
LENGTH_MM = 2 * (WALL_MM + GAP_MM) + PI4.length_mm
WIDTH_MM = 2 * (WALL_MM + GAP_MM) + PI4.width_mm
# 底の壁の上端。基板の上に2.5 mmの余裕を取る。蓋はこの上に載る。
HEIGHT_MM = BOARD_Z_MM + BOARD_HEIGHT_MM + 3.0
LID_MM = 2.0
# 下穴の周りに格子1つ分を加えても肉厚の要求を満たす径とする。
POST_DIAMETER_MM = 6.0
# 四隅のネジ柱の中心。壁の内側の角に寄せ、基板のkeepout (clearance 0.5 mm) の外に置く。
POST_CENTERS = {
    "front_left": (4.25, 4.25), "front_right": (LENGTH_MM - 4.25, 4.25),
    "back_left": (4.25, WIDTH_MM - 4.25), "back_right": (LENGTH_MM - 4.25, WIDTH_MM - 4.25),
}
POLICY = Policy(voxel_mm=0.5)
# 例とする印刷機は15 mmまでのbridgeを渡せるものとする。コネクタ開口の上縁はbridgeになる。
PRINTER = Fdm(bridge_max_mm=15.0)
PRINTER_SOURCE = "example printer assumed to bridge 15 mm"
PLAN = ManufacturingPlan("fdm", PRINTER, PRINTER_SOURCE)
# 蓋は上面を造形板に置き、lipを上へ積む。
LID_UPSIDE_DOWN = ManufacturingPlan("fdm_upside_down", PRINTER, PRINTER_SOURCE, orientation=Orientation("minus_z"))
LID_UPRIGHT = ManufacturingPlan("fdm_upright", PRINTER, PRINTER_SOURCE)
# 比較用の光造形。値は作例が決めた説明用の値であり、特定の機種と樹脂の値ではない。
RESIN = ManufacturingPlan(
    "resin", Resin(min_wall_mm=1.2, overhang_angle_deg=30.0, bridge_max_mm=15.0, min_drain_mm=3.0),
    "example values for the typedsolid example, not a printer specification",
)
# 比較用の切削と射出成形。値は作例が決めた説明用の値であり、特定の工具や成形条件の値ではない。
# 切削は上面と、コネクタ開口のある2面から工具を下ろす。
MILLING = ManufacturingPlan(
    "milling",
    Milling(
        min_wall_mm=1.2, tool_diameter_mm=3.0, tool_length_mm=30.0,
        additional_setups=(Orientation("minus_y"), Orientation("plus_x")),
    ),
    "example values for the typedsolid example, not a tool specification",
)
MOLDING = ManufacturingPlan(
    "molding", Molding(min_wall_mm=1.2, max_wall_mm=3.0),
    "example values for the typedsolid example, not a molding specification",
)
SOURCE = PlugSource("other", "example value for the typedsolid example, not a connector specification")
# 説明用のネジ寸法。特定の製品の値ではない。
LID_SCREW = ScrewSpec(
    "m2_5x10", "example value", length_mm=10.0, major_mm=2.5, head_mm=4.5, through_mm=2.9, driver_mm=3.0,
    pilot_mm=2.1,
)
BOARD_SCREW = replace(LID_SCREW, name="m2_5x6", length_mm=6.0)
OPEN_LID = Step("open_lid", ("lid",), (Move("plus_z"),))
# コネクタごとの、抜く向き、プラグ中心の高さ (基板下面から)、プラグ断面 (辺に沿う幅、高さ)。
CONNECTORS = {
    "usb_c_power": ("minus_y", 3.2, (8.4, 2.6)),
    "micro_hdmi_0": ("minus_y", 3.2, (6.5, 3.0)),
    "micro_hdmi_1": ("minus_y", 3.2, (6.5, 3.0)),
    "usb_a_0": ("plus_x", 5.6, (12.5, 4.5)),
    "usb_a_1": ("plus_x", 5.6, (12.5, 4.5)),
    "ethernet": ("plus_x", 8.35, (11.7, 8.0)),
}
CLEARANCE_MM = 0.5
# 蓋のlip。底の壁の内面から隙間を取り、四隅のネジ柱の手前で止める。
LIP_GAP_MM = 0.5
LIP_MM = 2.0
LIP_HEIGHT_MM = 3.0
# 柱の外周 (中心4.25 mm、半径3 mm) から0.75 mm離す。
LIP_END_MM = 8.0


def shell() -> tuple[Feature, ...]:
    """床と4枚の壁。"""
    return (
        Feature("floor", Box((0, 0, 0), (LENGTH_MM, WIDTH_MM, FLOOR_MM)), "base"),
        Feature("left", Box((0, 0, 0), (WALL_MM, WIDTH_MM, HEIGHT_MM)), "wall"),
        Feature("right", Box((LENGTH_MM - WALL_MM, 0, 0), (LENGTH_MM, WIDTH_MM, HEIGHT_MM)), "wall"),
        Feature("front", Box((0, 0, 0), (LENGTH_MM, WALL_MM, HEIGHT_MM)), "wall"),
        Feature("back", Box((0, WIDTH_MM - WALL_MM, 0), (LENGTH_MM, WIDTH_MM, HEIGHT_MM)), "wall"),
    )


def openings():
    """catalogのコネクタ位置に開ける開口と、プラグを抜く掃引。"""
    result = []
    for connector_id, (direction, height, plug) in CONNECTORS.items():
        center = PI4.connector_center(connector_id, ORIGIN, BOARD_Z_MM + height)
        edge = PI4.edge_position(direction, ORIGIN)
        if direction == "minus_y":
            plug_span, wall_span = (WALL_MM - 1.0, edge), (-1.0, WALL_MM + 1.0)
        else:
            plug_span, wall_span = (edge, LENGTH_MM - 1.0), (LENGTH_MM - WALL_MM - 1.0, LENGTH_MM + 1.0)
        result.append(connector_opening(
            connector_id, "base", direction, center, plug, plug_span, wall_span, CLEARANCE_MM, SOURCE,
        ))
    return result


def lid_screws(post_diameters: dict[str, float]):
    """蓋を四隅の柱へ締めるネジ。柱は床から壁の上端までの円柱で、ネジ固定のbossを兼ねる。"""
    return [
        screw_fixing(
            f"lid_{name}", LID_SCREW, base="base", clamp=("lid",), direction="minus_z", center=center,
            seat_mm=HEIGHT_MM + LID_MM, joint_mm=HEIGHT_MM, min_engagement_mm=3.0, min_boss_wall_mm=1.2,
            tip_clearance_mm=1.0, boss_diameter_mm=post_diameters.get(name, POST_DIAMETER_MM),
            boss_from_mm=FLOOR_MM - 1.0, release=Release(),
        )
        for name, center in POST_CENTERS.items()
    ]


def board_screws():
    """基板を底のbossへ締めるネジ。基板は部品ではなくkeepoutで表し、蓋を外してから外す。"""
    return [
        screw_fixing(
            f"board_{index}", BOARD_SCREW, base="base", clamp=(), direction="minus_z", center=center,
            seat_mm=BOARD_Z_MM + PCB_MM, joint_mm=BOARD_Z_MM, min_engagement_mm=3.0, min_boss_wall_mm=1.5,
            tip_clearance_mm=1.0, boss_diameter_mm=6.0, boss_from_mm=FLOOR_MM - 1.0,
            release=Release(OPEN_LID.id), clamp_keepouts=("pi4",),
        )
        for index, center in enumerate(PI4.mount_centers(ORIGIN))
    ]


def lid_lip() -> tuple[Feature, ...]:
    """蓋の下面から垂らす4本のlip。底の壁の内側に沿い、蓋の水平方向の位置を決める。"""
    inner = WALL_MM + LIP_GAP_MM
    z = (HEIGHT_MM - LIP_HEIGHT_MM, HEIGHT_MM)
    return (
        Feature("lip_front", Box((LIP_END_MM, inner, z[0]), (LENGTH_MM - LIP_END_MM, inner + LIP_MM, z[1])), "wall"),
        Feature("lip_back", Box((LIP_END_MM, WIDTH_MM - inner - LIP_MM, z[0]), (LENGTH_MM - LIP_END_MM, WIDTH_MM - inner, z[1])), "wall"),
        Feature("lip_left", Box((inner, LIP_END_MM, z[0]), (inner + LIP_MM, WIDTH_MM - LIP_END_MM, z[1])), "wall"),
        Feature("lip_right", Box((LENGTH_MM - inner - LIP_MM, LIP_END_MM, z[0]), (LENGTH_MM - inner, WIDTH_MM - LIP_END_MM, z[1])), "wall"),
    )


def lid_vents(pitch_mm: float, count: int) -> tuple[Feature, ...]:
    """蓋の中央に並ぶ幅3 mmの通気スリット。"""
    start = LENGTH_MM / 2.0 - pitch_mm * (count - 1) / 2.0 - 1.5
    return tuple(
        Feature(
            f"vent_{index}",
            Box((start + index * pitch_mm, 20.0, HEIGHT_MM - 1.0), (start + index * pitch_mm + 3.0, WIDTH_MM - 20.0, HEIGHT_MM + LID_MM + 1.0)),
            operation="cut",
        )
        for index in range(count)
    )


def model(
    *, post_diameters: dict[str, float] | None = None, vents: tuple[float, int] = (7.0, 5),
    base_extra: tuple[Feature, ...] = (), shift_opening: dict[str, float] | None = None,
) -> Model:
    shift_opening = shift_opening or {}
    connector_parts = openings()
    lid_fixings = lid_screws(post_diameters or {})
    board_fixings = board_screws()
    opening_features = []
    for opening in connector_parts:
        feature = opening.feature
        dz = shift_opening.get(opening.connector.id, 0.0)
        if dz:
            low, high = feature.shape.min, feature.shape.max
            feature = replace(feature, shape=Box((low[0], low[1], low[2] + dz), (high[0], high[1], high[2] + dz)))
        opening_features.append(feature)
    base = Part("base", shell() + tuple(opening_features) + tuple(
        feature for fixing in lid_fixings + board_fixings for feature in fixing.base_features
    ) + base_extra, manufacturing=(PLAN, RESIN, MILLING, MOLDING), adopted=PLAN.id)
    lid = Part("lid", (
        Feature("panel", Box((0, 0, HEIGHT_MM), (LENGTH_MM, WIDTH_MM, HEIGHT_MM + LID_MM)), "base"),
    ) + lid_lip() + tuple(feature for fixing in lid_fixings for feature in fixing.clamp_features) + lid_vents(*vents),
        manufacturing=(LID_UPSIDE_DOWN, LID_UPRIGHT), adopted=LID_UPSIDE_DOWN.id)
    # 基板は底のbossの上面に接するため、下面のclearanceだけを0とする。
    keepout = PI4.keepout(
        "pi4", ORIGIN, BOARD_HEIGHT_MM, Clearance(default=0.5, minus_z=0.0), attached_to="base",
    )
    # 蓋を外した後、基板を上へ取り出せること。
    lift_board = Sweep("pi4_lift", "plus_z", keepout="pi4", after_step=OPEN_LID.id)
    return Model(
        parts=(base, lid),
        keepouts=(keepout,),
        policy=POLICY,
        assembly=Assembly((OPEN_LID,)),
        sweeps=(lift_board,) + tuple(o.sweep for o in connector_parts) + tuple(
            fixing.sweep for fixing in lid_fixings + board_fixings
        ),
        fasteners=tuple(fixing.fastener for fixing in lid_fixings + board_fixings),
        connectors=tuple(o.connector for o in connector_parts),
    )


def pi4_enclosure() -> Model:
    """欠陥のない筐体。すべてのruleを通る。"""
    return model()


def pi4_enclosure_defects() -> Model:
    """5種類の欠陥を同時に入れた筐体。2部品にまたがり、6つのruleの7つのcheckが落ちる。
    採用しない製造案では、正常版と同じcheckに加えて各案の薄肉が落ちる。

    - 蓋の通気スリットの間隔を詰め、桟を1 mmにする (蓋のfinal_wall_thickness)。
    - 床の上面を基板の下で削り、残りを0.8 mmにする (底のfinal_wall_thickness)。
    - 奥左のネジ柱を細くし、下穴の周りの肉を欠く (fastener_fit)。
    - 基板のboss 0の上に1.5 mmのshimを載せ、基板の領域へ食い込ませる (keepout_clearance)。
      shimは基板を上へ取り出す経路も塞ぐ (access_clearance)。
    - Ethernetの開口を3 mm下へずらす。プラグが壁に当たり (access_clearance)、開口がプラグを
      含まない (connector_fit)。
    """
    # 下面から削ると造形板が蓋になり、閉空洞と支持のない天井になるため、上面から削る。
    pocket = Feature("floor_pocket", Box((40.0, 30.0, 0.8), (52.0, 42.0, FLOOR_MM + 1.0)), operation="cut")
    shim_center = PI4.mount_centers(ORIGIN)[0]
    # bossと同じ径の座金。角を持つ形はbossの外へ張り出し、支持のない箇所になる。
    shim = boss("pad_0_shim", "z", shim_center, 6.0, (BOARD_Z_MM, BOARD_Z_MM + 1.5))
    # shimにもネジを通す穴を開ける。穴が無いとshimがネジの下穴を塞ぎ、閉空洞になる。
    # 穴はbossの上面から始め、bossの下穴を広げない。
    shim_hole = hole("pad_0_shim_hole", "z", shim_center, BOARD_SCREW.through_mm, (BOARD_Z_MM, BOARD_Z_MM + 2.0))
    return model(
        post_diameters={"back_left": 3.6}, vents=(4.0, 8), base_extra=(pocket, shim, shim_hole),
        shift_opening={"ethernet": -3.0},
    )


# 落ちるcheck。採用した製造案と製造案に依らないcheckは(rule, target)、採用しない製造案の
# checkは(rule, target, plan)で表す。testとviewerの撮影が同じ一覧を使う。
ALTERNATIVES = {
    ("resin_suction", "base", "resin"),
    ("milling_corner", "base", "milling"),
    ("mold_undercut", "base", "molding"),
    ("mold_thick_wall", "base", "molding"),
    ("support_free", "lid", "fdm_upright"),
}
PI4_CASES = (
    (pi4_enclosure, set(), ALTERNATIVES),
    (pi4_enclosure_defects, {
        ("final_wall_thickness", "lid"),
        ("final_wall_thickness", "base"),
        ("fastener_fit", "lid_back_left/boss_wall"),
        ("keepout_clearance", "pi4/base"),
        ("access_clearance", "pi4_lift/base"),
        ("access_clearance", "ethernet_plug/base"),
        ("connector_fit", "ethernet"),
    }, ALTERNATIVES | {
        ("final_wall_thickness", "base", "resin"),
        ("final_wall_thickness", "base", "milling"),
        ("final_wall_thickness", "base", "molding"),
        ("final_wall_thickness", "lid", "fdm_upright"),
    }),
)
