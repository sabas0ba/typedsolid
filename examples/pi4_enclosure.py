"""Raspberry Pi 4 Model B用の、底と蓋の2部品からなる筐体。正常版と、複数の欠陥を入れた版を持つ。

基板の外形、取付穴、コネクタの辺上の位置は部品catalogの値 (公式の機械図) を使う。
catalogが持たない値、すなわち基板の高さ、PCBの厚み、コネクタの高さ、プラグの断面、
ネジの寸法、印刷機の特性、材料の許容ひずみ、光造形・切削・射出成形の特性は、この作例が
決めた説明用の値であり、製品の仕様ではない。

蓋は四隅の柱へネジで締め、下面のlipを底の壁の内側へ差し込む。さらに背面の2本のsnap fitが
蓋から垂れ、フックが底の背面の窓に掛かる。基板は底の4本のbossへネジで締め、蓋を外してから
外す。

製造案は部品ごとに持つ。蓋は裏返して前面の上の辺を造形板に寄せ、50°傾けてFDMで造形する案を
採用する。snap fitの梁は積層方向から50°離れ、snap_fitの積層方向の検査を通る。比較のため、
裏返しただけの案と、上面を造形板に置く案を持つ。前者は梁が積層方向に沿いsnap_fitの
積層方向の検査に落ち、後者はさらにlipの上の天板が支えを持たずsupport_freeに落ちる。底はFDMの案を
採用し、光造形、切削、射出成形の案を比較のために持つ。光造形の案は開いた箱を上向きに
造形するため、resin_suctionに落ちる。切削の案は上面と開口のある2面から工具を下ろし、
どこへも届くが、ネジ柱と壁の間の内角と細い下穴を工具の径で削れずmilling_cornerに落ちる。射出成形の
案は壁の開口が型の開く向きに抜けずmold_undercutに、壁の角と一体になった四隅のネジ柱が
厚くmold_thick_wallに落ち、抜き勾配は評価しない。採用しない案のcheckは出力の可否に用いない。

傾けた姿勢では、蓋の縦の面が積層方向に対して50°張り出す。作例の印刷機は55°までの
張り出しを支持なしで積めるものとする。

底は、基板を締めるbossから受ける熱の定常伝導を解き、最高温度を判定する。強度の解析ケースは
持たない。蓋を押す荷重は、0.5 mmの格子では厚さ2 mmの天板が4倍の格子で1 cellとなり、通気
スリットの端とlipの付け根の鋭い内角が応力の特異点となるため、変位も応力も収束しなかった。
"""

from dataclasses import replace

from typedsolid import (
    Analyses, Assembly, AtKeepout, Box, Clearance, Convection, Elastic, Fdm, Feature, HeatInput, ManufacturingPlan,
    Material, Milling, Model, Molding, Move, Orientation, Part, PlugSource, Policy, Release, Resin, ScrewSpec, Step,
    Surface, Sweep, ThermalCase, board, boss, connector_opening, hole, screw_fixing, snap_fit,
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
# 例とする印刷機は15 mmまでのbridgeを渡せ、鉛直から55°までの張り出しを支持なしで積めるものと
# する。コネクタ開口の上縁はbridgeになる。
PRINTER = Fdm(overhang_angle_deg=55.0, bridge_max_mm=15.0)
PRINTER_SOURCE = "example printer assumed to bridge 15 mm and print 55 degree overhangs"
PLAN = ManufacturingPlan("fdm", PRINTER, PRINTER_SOURCE)
# 蓋は裏返し、前面の上の辺を造形板に寄せて50°傾ける。積層方向は組立座標で後方の下へ向く。
# 前方へ傾けるとフックの外側の上の稜線が最初に積む位置となり、宙に浮く。
LID_TILTED = ManufacturingPlan(
    "fdm_tilted", PRINTER, PRINTER_SOURCE, orientation=Orientation("minus_z", turn_deg=180.0, tilt_deg=50.0),
)
# 比較用。裏返しただけの案と、上面を造形板に置く案。
LID_UPSIDE_DOWN = ManufacturingPlan("fdm_upside_down", PRINTER, PRINTER_SOURCE, orientation=Orientation("minus_z"))
LID_UPRIGHT = ManufacturingPlan("fdm_upright", PRINTER, PRINTER_SOURCE)
# 底と蓋の材料。値は説明用の値であり、特定の樹脂の値ではない。
MATERIAL = Material(
    "pla_example", "example PLA", "example value for the typedsolid example, not a material specification",
    allowable_strain=0.02,
    elastic=Elastic(youngs_modulus_mpa=3500.0, poisson_ratio=0.35),
    allowable_stress_mpa=40.0,
    interlayer_allowable_stress_mpa=15.0,
    thermal_conductivity_w_per_mk=0.13,
)
# 比較用の光造形。値は作例が決めた説明用の値であり、特定の機種と樹脂の値ではない。
RESIN = ManufacturingPlan(
    "resin", Resin(min_wall_mm=1.2, overhang_angle_deg=30.0, bridge_max_mm=15.0, min_drain_mm=3.0),
    "example values for the typedsolid example, not a printer specification",
)
# 比較用の切削と射出成形。値は作例が決めた説明用の値であり、特定の工具や成形条件の値ではない。
# 切削は上面と、コネクタ開口とsnap fitの窓のある3面から工具を下ろす。
MILLING = ManufacturingPlan(
    "milling",
    Milling(
        min_wall_mm=1.2, tool_diameter_mm=3.0, tool_length_mm=30.0,
        additional_setups=(Orientation("minus_y"), Orientation("plus_x"), Orientation("plus_y")),
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
# 背面のsnap fit。梁は背面のlipの位置で蓋の下面から垂れ、先端のフックが底の背面の窓に掛かる。
# 梁の厚みは最小肉厚に格子1つ分を加えた値以上とする。外すときは梁を内側 (-y) へたわませる。
SNAP_CENTERS_X = (30.0, LENGTH_MM - 30.0)
SNAP_WIDTH_MM = 8.0
SNAP_THICKNESS_MM = 2.0
SNAP_LENGTH_MM = 18.0
SNAP_HOOK_MM = 1.5
SNAP_HOOK_LENGTH_MM = 2.0
# 梁とlip、フックと窓の隙間。
SNAP_GAP_MM = 1.0
WINDOW_CLEARANCE_MM = 0.5


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


def snap_fits():
    """背面の2本のsnap fit。梁は背面のlipと同じ面に並び、lipはその位置で途切れる。"""
    beam_y = WIDTH_MM - WALL_MM - LIP_GAP_MM - SNAP_THICKNESS_MM / 2.0
    return [
        snap_fit(
            f"clip_{index}", part="lid", mate="base", step=OPEN_LID.id, root=(x, beam_y, HEIGHT_MM),
            length_direction="minus_z", deflection="minus_y", length_mm=SNAP_LENGTH_MM,
            thickness_mm=SNAP_THICKNESS_MM, width_mm=SNAP_WIDTH_MM, hook_mm=SNAP_HOOK_MM,
            hook_length_mm=SNAP_HOOK_LENGTH_MM,
        )
        for index, x in enumerate(SNAP_CENTERS_X)
    ]


def snap_windows() -> tuple[Feature, ...]:
    """底の背面の、フックが掛かる窓。フックの周りに隙間を取る。"""
    tip = HEIGHT_MM - SNAP_LENGTH_MM
    return tuple(
        Feature(
            f"clip_{index}_window",
            Box(
                (x - SNAP_WIDTH_MM / 2.0 - WINDOW_CLEARANCE_MM, WIDTH_MM - WALL_MM - 1.0, tip - WINDOW_CLEARANCE_MM),
                (x + SNAP_WIDTH_MM / 2.0 + WINDOW_CLEARANCE_MM, WIDTH_MM + 1.0,
                 tip + SNAP_HOOK_LENGTH_MM + WINDOW_CLEARANCE_MM),
            ),
            operation="cut",
        )
        for index, x in enumerate(SNAP_CENTERS_X)
    )


def lid_lip() -> tuple[Feature, ...]:
    """蓋の下面から垂らすlip。底の壁の内側に沿い、蓋の水平方向の位置を決める。背面のlipは
    snap fitの梁の位置で区切る。"""
    inner = WALL_MM + LIP_GAP_MM
    z = (HEIGHT_MM - LIP_HEIGHT_MM, HEIGHT_MM)
    back = (WIDTH_MM - inner - LIP_MM, WIDTH_MM - inner)
    edges = [LIP_END_MM]
    for x in SNAP_CENTERS_X:
        edges += [x - SNAP_WIDTH_MM / 2.0 - SNAP_GAP_MM, x + SNAP_WIDTH_MM / 2.0 + SNAP_GAP_MM]
    edges.append(LENGTH_MM - LIP_END_MM)
    back_segments = tuple(
        Feature(f"lip_back_{index}", Box((edges[2 * index], back[0], z[0]), (edges[2 * index + 1], back[1], z[1])), "wall")
        for index in range(len(edges) // 2)
    )
    return back_segments + (
        Feature("lip_front", Box((LIP_END_MM, inner, z[0]), (LENGTH_MM - LIP_END_MM, inner + LIP_MM, z[1])), "wall"),
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


def analyses() -> Analyses:
    """基板から底へ伝わる熱。値は説明用の値であり、製品の条件ではない。

    底は基板を締めるbossの上面から、ネジと座面を伝わる0.05 Wを受け、外面から室温25 °Cへ、
    内面から筐体内の空気40 °Cへ放熱する。筐体内の空気の温度は利用者が与える値であり、
    解析は求めない。例とするPLAの軟化を避けるため、底の最高温度を55 °C以下とする。
    """
    # 外面は各面の平面上の薄い箱で選ぶ。1つの面は先に書いた対流にだけ属するため、
    # 残りの内面をSurfaceで選ぶ。
    outside = (
        Box((-0.1, -1.0, -1.0), (0.1, WIDTH_MM + 1.0, HEIGHT_MM + 1.0)),
        Box((LENGTH_MM - 0.1, -1.0, -1.0), (LENGTH_MM + 0.1, WIDTH_MM + 1.0, HEIGHT_MM + 1.0)),
        Box((-1.0, -0.1, -1.0), (LENGTH_MM + 1.0, 0.1, HEIGHT_MM + 1.0)),
        Box((-1.0, WIDTH_MM - 0.1, -1.0), (LENGTH_MM + 1.0, WIDTH_MM + 0.1, HEIGHT_MM + 1.0)),
        Box((-1.0, -1.0, -0.1), (LENGTH_MM + 1.0, WIDTH_MM + 1.0, 0.1)),
    )
    heat = ThermalCase(
        "board_heat", "base", max_temperature_c=55.0,
        heat_inputs=(HeatInput(AtKeepout("pi4"), 0.05),),
        convection=tuple(Convection(region, 10.0, 25.0) for region in outside) + (Convection(Surface(), 5.0, 40.0),),
    )
    return Analyses(thermal=(heat,))


def model(
    *, post_diameters: dict[str, float] | None = None, vents: tuple[float, int] = (7.0, 5),
    base_extra: tuple[Feature, ...] = (), shift_opening: dict[str, float] | None = None,
) -> Model:
    shift_opening = shift_opening or {}
    connector_parts = openings()
    lid_fixings = lid_screws(post_diameters or {})
    board_fixings = board_screws()
    clips = snap_fits()
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
    ) + snap_windows() + base_extra, material=MATERIAL.id, manufacturing=(PLAN, RESIN, MILLING, MOLDING), adopted=PLAN.id)
    lid = Part("lid", (
        Feature("panel", Box((0, 0, HEIGHT_MM), (LENGTH_MM, WIDTH_MM, HEIGHT_MM + LID_MM)), "base"),
    ) + lid_lip() + tuple(feature for clip in clips for feature in clip.features) + tuple(
        feature for fixing in lid_fixings for feature in fixing.clamp_features
    ) + lid_vents(*vents), material=MATERIAL.id,
        manufacturing=(LID_TILTED, LID_UPSIDE_DOWN, LID_UPRIGHT), adopted=LID_TILTED.id)
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
        materials=(MATERIAL,),
        snap_fits=tuple(clip.snap for clip in clips),
        connectors=tuple(o.connector for o in connector_parts),
        analyses=analyses(),
    )


def pi4_enclosure() -> Model:
    """欠陥のない筐体。すべてのruleを通る。"""
    return model()


def pi4_enclosure_defects() -> Model:
    """5種類の欠陥を同時に入れた筐体。2部品にまたがり、7つのruleの8つのcheckが落ちる。
    採用しない製造案では、正常版と同じcheckに加えて各案の薄肉が落ちる。

    - 蓋の通気スリットの間隔を詰め、桟を1 mmにする (蓋のfinal_wall_thickness)。
    - 床の上面を基板の下で削り、残りを0.8 mmにする (底のfinal_wall_thickness)。
    - 奥左のネジ柱を細くし、下穴の周りの肉を欠く (fastener_fit)。
    - 基板のboss 0の上に1.5 mmのshimを載せ、基板の領域へ食い込ませる (keepout_clearance)。
      shimは基板を上へ取り出す経路も塞ぐ (access_clearance)。基板の領域に入ったshimは基板から
      熱を受け、床から離れた細い円柱であるため底の最高温度が上限を超える (thermal)。
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
    # 裏返しただけでは、梁が積層方向に沿い、フックの下面が宙に浮く。
    ("snap_fit", "clip_0/layer", "fdm_upside_down"),
    ("snap_fit", "clip_1/layer", "fdm_upside_down"),
    ("support_free", "lid", "fdm_upside_down"),
    ("snap_fit", "clip_0/layer", "fdm_upright"),
    ("snap_fit", "clip_1/layer", "fdm_upright"),
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
        ("thermal", "board_heat/temperature"),
    }, ALTERNATIVES | {
        ("final_wall_thickness", "base", "resin"),
        ("final_wall_thickness", "base", "milling"),
        ("final_wall_thickness", "base", "molding"),
        ("final_wall_thickness", "lid", "fdm_upside_down"),
        ("final_wall_thickness", "lid", "fdm_upright"),
    }),
)
