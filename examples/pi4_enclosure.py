"""Raspberry Pi 4 Model B用の、底と蓋の2部品からなる筐体。正常版と、複数の欠陥を入れた版を持つ。

基板の外形、取付穴、コネクタの辺上の位置は部品catalogの値 (公式の機械図) を使う。
catalogが持たない値、すなわち基板の高さ、PCBの厚み、コネクタの高さ、プラグの断面、
ネジの寸法、印刷機がbridgeで渡せる長さは、この作例が決めた説明用の値であり、
製品の仕様ではない。

蓋は四隅の柱へネジで締める。基板は底の4本のbossへネジで締め、蓋を外してから外す。
snap fitは持たない。現在のIRは印刷方向をmodel全体で1つだけ持ち、蓋から垂らす梁は
積層方向に沿うため、正常な設計でもsnap_fitの積層方向の検査に落ちるためである。
"""

from dataclasses import replace

from typedsolid import (
    Assembly, Box, Clearance, Feature, Model, Move, Part, PlugSource, Policy, Release, ScrewSpec, Step,
    Sweep, board, boss, connector_opening, hole, screw_fixing,
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
# 例とする印刷機は15 mmまでのbridgeを渡せるものとする。コネクタ開口の上縁はbridgeになる。
POLICY = Policy(voxel_mm=0.5, bridge_max_mm=15.0)
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
    ) + base_extra)
    lid = Part("lid", (
        Feature("panel", Box((0, 0, HEIGHT_MM), (LENGTH_MM, WIDTH_MM, HEIGHT_MM + LID_MM)), "base"),
    ) + tuple(feature for fixing in lid_fixings for feature in fixing.clamp_features) + lid_vents(*vents))
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


# 落ちるruleとtargetの組。testとviewerの撮影が同じ一覧を使う。
PI4_CASES = (
    (pi4_enclosure, set()),
    (pi4_enclosure_defects, {
        ("final_wall_thickness", "lid"),
        ("final_wall_thickness", "base"),
        ("fastener_fit", "lid_back_left/boss_wall"),
        ("keepout_clearance", "pi4/base"),
        ("access_clearance", "pi4_lift/base"),
        ("access_clearance", "ethernet_plug/base"),
        ("connector_fit", "ethernet"),
    }),
)
