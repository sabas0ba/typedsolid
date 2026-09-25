"""snap fitの梁・フックの形状とIRを一括して作るhelper。

梁は矩形断面の片持ち梁とし、フックは先端の、外すときにたわむ向きと反対の側へ
張り出す。ひずみと積層方向はRust coreが、梁の実形状、たわむ空間、保持、
外れる経路はbackendが検査する。
"""

from dataclasses import dataclass

from .model import Box, Direction, Feature, SnapFit, Vec3

AXES = "xyz"


@dataclass(frozen=True)
class SnapFitFixing:
    """featuresはpartへ加える梁とフック、snapはModel.snap_fitsへ渡すIR。"""

    features: tuple[Feature, ...]
    snap: SnapFit


def snap_fit(
    id: str,
    *,
    part: str,
    mate: str,
    step: str,
    root: Vec3,
    length_direction: Direction,
    deflection: Direction,
    length_mm: float,
    thickness_mm: float,
    width_mm: float,
    hook_mm: float,
    hook_length_mm: float,
    deflection_mm: float | None = None,
) -> SnapFitFixing:
    """梁の根元の面の中心をrootとし、length_directionへlength_mm伸びる梁とフックを作る。

    thickness_mmはたわむ向きの厚み、width_mmはそれに垂直な幅である。フックは先端から
    hook_length_mmの範囲で、deflectionと反対の側へhook_mm張り出す。deflection_mmを
    省くとhook_mmとする。はめ合い隙間を要求する分解stepでは、その分を加えて与える。
    """
    along = AXES.index(length_direction[-1])
    across = AXES.index(deflection[-1])
    if along == across:
        raise ValueError(f"{id}: deflection must be perpendicular to the length")
    if not 0.0 < hook_length_mm < length_mm:
        raise ValueError(f"{id}: hook_length_mm must be positive and shorter than length_mm")
    sign = 1.0 if length_direction.startswith("plus") else -1.0
    tip = root[along] + sign * length_mm
    half = {across: thickness_mm / 2.0, 3 - along - across: width_mm / 2.0}

    beam_low, beam_high = list(root), list(root)
    beam_low[along], beam_high[along] = sorted((root[along], tip))
    for axis, extent in half.items():
        beam_low[axis] = root[axis] - extent
        beam_high[axis] = root[axis] + extent

    hook_low, hook_high = list(beam_low), list(beam_high)
    hook_low[along], hook_high[along] = sorted((tip - sign * hook_length_mm, tip))
    if deflection.startswith("plus"):
        hook_low[across], hook_high[across] = beam_low[across] - hook_mm, beam_low[across]
    else:
        hook_low[across], hook_high[across] = beam_high[across], beam_high[across] + hook_mm

    features = (
        Feature(f"{id}_beam", Box(tuple(beam_low), tuple(beam_high)), "mount"),  # type: ignore[arg-type]
        Feature(f"{id}_hook", Box(tuple(hook_low), tuple(hook_high)), "mount"),  # type: ignore[arg-type]
    )
    snap = SnapFit(
        id=id, part=part, beam=f"{id}_beam", hook=f"{id}_hook",
        length_direction=length_direction, deflection=deflection,
        deflection_mm=hook_mm if deflection_mm is None else deflection_mm,
        mate=mate, step=step,
    )
    return SnapFitFixing(features, snap)
