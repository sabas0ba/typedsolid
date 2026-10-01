"""コネクタ開口のhelper。壁の切り欠き、プラグを抜く掃引、IRのConnectorを一度に作る。

プラグ外形の寸法は組み込まず、利用者が出典とともに与える。USB・HDMI等の規格書は
利用許諾の範囲と入手条件の点で公開catalogへの転記に使えないため、部品datasheetの値や
実測値を用いる。
"""

from dataclasses import dataclass

from .model import Box, Connector, Direction, Feature, PlugSource, Role, Sweep, Vec2

__all__ = ["ConnectorOpening", "connector_opening"]

_AXES = {"x": 0, "y": 1, "z": 2}
# 抜く軸に垂直な2軸。Cylinderのcenterと同じ順。
_PLANE = {"x": (1, 2), "y": (0, 2), "z": (0, 1)}


@dataclass(frozen=True)
class ConnectorOpening:
    """connector_openingが作る組。featureは壁の部品へ、sweepとconnectorはModelへ加える。"""

    feature: Feature
    sweep: Sweep
    connector: Connector


def _box(axis: str, span: Vec2, center: Vec2, size: Vec2) -> Box:
    low = [0.0, 0.0, 0.0]
    high = [0.0, 0.0, 0.0]
    low[_AXES[axis]], high[_AXES[axis]] = span
    for slot, index in enumerate(_PLANE[axis]):
        low[index] = center[slot] - size[slot] / 2.0
        high[index] = center[slot] + size[slot] / 2.0
    return Box((low[0], low[1], low[2]), (high[0], high[1], high[2]))


def connector_opening(
    id: str,
    part: str,
    direction: Direction,
    center: Vec2,
    plug_mm: Vec2,
    plug_span: Vec2,
    wall_span: Vec2,
    clearance_mm: float,
    source: PlugSource,
    after_step: str | None = None,
    role: Role = "generic",
) -> ConnectorOpening:
    """壁を貫く開口と、嵌合位置からプラグを抜く掃引を作る。

    directionはプラグを抜く向き。centerは抜く軸に垂直な平面でのプラグ断面の中心で、
    並びはCylinderのcenterと同じ (x方向に抜くなら(y, z))。plug_mmはその平面での
    プラグ断面の寸法。plug_spanは嵌合状態でプラグが占める抜く軸方向の範囲、
    wall_spanは切り欠く壁の抜く軸方向の範囲。開口の断面はプラグ断面を各辺
    clearance_mmだけ広げた大きさとする。掃引は残っている部品の外まで動かす。
    """
    for name, span in (("plug_span", plug_span), ("wall_span", wall_span)):
        if not span[0] < span[1]:
            raise ValueError(f"{id}: {name}は下限 < 上限である必要がある")
    if min(plug_mm) <= 0.0:
        raise ValueError(f"{id}: plug_mmは正の値である必要がある")
    if clearance_mm < 0.0:
        raise ValueError(f"{id}: clearance_mmは0以上である必要がある")
    axis = direction.split("_")[1]
    hole = (plug_mm[0] + 2.0 * clearance_mm, plug_mm[1] + 2.0 * clearance_mm)
    feature = Feature(f"{id}_opening", _box(axis, wall_span, center, hole), role, "cut")
    sweep = Sweep(
        f"{id}_plug", direction, shape=_box(axis, plug_span, center, plug_mm),
        distance_mm="exit", after_step=after_step,
    )
    connector = Connector(id, part, feature.id, sweep.id, plug_mm, clearance_mm, source)
    return ConnectorOpening(feature, sweep, connector)
