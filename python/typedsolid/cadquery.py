"""軸平行primitiveのunion-minus-cutsをCadQueryに変換する実験的backend。

exportは既定で子processにbackendを隔離し、timeoutで打ち切る。buildは形状を
呼び出し側へ返すため同一processで動く。どちらも同じ内部関数で検査し、判定は
実行経路に依らない。cache_dirを与えると部品単位の結果を保存し、中断後の
再実行では済んだ部品を再利用する。
"""

from __future__ import annotations

from collections.abc import Callable, Sequence
import copy
from dataclasses import dataclass
import hashlib
from importlib.metadata import version
import io
from itertools import combinations
import json
import math
from pathlib import Path
import shutil
import tempfile
from typing import Any

import cadquery as cq
from OCP.BRepPrimAPI import BRepPrimAPI_MakePrism
from OCP.gp import gp_Vec

from . import _native, worker
from .cache import Cache
from .figures import write_model_figures
from .model import Model
from .projection import write_projections
from .viewer import write_viewer

Progress = Callable[[str], None]
# 検出箇所と、並べ替えに使う体積の組。
Located = Sequence[tuple[float, dict]]

# OCCTの境界演算誤差を吸収する接触判定の絶対体積許容差。単位はmm³。
VOLUME_TOLERANCE = 1e-7
# 立体が空でないとみなす下限体積。単位はmm³。実装上の最小box寸法0.001 mmの立方体は
# 1e-9 mm³であり、接触判定の許容差を流用すると正当な最小形状を無効と判定するため、
# 目的の異なる閾値として独立に定義する。
EMPTY_VOLUME_TOLERANCE = 1e-12
# "exit"で残っている部品の外へ出るときの余裕。単位はmm。
EXIT_MARGIN_MM = 2.0
# exportの既定の制限時間。単位は秒。作例はcacheなしで約12秒で終わる。
DEFAULT_TIMEOUT_S = 600.0
# 図を書くdirectoryと、出力を拒否した場合に残すfile。
FIGURES_DIR = "figures"
# 投影図を置くfigures/の下のdirectory。断面図のfile名はすべて`.svg`で終わり、この名前と重ならない。
PROJECTION_DIR = "projection"
# 3D viewer。部品のmeshと検査結果を埋め込んだ1つのHTMLである。
VIEWER_FILE = "viewer.html"
REJECTED_FILES = ("report.json", FIGURES_DIR)
GEOMETRY_RULES = (
    "valid_solid", "single_solid", "keepout_clearance", "access_clearance", "part_interference",
    "disassembly_path", "disassembly_separation", "fastener_fit", "snap_fit", "fastener_release",
    "connector_fit",
)
# ネジの頭の座面として、座面からclamp側へ材料を要求する深さ。単位はmm。clampが薄ければその厚みまでとする。
BEARING_DEPTH_MM = 0.5
# 領域全体が材料であることの判定で許す不足体積の相対量。円筒面の一致による演算誤差を吸収する。
FILL_TOLERANCE = 1e-6
# 体積を断面積で割って得るかかり長さの比較で許す誤差。単位はmm。
LENGTH_TOLERANCE = 1e-6
# face法線と移動方向の内積をこれ以下とみなす面は、移動方向と平行として扱う。
PARALLEL_TOLERANCE = 1e-9
# 検出箇所の座標を丸める小数点以下の桁数。単位はmm。
LOCATION_DIGITS = 6
# 出力表現の検査であり、STLを書き出すexportでのみ評価できる。
MESH_RULES = ("mesh_manifold", "mesh_volume")
# IRから直接rasterizeして判定する。backendのtopologyに依存しない。
VOXEL_RULES = ("final_wall_thickness", "neck_section", "closed_cavity", "support_free")
AXES = "xyz"
# 各軸の負側・正側の面名。indexは軸番号に対応する。
NEGATIVE_FACES = ("minus_x", "minus_y", "minus_z")
POSITIVE_FACES = ("plus_x", "plus_y", "plus_z")


def _perpendicular(axis: int) -> list[int]:
    """軸に垂直な2軸のindex。cylinderのcenterはこの順に並ぶ。"""
    return [index for index in range(3) if index != axis]


def _aabb(shape: dict) -> tuple[list[float], list[float]]:
    """軸平行境界box。keepoutの膨張と掃引の距離はこれを基準にする。"""
    if shape["kind"] == "box":
        return list(shape["min"]), list(shape["max"])
    axis = AXES.index(shape["axis"])
    low, high = [0.0] * 3, [0.0] * 3
    low[axis], high[axis] = shape["span"]
    for slot, index in enumerate(_perpendicular(axis)):
        low[index] = shape["center"][slot] - shape["radius"]
        high[index] = shape["center"][slot] + shape["radius"]
    return low, high


def _solid(shape: dict) -> cq.Solid:
    if shape["kind"] == "box":
        lo, hi = shape["min"], shape["max"]
        return cq.Solid.makeBox(*(hi[i] - lo[i] for i in range(3)), pnt=cq.Vector(*lo))
    axis = AXES.index(shape["axis"])
    origin, direction = [0.0] * 3, [0.0] * 3
    origin[axis] = shape["span"][0]
    direction[axis] = 1.0
    for slot, index in enumerate(_perpendicular(axis)):
        origin[index] = shape["center"][slot]
    height = shape["span"][1] - shape["span"][0]
    return cq.Solid.makeCylinder(
        shape["radius"], height, pnt=cq.Vector(*origin), dir=cq.Vector(*direction)
    )


def _box_solid(bounds: tuple[list[float], list[float]]) -> cq.Solid:
    low, high = bounds
    return _solid({"kind": "box", "min": low, "max": high})


def _expanded(shape: dict, clearance: dict) -> tuple[list[float], list[float]]:
    """面ごとのclearanceでkeepoutのAABBを広げる。未指定の面にはdefaultを使う。"""
    low, high = _aabb(shape)
    default = clearance["default"]
    return (
        [low[i] - clearance.get(NEGATIVE_FACES[i], default) for i in range(3)],
        [high[i] + clearance.get(POSITIVE_FACES[i], default) for i in range(3)],
    )


def _vector(axis: int, distance: float) -> cq.Vector:
    components = [0.0, 0.0, 0.0]
    components[axis] = distance
    return cq.Vector(*components)


def _prism(face: cq.Face, vector: cq.Vector) -> list[cq.Solid]:
    maker = BRepPrimAPI_MakePrism(face.wrapped, gp_Vec(vector.x, vector.y, vector.z))
    maker.Build()
    return cq.Shape.cast(maker.Shape()).Solids()


def _split_coordinates(features: list[dict], axis: int, offset: list[float]) -> list[float]:
    """axisに垂直な分割平面の位置。axisと異なる軸を持つ円柱ごとに、その中心を通す。"""
    coordinates = set()
    for feature in features:
        shape = feature["shape"]
        if shape["kind"] != "cylinder":
            continue
        cylinder_axis = AXES.index(shape["axis"])
        if cylinder_axis == axis:
            continue
        slot = _perpendicular(cylinder_axis).index(axis)
        coordinates.add(shape["center"][slot] + offset[axis])
    return sorted(coordinates)


def _swept(shape: cq.Shape, axis: int, distance: float, coordinates: list[float]) -> cq.Shape:
    """shapeを軸方向にdistanceだけ掃引した立体。

    OCCTはsolidのprismを扱わない。掃引体積は、元の形状、終点の形状、移動方向と
    平行でない各faceのprismの和に等しい (測度0の差を除く)。境界を横切る線分は
    移動方向と交差するfaceを通るためである。閉じた円筒面を垂直方向に押し出すと
    自己交差するため、円筒軸を通り移動方向に垂直な平面で先に分割し、各半面の
    法線が移動方向に対して一定の向きを持つようにする。
    """
    vector = _vector(axis, distance)
    bounds = shape.BoundingBox()
    size = 2.0 * bounds.DiagonalLength + 10.0
    planes = []
    for coordinate in coordinates:
        base = [bounds.center.x, bounds.center.y, bounds.center.z]
        base[axis] = coordinate
        planes.append(cq.Face.makePlane(size, size, basePnt=cq.Vector(*base), dir=_vector(axis, 1.0)))
    pieces = shape.split(*planes) if planes else shape
    threshold = PARALLEL_TOLERANCE * abs(distance)
    solids = [shape.translate(vector)]
    for face in pieces.Faces():
        if abs(face.normalAt().dot(vector)) > threshold:
            solids += _prism(face, vector)
    return shape.fuse(*solids).clean()


def _laterally_expanded(
    shape: cq.Shape, axis: int, clearance: float, features: list[dict], offset: list[float],
) -> cq.Shape:
    """移動軸に垂直な2軸にだけclearanceだけ広げる。辺長2·clearanceの正方形とのMinkowski和。

    移動方向の手前にある載置面の接触は体積0のまま残り、横ですれ違う面の隙間
    不足だけが共通体積として現れる。
    """
    if clearance == 0.0:
        return shape
    for lateral in _perpendicular(axis):
        shifted = list(offset)
        shifted[lateral] -= clearance
        shape = _swept(
            shape.translate(_vector(lateral, -clearance)), lateral, 2.0 * clearance,
            _split_coordinates(features, lateral, shifted),
        )
    return shape


def _add_bounds(
    parts: Sequence[dict], offset: list[float], keepouts: Sequence[dict] = (),
) -> tuple[list[float], list[float]] | None:
    """部品の付加形状とkeepoutの箱を合わせたAABBをoffsetだけ動かしたもの。"""
    boxes = [
        _aabb(feature["shape"])
        for part in parts
        for feature in part["features"]
        if feature["operation"] == "add"
    ] + [_aabb(keepout["shape"]) for keepout in keepouts]
    if not boxes:
        return None
    return (
        [min(box[0][i] for box in boxes) + offset[i] for i in range(3)],
        [max(box[1][i] for box in boxes) + offset[i] for i in range(3)],
    )


def _exit_distance(
    moving: tuple[list[float], list[float]],
    remaining: tuple[list[float], list[float]] | None,
    direction: str,
) -> float:
    """残っている部品のAABBの外へ、EXIT_MARGIN_MMの余裕をもって出る距離。"""
    if remaining is None:
        return EXIT_MARGIN_MM
    axis = AXES.index(direction[-1])
    if direction.startswith("plus"):
        gap = remaining[1][axis] - moving[0][axis]
    else:
        gap = moving[1][axis] - remaining[0][axis]
    return max(gap, 0.0) + EXIT_MARGIN_MM


@dataclass(frozen=True)
class _Snap:
    """snap fit 1件の形状。

    hookとbeamはcutを反映した実体、deflectedはフックをたわませた位置に置いたもの、
    envelopeは梁をたわむ向きへdeflection_mmだけ掃引した、曲がった梁を含む包絡である。
    """

    data: dict
    hook: cq.Shape
    beam: cq.Shape
    rest: cq.Shape | None
    deflected: cq.Shape
    envelope: cq.Shape


def _snap_shapes(data: dict, shapes: dict[str, cq.Shape]) -> list[_Snap]:
    """snap fitごとに、フックと梁の実体、それらを除いた部品、たわんだフックと梁の包絡を作る。

    フックは梁の根元を支点に曲がるが、たわみの分だけ平行に動かして近似する。
    曲がった梁は、梁を同じ量だけ平行に掃引した包絡に含まれる。根元は実際には
    動かないため、この包絡は保守側である。
    """
    parts = {part["id"]: part for part in data["parts"]}
    result = []
    for snap in data["snap_fits"]:
        features = parts[snap["part"]]["features"]
        by_id = {feature["id"]: feature for feature in features}
        shape = shapes[snap["part"]]
        hook = shape.intersect(_solid(by_id[snap["hook"]]["shape"]))
        beam = shape.intersect(_solid(by_id[snap["beam"]]["shape"]))
        others = [f for f in features if f["id"] not in (snap["beam"], snap["hook"])]
        rest = _shape_of(others) if any(f["operation"] == "add" for f in others) else None
        axis = AXES.index(snap["deflection"][-1])
        signed = snap["deflection_mm"] if snap["deflection"].startswith("plus") else -snap["deflection_mm"]
        envelope = _swept(beam, axis, signed, _split_coordinates(features, axis, [0.0, 0.0, 0.0]))
        result.append(_Snap(snap, hook, beam, rest, hook.translate(_vector(axis, signed)), envelope))
    return result


def _released(part: dict, snaps: list[_Snap]) -> cq.Shape:
    """同じstepで外すsnap fitのフックをすべてたわませ、梁の包絡を加えた部品。

    経路に沿って掃引されるため、途中の障害物も曲がった梁との干渉として検出する。
    """
    hooks = {snap.data["hook"] for snap in snaps}
    shape = _shape_of([f for f in part["features"] if f["id"] not in hooks])
    for snap in snaps:
        shape = shape.fuse(snap.envelope).fuse(snap.deflected)
    return shape.clean()


def _disassembly_checks(
    data: dict, shapes: dict[str, cq.Shape], snaps: list[_Snap], progress: Progress,
) -> list[dict]:
    """分解stepを順に実行し、移動中の干渉と、経路の終端で外れることを検査する。

    keepoutは箱そのもの (clearanceを含まない) を障害物とする。取付先の部品を動かす
    stepではその部品と一緒に動き、以降の状態から除かれる。取付先のないkeepoutは
    最後まで残る。targetではkeepoutを`keepout:<id>`と書き、部品のidと区別する。

    snap fitを外すstepでは、フックをたわませた部品で経路を掃引する。あわせて、
    たわませないフックがそのstepの経路を塞ぐこと (保持) を検査する。
    """
    parts = {part["id"]: part for part in data["parts"]}
    assembly = data["assembly"]
    present = dict(shapes)
    present_keepouts = list(data["keepouts"])
    checks: list[dict] = []
    for step in assembly["steps"]:
        progress(f"step {step['id']}: disassembly")
        releasing = [snap for snap in snaps if snap.data["step"] == step["id"]]
        for part_id in {snap.data["part"] for snap in releasing}:
            present[part_id] = _released(parts[part_id], [snap for snap in releasing if snap.data["part"] == part_id])
        clearance = step.get("fit_clearance_mm", assembly["fit_clearance_mm"])
        moving_ids = step["parts"]
        label = f"{step['id']}/{'+'.join(moving_ids)}"
        moving_shape = present.pop(moving_ids[0])
        for part_id in moving_ids[1:]:
            moving_shape = moving_shape.fuse(present.pop(part_id))
        carried = [k for k in present_keepouts if k.get("attached_to") in moving_ids]
        present_keepouts = [k for k in present_keepouts if k not in carried]
        for keepout in carried:
            moving_shape = moving_shape.fuse(_solid(keepout["shape"]))
        features = [feature for part_id in moving_ids for feature in parts[part_id]["features"]]
        obstacles = dict(present) | {f"keepout:{k['id']}": _solid(k["shape"]) for k in present_keepouts}
        remaining_bounds = _add_bounds([parts[part_id] for part_id in present], [0.0, 0.0, 0.0], present_keepouts)
        moving_parts = [parts[p] for p in moving_ids]
        offset = [0.0, 0.0, 0.0]
        segments: list[tuple[str, float]] = []

        def sweep_overlaps(direction: str, distance: float) -> dict[str, tuple[float, Located]]:
            axis = AXES.index(direction[-1])
            signed = distance if direction.startswith("plus") else -distance
            start = moving_shape.translate(cq.Vector(*offset))
            expanded = _laterally_expanded(start, axis, clearance, features, offset)
            region = _swept(expanded, axis, signed, _split_coordinates(features, axis, offset))
            offset[axis] += signed
            return {name: _intersection(region, obstacle) for name, obstacle in obstacles.items()}

        # 検出箇所は掃引領域と障害物の共通部分であり、障害物側の組立位置にある。
        for index, segment in enumerate(step["path"]):
            direction = segment["direction"]
            distance = segment["distance_mm"]
            if distance == "exit":
                distance = _exit_distance(_add_bounds(moving_parts, offset, carried), remaining_bounds, direction)
            segments.append((direction, distance))
            overlaps = sweep_overlaps(direction, distance)
            if not overlaps:
                checks.append(_check("disassembly_path", f"{label}/{index}", True, f"{direction} {distance:.6g} mm; no remaining parts or keepouts"))
            for name, (overlap, located) in overlaps.items():
                checks.append(_check("disassembly_path", f"{label}/{index}/{name}", overlap <= VOLUME_TOLERANCE, f"{direction} {distance:.6g} mm with fit clearance {clearance} mm: overlap {overlap:.9g} mm³", located))

        # 最後の区間の方向へ外まで動かし続けられれば、部品は外れている。
        last = step["path"][-1]
        if last["distance_mm"] == "exit":
            checks.append(_check("disassembly_separation", label, True, f"last segment exits along {last['direction']}"))
        else:
            continuation = _exit_distance(_add_bounds(moving_parts, offset, carried), remaining_bounds, last["direction"])
            overlaps = sweep_overlaps(last["direction"], continuation)
            blocking = {part_id: item for part_id, item in overlaps.items() if item[0] > VOLUME_TOLERANCE}
            message = (
                f"continuing {continuation:.6g} mm along {last['direction']} "
                + ("is clear" if not blocking else "is blocked by " + ", ".join(f"{p} ({v:.9g} mm³)" for p, (v, _) in blocking.items()))
            )
            located = [item for _, boxes in blocking.values() for item in boxes]
            checks.append(_check("disassembly_separation", label, not blocking, message, located))

        for snap in releasing:
            checks.append(_retention_check(snap, shapes, parts, moving_ids, segments))
            # stepの後に残る部品では、フックは元の位置へ戻る。
            if snap.data["part"] in present:
                present[snap.data["part"]] = shapes[snap.data["part"]]
    return checks


def _retention_check(
    snap: _Snap, shapes: dict[str, cq.Shape], parts: dict[str, dict], moving_ids: list[str],
    segments: list[tuple[str, float]],
) -> dict:
    """たわませないフックとmateの相対運動がstepの経路で干渉する。干渉しなければ保持していない。

    保持しない場合の検出箇所は、組立位置にあるフックである。
    """
    mate_id = snap.data["mate"]
    # Rust coreの検証により、stepはpartとmateの一方だけを動かす。
    if snap.data["part"] in moving_ids:
        piece, obstacle, features = snap.hook, shapes[mate_id], []
    else:
        piece, obstacle, features = shapes[mate_id], snap.hook, parts[mate_id]["features"]
    offset = [0.0, 0.0, 0.0]
    total = 0.0
    for direction, distance in segments:
        axis = AXES.index(direction[-1])
        signed = distance if direction.startswith("plus") else -distance
        region = _swept(piece.translate(cq.Vector(*offset)), axis, signed, _split_coordinates(features, axis, offset))
        total += _overlap(region, obstacle)
        offset[axis] += signed
    return _check(
        "snap_fit", f"{snap.data['id']}/retention", total > VOLUME_TOLERANCE,
        f"undeflected hook against {mate_id} along step {snap.data['step']}: overlap {total:.9g} mm³",
        _boxes(snap.hook),
    )


def _present_after(data: dict, step_id: str | None) -> list[str]:
    """step_idを終えた状態で残っている部品。Noneは組立完了の状態。"""
    removed: set[str] = set()
    if step_id is not None:
        for step in data["assembly"]["steps"]:
            removed.update(step["parts"])
            if step["id"] == step_id:
                break
    return [part["id"] for part in data["parts"] if part["id"] not in removed]


def _present_before(data: dict, step_id: str) -> list[str]:
    """step_idを始める直前に残っている部品。"""
    removed: set[str] = set()
    for step in data["assembly"]["steps"]:
        if step["id"] == step_id:
            break
        removed.update(step["parts"])
    return [part["id"] for part in data["parts"] if part["id"] not in removed]


def _snap_checks(data: dict, shapes: dict[str, cq.Shape], snaps: list[_Snap], progress: Progress) -> list[dict]:
    """snap fitの梁とフックが実形状にあり、外すstepの直前の状態でたわむ空間が空いている。

    たわむ空間は、残っている部品とkeepoutの箱 (clearanceを含まない) を障害物とする。

    たわむ空間は、梁とフックをdeflection_mmだけ平行に動かした掃引で表す。根元は実際には
    動かないため、根元付近では保守側の判定になる。
    """
    parts = {part["id"]: part for part in data["parts"]}
    checks: list[dict] = []
    for snap in snaps:
        snap_id, part_id = snap.data["id"], snap.data["part"]
        progress(f"snap fit {snap_id}")
        features = parts[part_id]["features"]
        by_id = {feature["id"]: feature for feature in features}
        boxes = _solid(by_id[snap.data["beam"]]["shape"]).fuse(_solid(by_id[snap.data["hook"]]["shape"]))
        expected = sum(abs(s.Volume()) for s in boxes.Solids())
        filled = _overlap(boxes, shapes[part_id])
        passed = expected - filled <= max(VOLUME_TOLERANCE, FILL_TOLERANCE * expected)
        checks.append(_check(
            "snap_fit", f"{snap_id}/beam", passed, f"{filled / expected:.6%} of the beam and hook boxes is material of {part_id}",
            () if passed else _missing(boxes, shapes[part_id]),
        ))

        axis = AXES.index(snap.data["deflection"][-1])
        signed = snap.data["deflection_mm"] if snap.data["deflection"].startswith("plus") else -snap.data["deflection_mm"]
        hook_region = _swept(snap.hook, axis, signed, _split_coordinates(features, axis, [0.0, 0.0, 0.0]))
        region = snap.envelope.fuse(hook_region)
        obstacles = {} if snap.rest is None else {part_id: snap.rest}
        present = _present_before(data, snap.data["step"])
        for other in present:
            if other != part_id:
                obstacles[other] = shapes[other]
        # keepoutは取付先がsnap fitを持つ部品でも、梁と一緒にはたわまない。
        for keepout in data["keepouts"]:
            if keepout.get("attached_to") in (None, *present):
                obstacles[f"keepout:{keepout['id']}"] = _solid(keepout["shape"])
        blocking = {name: _intersection(region, obstacle) for name, obstacle in obstacles.items()}
        blocking = {name: item for name, item in blocking.items() if item[0] > VOLUME_TOLERANCE}
        message = (
            f"deflecting {snap.data['deflection_mm']} mm along {snap.data['deflection']} before {snap.data['step']} "
            + ("is clear" if not blocking else "hits " + ", ".join(f"{name} ({overlap:.9g} mm³)" for name, (overlap, _) in blocking.items()))
        )
        located = [item for _, boxes in blocking.values() for item in boxes]
        checks.append(_check("snap_fit", f"{snap_id}/deflection_space", not blocking, message, located))
    return checks


def _sweep_checks(data: dict, shapes: dict[str, cq.Shape], progress: Progress) -> list[dict]:
    """工具・ケーブル・コネクタやkeepoutの掃引が、その状態で残っている部品と干渉しない。

    keepoutを参照する掃引は、keepoutをclearanceの分だけ広げたboxを使う。
    """
    parts = {part["id"]: part for part in data["parts"]}
    keepouts = {keepout["id"]: keepout for keepout in data["keepouts"]}
    checks: list[dict] = []
    for sweep in data["sweeps"]:
        progress(f"sweep {sweep['id']}")
        if "keepout" in sweep:
            keepout = keepouts[sweep["keepout"]]
            bounds = _expanded(keepout["shape"], keepout["clearance_mm"])
            start = _box_solid(bounds)
            features: list[dict] = []
        else:
            bounds = _aabb(sweep["shape"])
            start = _solid(sweep["shape"])
            features = [{"shape": sweep["shape"]}]
        present = _present_after(data, sweep.get("after_step"))
        state = sweep.get("after_step", "assembly")
        direction = sweep["direction"]
        distance = sweep["distance_mm"]
        if distance == "exit":
            distance = _exit_distance(bounds, _add_bounds([parts[p] for p in present], [0.0, 0.0, 0.0]), direction)
        axis = AXES.index(direction[-1])
        signed = distance if direction.startswith("plus") else -distance
        region = _swept(start, axis, signed, _split_coordinates(features, axis, [0.0, 0.0, 0.0]))
        if not present:
            checks.append(_check("access_clearance", sweep["id"], True, f"{direction} {distance:.6g} mm after {state}; no remaining parts"))
        for part_id in present:
            overlap, located = _intersection(region, shapes[part_id])
            checks.append(_check("access_clearance", f"{sweep['id']}/{part_id}", overlap <= VOLUME_TOLERANCE, f"{direction} {distance:.6g} mm after {state}: overlap {overlap:.9g} mm³", located))
    return checks


def _fastener_checks(data: dict, shapes: dict[str, cq.Shape], progress: Progress) -> list[dict]:
    """ネジ固定の寸法が実形状と整合する。組立完了の状態で、ネジ1本ごとに5項目を見る。

    through: clampの貫通穴の径にclampの材料がない。
    bearing: 頭の座面の輪帯がclampの材料で埋まっている。
    engagement: ねじ山がかかる長さが要求以上ある。
    clear_tip: 下穴とインサート穴、インサートより先のねじ部の経路にbaseの材料がない。
    boss_wall: 下穴またはインサート穴の周囲にmin_boss_wall_mmの肉がある。
    """
    checks: list[dict] = []
    for fastener in data["fasteners"]:
        fastener_id = fastener["id"]
        progress(f"fastener {fastener_id}")
        direction = fastener["direction"]
        sign = 1.0 if direction.startswith("plus") else -1.0
        seat, joint = fastener["seat_mm"], fastener["joint_mm"]
        screw, anchor = fastener["screw"], fastener["anchor"]
        major = screw["major_mm"]
        tip = seat + sign * screw["length_mm"]
        base = shapes[fastener["base"]]

        def cylinder(diameter: float, start: float, end: float) -> cq.Solid:
            shape = {
                "kind": "cylinder", "axis": direction[-1], "center": fastener["center"],
                "radius": diameter / 2.0, "span": [min(start, end), max(start, end)],
            }
            return _solid(shape)

        def filled(inner: float, outer: float, start: float, end: float, material: cq.Shape) -> tuple[bool, float, Located]:
            """輪帯が材料で埋まっているか、埋まっている割合、埋まっていない部分の外接box。"""
            ring = cylinder(outer, start, end).cut(cylinder(inner, start, end))
            expected = math.pi * (outer**2 - inner**2) / 4.0 * abs(end - start)
            present = _overlap(ring, material)
            passed = expected - present <= max(VOLUME_TOLERANCE, FILL_TOLERANCE * expected)
            return passed, present / expected, () if passed else _missing(ring, material)

        def record(aspect: str, passed: bool, message: str, located: Located = ()) -> None:
            checks.append(_check("fastener_fit", f"{fastener_id}/{aspect}", passed, message, located))

        if not fastener["clamp"]:
            for aspect in ("through", "bearing"):
                record(aspect, True, "no clamp part declared; the clamped object is not modelled")
        else:
            clamp = shapes[fastener["clamp"][0]]
            for part_id in fastener["clamp"][1:]:
                clamp = clamp.fuse(shapes[part_id])
            through = fastener["through_mm"]
            overlap, located = _intersection(cylinder(through, seat, joint), clamp)
            record("through", overlap <= VOLUME_TOLERANCE, f"clamp material inside the {through} mm through hole: {overlap:.9g} mm³", located)
            depth = min(BEARING_DEPTH_MM, sign * (joint - seat))
            passed, fraction, located = filled(through, screw["head_mm"], seat, seat + sign * depth, clamp)
            record("bearing", passed, f"{fraction:.6%} of the ring between {through} and {screw['head_mm']} mm, {depth:.6g} mm under the head, is clamp material", located)

        reach = sign * (tip - joint)
        obstructions: list[tuple[float, dict]] = []
        if anchor["kind"] == "self_tapping":
            bore = anchor["pilot_mm"]
            span = reach
            if reach > 0.0:
                area = math.pi * (major**2 - bore**2) / 4.0
                ring = cylinder(major, joint, tip).cut(cylinder(bore, joint, tip))
                engaged = _overlap(ring, base) / area
                blocked, obstructions = _intersection(cylinder(bore, joint, tip), base)
            else:
                engaged, blocked = 0.0, 0.0
            engagement = f"thread between {bore} and {major} mm engages {engaged:.6g} mm of base over a reach of {reach:.6g} mm"
        else:
            bore, length = anchor["hole_mm"], anchor["length_mm"]
            span = length
            engaged = min(max(reach, 0.0), length)
            bottom = joint + sign * length
            blocked, obstructions = _intersection(cylinder(bore, joint, bottom), base)
            if reach > length:
                beyond, located = _intersection(cylinder(major, bottom, tip), base)
                blocked += beyond
                obstructions = [*obstructions, *located]
            engagement = f"screw reaches {reach:.6g} mm past the joint into a {length} mm insert; engages {engaged:.6g} mm"
        required = fastener["min_engagement_mm"]
        # かかり長さの不足は、要求されるかかり長さの範囲のネジを検出箇所とする。
        engaging = cylinder(major, joint, joint + sign * required)
        record("engagement", engaged >= required - LENGTH_TOLERANCE, f"{engagement}; required {required} mm", _boxes(engaging))
        record("clear_tip", blocked <= VOLUME_TOLERANCE, f"base material in the bore and the screw path: {blocked:.9g} mm³", obstructions)
        wall = fastener["min_boss_wall_mm"]
        if span > 0.0:
            passed, fraction, located = filled(bore, bore + 2.0 * wall, joint, joint + sign * span, base)
            record("boss_wall", passed, f"{fraction:.6%} of a {wall} mm wall around the {bore} mm bore over {span:.6g} mm is base material", located)
        else:
            checks.append({
                "rule": "fastener_fit", "target": f"{fastener_id}/boss_wall", "status": "not_evaluated",
                "message": "the screw does not reach past the joint; no bore span to evaluate",
            })
    return checks


def _check(rule: str, target: str, passed: bool, message: str, located: Located = ()) -> dict:
    """checkを作る。failなら検出箇所を体積の大きい順にMAX_LOCATIONS件まで持たせ、総数をmessageに書く。"""
    check = {"rule": rule, "target": target, "status": "pass" if passed else "fail", "message": message}
    if not passed and located:
        ordered = sorted(located, key=lambda item: -item[0])
        check["locations"] = [location for _, location in ordered[:_native.MAX_LOCATIONS]]
        check["message"] += f"; {len(ordered)} location(s)"
    return check


def _boxes(shape: cq.Shape) -> Located:
    """solidごとの体積と外接box。体積が無視できるsolidは除く。

    座標はOCCTの許容差による端数を除くため、LOCATION_DIGITS桁に丸める。
    """
    located = []
    for solid in shape.Solids():
        volume = abs(solid.Volume())
        if volume <= EMPTY_VOLUME_TOLERANCE:
            continue
        box = solid.BoundingBox()
        located.append((volume, {
            "min": [round(v, LOCATION_DIGITS) for v in (box.xmin, box.ymin, box.zmin)],
            "max": [round(v, LOCATION_DIGITS) for v in (box.xmax, box.ymax, box.zmax)],
        }))
    return located


def _intersection(a: cq.Shape, b: cq.Shape) -> tuple[float, Located]:
    """共通部分の体積と、共通部分のsolidごとの外接box。"""
    common = a.intersect(b)
    if not common.isValid():
        raise ValueError("intersection returned invalid topology")
    volume = sum(abs(s.Volume()) for s in common.Solids())
    if not math.isfinite(volume):
        raise ValueError("intersection returned non-finite volume")
    return volume, _boxes(common)


def _overlap(a: cq.Shape, b: cq.Shape) -> float:
    return _intersection(a, b)[0]


def _missing(region: cq.Shape, material: cq.Shape) -> Located:
    """regionのうちmaterialで埋まっていない部分の外接box。材料の不足を示すcheckのfail時にだけ求める。"""
    return _boxes(region.cut(material))


@dataclass(frozen=True)
class Build:
    shapes: dict[str, cq.Shape]
    report: dict[str, Any]
    model_json: str

    @property
    def export_allowed(self) -> bool:
        return _native.export_allowed(json.dumps(self.report))


def _silent(stage: str) -> None:
    del stage


def _open(cache_dir: str | Path | None) -> Cache | None:
    return None if cache_dir is None else Cache(cache_dir)


def _shape_of(features: list[dict]) -> cq.Shape:
    """全Addの和から全Cutを引く。Addを1つ以上含むこと。"""
    additives = [_solid(f["shape"]) for f in features if f["operation"] == "add"]
    shape: cq.Shape = additives[0]
    for additive in additives[1:]:
        shape = shape.fuse(additive)
    for feature in features:
        if feature["operation"] == "cut":
            shape = shape.cut(_solid(feature["shape"]))
    return shape.clean()


def _part_shape(part: dict, cache: Cache | None, progress: Progress) -> cq.Shape:
    """部品の最終形状。cacheにあればbinary BREPから復元する。"""
    key = "" if cache is None else cache.key("brep", part)
    content = None if cache is None else cache.get(key)
    if content is not None:
        progress(f"part {part['id']}: shape reused")
        return cq.Shape.importBin(io.BytesIO(content))
    progress(f"part {part['id']}: building shape")
    shape = _shape_of(part["features"])
    if cache is not None:
        buffer = io.BytesIO()
        shape.exportBin(buffer)
        cache.put(key, buffer.getvalue())
    return shape


def _voxel_checks(data: dict, cache: Cache | None, progress: Progress) -> list[dict]:
    """部品ごとにvoxel評価する。評価は部品単位で独立しており、keepout、掃引、assemblyを使わない。"""
    checks: list[dict] = []
    for part in data["parts"]:
        key = "" if cache is None else cache.key("voxel", {"part": part, "policy": data["policy"]})
        content = None if cache is None else cache.get(key)
        if content is not None:
            progress(f"part {part['id']}: voxel checks reused")
            checks += json.loads(content)
            continue
        progress(f"part {part['id']}: voxel evaluation")
        # 単部品のモデルとして評価する。掃引、分解step、ネジ固定、snap fit、コネクタ開口は
        # 他の部品や掃引を参照するため除く。
        single = json.dumps(
            {
                **data, "parts": [part], "keepouts": [], "sweeps": [],
                "assembly": {"fit_clearance_mm": 0.0, "steps": []}, "fasteners": [], "snap_fits": [],
                "connectors": [],
            },
            allow_nan=False,
        )
        result = json.loads(_native.evaluate_voxels(single))
        if cache is not None:
            cache.put(key, json.dumps(result).encode())
        checks += result
    return checks


def build(
    model: Model,
    *,
    cache_dir: str | Path | None = None,
    progress: Progress | None = None,
) -> Build:
    """検査結果と形状を返す。CAD例外はfailに変換し、形状は出力しない。

    同一processで動くため、backendが停止すると呼び出し側も停止する。
    形状が不要で打ち切りが必要な場合はexportを使う。
    """
    return _build(model.to_json(), _open(cache_dir), progress or _silent)


def _build(model_json: str, cache: Cache | None, progress: Progress) -> Build:
    data = json.loads(model_json)
    report = json.loads(_native.preflight(model_json))
    checks = report["checks"]
    shapes: dict[str, cq.Shape] = {}
    geometry: list[dict] = []
    try:
        for part in data["parts"]:
            shape = _part_shape(part, cache, progress)
            solids = shape.Solids()
            volume = sum(abs(s.Volume()) for s in solids)
            valid = bool(solids) and shape.isValid() and math.isfinite(volume) and volume > EMPTY_VOLUME_TOLERANCE
            geometry.append(_check("valid_solid", part["id"], valid, f"valid={shape.isValid()}, volume={volume:.9g} mm³"))
            # 分かれた場合は各solidの外接boxを検出箇所とする。最大のsolidが本体である。
            geometry.append(_check("single_solid", part["id"], len(solids) == 1, f"final solid count: {len(solids)}", _boxes(shape)))
            shapes[part["id"]] = shape

        progress("keepout and interference checks")
        for keepout in data["keepouts"]:
            clearance = keepout["clearance_mm"]
            bounds = _expanded(keepout["shape"], clearance)
            volume_shape = _box_solid(bounds)
            clearance_text = ", ".join(f"{face}={value}" for face, value in clearance.items())
            for part_id, shape in shapes.items():
                overlap, located = _intersection(shape, volume_shape)
                geometry.append(_check("keepout_clearance", f"{keepout['id']}/{part_id}", overlap <= VOLUME_TOLERANCE, f"overlap: {overlap:.9g} mm³; clearance: {clearance_text}", located))
        for (left, a), (right, b) in combinations(shapes.items(), 2):
            overlap, located = _intersection(a, b)
            geometry.append(_check("part_interference", f"{left}/{right}", overlap <= VOLUME_TOLERANCE, f"overlap: {overlap:.9g} mm³", located))
        snaps = _snap_shapes(data, shapes)
        geometry += _disassembly_checks(data, shapes, snaps, progress)
        # 経路上に別部品がある場合も検査するため、残っている部品の外まで掃引する。
        geometry += _sweep_checks(data, shapes, progress)
        geometry += _fastener_checks(data, shapes, progress)
        # ひずみと積層方向は寸法だけで決まるため、判定をRust coreに置く。
        geometry += json.loads(_native.evaluate_snap_fits(model_json))
        # ネジを外す順序も形状を使わないため、判定をRust coreに置く。
        geometry += json.loads(_native.evaluate_fastener_releases(model_json))
        # コネクタ開口の寸法の整合もIRの寸法だけで決まる。
        geometry += json.loads(_native.evaluate_connectors(model_json))
        geometry += _snap_checks(data, shapes, snaps, progress)
        for rule in GEOMETRY_RULES:
            if not any(c["rule"] == rule for c in geometry):
                geometry.append(_check(rule, "model", True, "no applicable declared targets"))
        checks[:] = [c for c in checks if c["rule"] not in GEOMETRY_RULES] + geometry
    except Exception as error:
        # Backendの例外は握りつぶして合格にせず、明示的な失敗として残す。
        shapes.clear()
        checks.append(_check("valid_solid", "backend", False, f"{type(error).__name__}: {error}"))
    # voxel評価はCAD backendを経由しないため、上のgeometry検査とは独立に扱う。
    try:
        voxel_checks = _voxel_checks(data, cache, progress)
    except Exception as error:
        voxel_checks = [_check(rule, "model", False, f"{type(error).__name__}: {error}") for rule in VOXEL_RULES]
    checks[:] = [c for c in checks if c["rule"] not in VOXEL_RULES] + voxel_checks
    return Build(shapes, report, model_json)


def write_projection_figures(result: Build, directory: str | Path, *, overview: bool = True) -> list[str]:
    """buildの結果について、組立状態の投影図をdirectoryへ書き、file名を返す。keepoutの箱を重ねる。

    cutで材料が残らない部品は外接boxを持たず、隠線処理できないため描かない。
    描ける部品がなければ何も書かない。
    """
    keepouts = [_aabb(keepout["shape"]) for keepout in json.loads(result.model_json)["keepouts"]]
    return write_projections(_drawable(result), result.report["checks"], directory, keepouts, overview)


def write_viewer_figure(
    result: Build, path: str | Path, *, title: str | None = None, checks: list[dict] | None = None,
) -> Path:
    """buildの結果について、部品、keepout、checkを埋め込んだ3D viewerのHTMLをpathへ書く。

    checksを与えるとbuildのreportの代わりに埋め込む。exportがmeshの検査を加えたreportで
    書き直すために使う。描ける部品がない場合 (backendの例外、材料の残らない部品) も、
    checkの一覧は表示できるため書く。
    """
    data = json.loads(result.model_json)
    keepouts = [(keepout["id"], _aabb(keepout["shape"])) for keepout in data["keepouts"]]
    name = title or "TypedSolid: " + ", ".join(part["id"] for part in data["parts"])
    embedded = result.report["checks"] if checks is None else checks
    return write_viewer(_drawable(result), embedded, path, keepouts, name)


def _drawable(result: Build) -> dict[str, cq.Shape]:
    """体積を持つsolidのある部品。cutで材料が残らない部品は外接boxを持たず、描けない。"""
    return {
        part_id: shape for part_id, shape in result.shapes.items()
        if any(abs(solid.Volume()) > EMPTY_VOLUME_TOLERANCE for solid in shape.Solids())
    }


def _reject_unless_allowed(report: dict) -> None:
    if _native.export_allowed(json.dumps(report)):
        return
    failures = [c for c in report["checks"] if c["status"] == "fail" or (c["rule"] in report["required"] and c["status"] != "pass")]
    raise ValueError("export blocked: " + json.dumps(failures, ensure_ascii=False))


def _reject_with_figures(report: dict, root: Path, figure_names: list[str]) -> None:
    """出力を拒否する場合、reportと図をstagingに残してから送出する。親がREJECTED_FILESだけを移す。"""
    if _native.export_allowed(json.dumps(report)):
        return
    rejected = {"schema_version": 1, "units": "mm", "rejected": True, "report": report, "figures": figure_names}
    (root / "report.json").write_text(json.dumps(rejected, indent=2) + "\n", encoding="utf-8")
    _reject_unless_allowed(report)


def _export_staged(
    progress: Progress, model_json: str, staging: str, cache_dir: str | None, figures: bool = True,
) -> dict:
    """検査し、stagingへ出力してmanifestを返す。子processでも同一processでも同じ処理を通る。"""
    result = _build(model_json, _open(cache_dir), progress)
    root = Path(staging)
    figure_names: list[str] = []
    if figures:
        progress("writing figures")
        figure_names = write_model_figures(result.model_json, result.report["checks"], root / FIGURES_DIR)
        # backendが例外で止まった場合は形状がなく、投影図を描かない。
        progress("writing projections")
        projections = write_projection_figures(result, root / FIGURES_DIR / PROJECTION_DIR)
        figure_names += [f"{PROJECTION_DIR}/{name}" for name in projections]
        progress("writing viewer")
        write_viewer_figure(result, root / FIGURES_DIR / VIEWER_FILE)
        figure_names.append(VIEWER_FILE)
    _reject_with_figures(result.report, root, figure_names)
    tolerance = json.loads(result.model_json)["policy"]["mesh_volume_tolerance"]
    files = {}
    mesh_checks: list[dict] = []
    for part_id, shape in result.shapes.items():
        for suffix in ("stl", "step"):
            progress(f"part {part_id}: writing {suffix}")
            path = root / f"{part_id}.{suffix}"
            cq.exporters.export(shape, str(path), tolerance=0.01, angularTolerance=0.1)
            content = path.read_bytes()
            if not content:
                raise ValueError(f"empty export: {part_id}.{suffix}")
            files[path.name] = hashlib.sha256(content).hexdigest()
            if suffix == "stl":
                volume = sum(abs(s.Volume()) for s in shape.Solids())
                mesh_checks += json.loads(_native.inspect_mesh(content, part_id, volume, tolerance))
    # meshは書き出し後にしか検査できない。failなら呼び出し側がstagingごと破棄する。
    report = copy.deepcopy(result.report)
    report["checks"] = [c for c in report["checks"] if c["rule"] not in MESH_RULES] + mesh_checks
    if figures:
        # viewerはcheckの状態を一覧で示すため、meshの検査を加えたreportで書き直す。
        # 断面図と投影図は検出箇所だけを描き、meshの検査は検出箇所を持たないため変わらない。
        write_viewer_figure(result, root / FIGURES_DIR / VIEWER_FILE, checks=report["checks"])
    _reject_with_figures(report, root, figure_names)
    # 保存bytesとdigestを同一の値から得る。model.jsonの再hashで照合できるようにする。
    model_bytes = (result.model_json + "\n").encode("utf-8")
    (root / "model.json").write_bytes(model_bytes)
    manifest = {
        # report.json自体の形式版。意味モデルの版はmodel.jsonが持つ。
        "schema_version": 1, "units": "mm", "cadquery": version("cadquery"),
        "cadquery_ocp": version("cadquery-ocp"),
        "model_sha256": hashlib.sha256(model_bytes).hexdigest(),
        "files_sha256": files, "report": report, "figures": figure_names,
        "notice": "Only listed checks were evaluated. This is not a printability or structural safety certification.",
    }
    (root / "report.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    return manifest


def _keep_rejected(staging: Path, target: Path, error: Exception) -> None:
    """検査で拒否したstagingから、reportと図だけを隣のdirectoryへ移す。"""
    report = staging / "report.json"
    if not report.exists() or not json.loads(report.read_text(encoding="utf-8")).get("rejected"):
        return
    rejected = target.with_name(target.name + ".rejected")
    if rejected.exists():
        error.add_note(f"report and figures were not kept: {rejected} already exists")
        return
    rejected.mkdir()
    for name in REJECTED_FILES:
        if (staging / name).exists():
            (staging / name).rename(rejected / name)
    error.add_note(f"report and figures: {rejected}")


def export(
    model: Model,
    directory: str | Path,
    *,
    timeout_s: float | None = DEFAULT_TIMEOUT_S,
    cache_dir: str | Path | None = None,
    progress: Progress | None = None,
    isolated: bool = True,
    figures: bool = True,
) -> dict:
    """モデルを検査し、新規ディレクトリへ出力する。既存成果物は上書きしない。

    既定では子processで実行し、timeout_sを超えるとWorkerTimeoutを送出する。
    isolated=Falseは同一processで実行し、timeout_sを使わない。backendを
    debuggerで追う場合に使う。いずれの経路でも、失敗時はdirectoryを作らない。

    figuresが真なら、部品の概観と検出箇所の断面図を`figures/`に、組立状態の投影図を
    `figures/projection/`に、3D viewerを`figures/viewer.html`に書く。検査で出力を拒否した場合は、reportと図だけを
    `<directory>.rejected/`に残し、例外にその場所を注記する。STL/STEPは残さない。
    """
    target = Path(directory)
    if target.exists():
        raise FileExistsError(f"output directory already exists: {target}")
    target.parent.mkdir(parents=True, exist_ok=True)
    # stagingの作成と破棄は親が持つ。子がtimeoutで止められても残骸を残さない。
    staging = Path(tempfile.mkdtemp(prefix=".typedsolid-", dir=target.parent))
    cache = None if cache_dir is None else str(cache_dir)
    try:
        try:
            if isolated:
                manifest = worker.run(
                    _export_staged, model.to_json(), str(staging), cache, figures,
                    timeout_s=timeout_s, progress=progress,
                )
            else:
                manifest = _export_staged(progress or _silent, model.to_json(), str(staging), cache, figures)
        except Exception as error:
            _keep_rejected(staging, target, error)
            raise
        # mkdirは存在判定と作成を一体で行う。競合時に他者の出力を置換しない。
        target.mkdir()
        for path in staging.iterdir():
            path.rename(target / path.name)
        return manifest
    finally:
        shutil.rmtree(staging, ignore_errors=True)
