"""ネジ固定の形状とIRを一括して作るhelper。

ネジとインサートの寸法は利用者が規格表や実測から与える。helperはその値から
baseのbossと下穴、clampの貫通穴、ドライバの掃引、IRのFastenerを作る。
寸法の整合はRust coreの検証と、backendのfastener_fit検査が確かめる。
"""

from dataclasses import dataclass

from .model import Cylinder, Direction, Fastener, Feature, Insert, Release, Screw, SelfTapping, Sweep, Vec2

# 穴のcutを境目と座面から外側へ延ばす長さ。単位はmm。端面をcut対象の面と一致させると、
# 残るかどうかが演算誤差で決まる薄い膜が生じうるため、外側へ突き抜けさせる。
CUT_OVERRUN_MM = 0.5
# ドライバの掃引の起点とする円板の厚み。単位はmm。座面から外向きに置く。
DRIVER_DISK_MM = 0.1


@dataclass(frozen=True)
class ScrewSpec:
    """ネジの寸法。sourceは値の出典。

    through_mmはclampに開ける貫通穴の径、driver_mmは工具の軸径、
    pilot_mmはセルフタップで締める場合の下穴の径である。
    """

    name: str
    source: str
    length_mm: float
    major_mm: float
    head_mm: float
    through_mm: float
    driver_mm: float
    pilot_mm: float | None = None


@dataclass(frozen=True)
class InsertSpec:
    """熱圧入インサートの寸法。hole_mmは圧入前の下穴の径、sourceは値の出典。"""

    name: str
    source: str
    hole_mm: float
    length_mm: float


@dataclass(frozen=True)
class ScrewFixing:
    """base_featuresはbaseの部品へ、clamp_featuresはclampの各部品へ加える。"""

    base_features: tuple[Feature, ...]
    clamp_features: tuple[Feature, ...]
    sweep: Sweep
    fastener: Fastener


def _opposite(direction: Direction) -> Direction:
    sign, axis = direction.split("_")
    return f"{'minus' if sign == 'plus' else 'plus'}_{axis}"  # type: ignore[return-value]


def _span(a: float, b: float) -> Vec2:
    return (min(a, b), max(a, b))


def screw_fixing(
    id: str,
    screw: ScrewSpec,
    *,
    base: str,
    clamp: tuple[str, ...],
    direction: Direction,
    center: Vec2,
    seat_mm: float,
    joint_mm: float,
    min_engagement_mm: float,
    min_boss_wall_mm: float,
    tip_clearance_mm: float,
    boss_diameter_mm: float | None = None,
    boss_from_mm: float | None = None,
    insert: InsertSpec | None = None,
    after_step: str | None = None,
    release: Release | None = None,
    clamp_keepouts: tuple[str, ...] = (),
) -> ScrewFixing:
    """clampをbaseへ締めるネジ1本分の形状とIR。

    directionは締め込む向き、seat_mmは頭が当たる面、joint_mmはclampとbaseの境目の
    軸方向の座標である。bossはboss_from_mmからjoint_mmまでの円柱で、bossを使わず
    既存の肉に穴を開ける場合は省く。下穴はネジの先端からtip_clearance_mmだけ深くする。
    ドライバの掃引は、after_stepを終えた状態で座面から締め込みと逆向きに外まで抜く。
    after_stepを省きreleaseを与えると、ネジを外す状態 (release.after_step) で掃引する。
    clamp_keepoutsは、clampの部品の代わりや追加としてネジで締めるkeepoutのid。
    """
    if tip_clearance_mm < 0.0:
        raise ValueError(f"{id}: tip_clearance_mm must not be negative")
    if (boss_diameter_mm is None) != (boss_from_mm is None):
        raise ValueError(f"{id}: boss_diameter_mm and boss_from_mm are given together")
    sign = 1.0 if direction.startswith("plus") else -1.0
    axis = direction[-1]
    tip = seat_mm + sign * screw.length_mm
    depth = sign * (tip - joint_mm) + tip_clearance_mm
    mouth = joint_mm - sign * CUT_OVERRUN_MM

    base_features: tuple[Feature, ...] = ()
    if boss_diameter_mm is not None and boss_from_mm is not None:
        if sign * (boss_from_mm - joint_mm) <= 0.0:
            raise ValueError(f"{id}: boss_from_mm must lie beyond joint_mm along the direction")
        base_features += (
            Feature(f"{id}_boss", Cylinder(axis, center, boss_diameter_mm / 2.0, _span(boss_from_mm, joint_mm)), "mount"),
        )
    anchor: SelfTapping | Insert
    if insert is None:
        if screw.pilot_mm is None:
            raise ValueError(f"{id}: self-tapping requires ScrewSpec.pilot_mm")
        anchor = SelfTapping(screw.pilot_mm)
        bore = screw.pilot_mm
    else:
        anchor = Insert(insert.hole_mm, insert.length_mm)
        bore = insert.hole_mm
        depth = max(depth, insert.length_mm)
    if depth <= 0.0:
        raise ValueError(f"{id}: the screw does not reach past joint_mm")
    base_features += (
        Feature(f"{id}_bore", Cylinder(axis, center, bore / 2.0, _span(mouth, joint_mm + sign * depth)), "mount", "cut"),
    )

    clamp_features: tuple[Feature, ...] = ()
    if clamp:
        span = _span(seat_mm - sign * CUT_OVERRUN_MM, joint_mm + sign * CUT_OVERRUN_MM)
        clamp_features = (Feature(f"{id}_through", Cylinder(axis, center, screw.through_mm / 2.0, span), "mount", "cut"),)

    radius = max(screw.head_mm, screw.driver_mm) / 2.0
    driver = Sweep(
        f"{id}_driver", _opposite(direction),
        shape=Cylinder(axis, center, radius, _span(seat_mm, seat_mm - sign * DRIVER_DISK_MM)),
        after_step=after_step if after_step is not None or release is None else release.after_step,
    )
    fastener = Fastener(
        id=id, base=base, clamp=clamp, direction=direction, center=center,
        seat_mm=seat_mm, joint_mm=joint_mm,
        screw=Screw(screw.length_mm, screw.major_mm, screw.head_mm),
        through_mm=screw.through_mm, anchor=anchor,
        min_engagement_mm=min_engagement_mm, min_boss_wall_mm=min_boss_wall_mm,
        release=release, clamp_keepouts=clamp_keepouts,
    )
    return ScrewFixing(base_features, clamp_features, driver, fastener)
