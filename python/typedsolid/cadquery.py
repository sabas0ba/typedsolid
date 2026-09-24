"""軸平行primitiveのunion-minus-cutsをCadQueryに変換する実験的backend。

exportは既定で子processにbackendを隔離し、timeoutで打ち切る。buildは形状を
呼び出し側へ返すため同一processで動く。どちらも同じ内部関数で検査し、判定は
実行経路に依らない。cache_dirを与えると部品単位の結果を保存し、中断後の
再実行では済んだ部品を再利用する。
"""

from __future__ import annotations

from collections.abc import Callable
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

from . import _native, worker
from .cache import Cache
from .model import Model

Progress = Callable[[str], None]

# OCCTの境界演算誤差を吸収する接触判定の絶対体積許容差。単位はmm³。
VOLUME_TOLERANCE = 1e-7
# 立体が空でないとみなす下限体積。単位はmm³。実装上の最小box寸法0.001 mmの立方体は
# 1e-9 mm³であり、接触判定の許容差を流用すると正当な最小形状を無効と判定するため、
# 目的の異なる閾値として独立に定義する。
EMPTY_VOLUME_TOLERANCE = 1e-12
# accessの掃引をモデル境界の外へ出す余裕。単位はmm。
EXIT_MARGIN_MM = 2.0
# exportの既定の制限時間。単位は秒。作例はcacheなしで約12秒で終わる。
DEFAULT_TIMEOUT_S = 600.0
GEOMETRY_RULES = (
    "valid_solid", "single_solid", "keepout_clearance", "access_clearance", "part_interference",
)
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
    """軸平行境界box。keepoutの膨張とaccessの掃引はこれを基準にする。"""
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


def _corridor(
    bounds: tuple[list[float], list[float]],
    direction: str,
    model: tuple[list[float], list[float]],
) -> tuple[tuple[list[float], list[float]], float]:
    """指定方向へモデル境界の外まで掃引した領域と、その到達位置。"""
    low, high = list(bounds[0]), list(bounds[1])
    axis = AXES.index(direction[-1])
    if direction.startswith("plus"):
        high[axis] = max(model[1][axis] + EXIT_MARGIN_MM, high[axis])
        return (low, high), high[axis]
    low[axis] = min(model[0][axis] - EXIT_MARGIN_MM, low[axis])
    return (low, high), low[axis]


def _model_bounds(data: dict) -> tuple[list[float], list[float]]:
    """全部品の付加形状を含むAABB。"""
    boxes = [
        _aabb(feature["shape"])
        for part in data["parts"]
        for feature in part["features"]
        if feature["operation"] == "add"
    ]
    return (
        [min(box[0][i] for box in boxes) for i in range(3)],
        [max(box[1][i] for box in boxes) for i in range(3)],
    )


def _check(rule: str, target: str, passed: bool, message: str) -> dict:
    return {"rule": rule, "target": target, "status": "pass" if passed else "fail", "message": message}


def _overlap(a: cq.Shape, b: cq.Shape) -> float:
    common = a.intersect(b)
    if not common.isValid():
        raise ValueError("intersection returned invalid topology")
    volume = sum(abs(s.Volume()) for s in common.Solids())
    if not math.isfinite(volume):
        raise ValueError("intersection returned non-finite volume")
    return volume


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


def _part_shape(part: dict, cache: Cache | None, progress: Progress) -> cq.Shape:
    """部品の最終形状。cacheにあればbinary BREPから復元する。"""
    key = "" if cache is None else cache.key("brep", part)
    content = None if cache is None else cache.get(key)
    if content is not None:
        progress(f"part {part['id']}: shape reused")
        return cq.Shape.importBin(io.BytesIO(content))
    progress(f"part {part['id']}: building shape")
    additives = [_solid(f["shape"]) for f in part["features"] if f["operation"] == "add"]
    shape: cq.Shape = additives[0]
    for additive in additives[1:]:
        shape = shape.fuse(additive)
    for feature in part["features"]:
        if feature["operation"] == "cut":
            shape = shape.cut(_solid(feature["shape"]))
    shape = shape.clean()
    if cache is not None:
        buffer = io.BytesIO()
        shape.exportBin(buffer)
        cache.put(key, buffer.getvalue())
    return shape


def _voxel_checks(data: dict, cache: Cache | None, progress: Progress) -> list[dict]:
    """部品ごとにvoxel評価する。評価は部品単位で独立しており、keepoutを使わない。"""
    checks: list[dict] = []
    for part in data["parts"]:
        key = "" if cache is None else cache.key("voxel", {"part": part, "policy": data["policy"]})
        content = None if cache is None else cache.get(key)
        if content is not None:
            progress(f"part {part['id']}: voxel checks reused")
            checks += json.loads(content)
            continue
        progress(f"part {part['id']}: voxel evaluation")
        single = json.dumps({**data, "parts": [part], "keepouts": []}, allow_nan=False)
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
            geometry.append(_check("single_solid", part["id"], len(solids) == 1, f"final solid count: {len(solids)}"))
            shapes[part["id"]] = shape

        # モデル全体の外まで掃引することで、経路上に別部品がある場合も検査する。
        progress("keepout, access and interference checks")
        model_bounds = _model_bounds(data)
        for keepout in data["keepouts"]:
            clearance = keepout["clearance_mm"]
            bounds = _expanded(keepout["shape"], clearance)
            volume_shape = _box_solid(bounds)
            clearance_text = ", ".join(f"{face}={value}" for face, value in clearance.items())
            for part_id, shape in shapes.items():
                overlap = _overlap(shape, volume_shape)
                geometry.append(_check("keepout_clearance", f"{keepout['id']}/{part_id}", overlap <= VOLUME_TOLERANCE, f"overlap: {overlap:.9g} mm³; clearance: {clearance_text}"))
            for direction in keepout["access"]:
                corridor, exit_at = _corridor(bounds, direction, model_bounds)
                sweep = _box_solid(corridor)
                for part_id, shape in shapes.items():
                    overlap = _overlap(shape, sweep)
                    geometry.append(_check("access_clearance", f"{keepout['id']}/{part_id}/{direction}", overlap <= VOLUME_TOLERANCE, f"{direction} straight access overlap: {overlap:.9g} mm³; exit at {exit_at} mm"))
        for (left, a), (right, b) in combinations(shapes.items(), 2):
            overlap = _overlap(a, b)
            geometry.append(_check("part_interference", f"{left}/{right}", overlap <= VOLUME_TOLERANCE, f"overlap: {overlap:.9g} mm³"))
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


def _reject_unless_allowed(report: dict) -> None:
    if _native.export_allowed(json.dumps(report)):
        return
    failures = [c for c in report["checks"] if c["status"] == "fail" or (c["rule"] in report["required"] and c["status"] != "pass")]
    raise ValueError("export blocked: " + json.dumps(failures, ensure_ascii=False))


def _export_staged(progress: Progress, model_json: str, staging: str, cache_dir: str | None) -> dict:
    """検査し、stagingへ出力してmanifestを返す。子processでも同一processでも同じ処理を通る。"""
    result = _build(model_json, _open(cache_dir), progress)
    _reject_unless_allowed(result.report)
    tolerance = json.loads(result.model_json)["policy"]["mesh_volume_tolerance"]
    root = Path(staging)
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
    _reject_unless_allowed(report)
    # 保存bytesとdigestを同一の値から得る。model.jsonの再hashで照合できるようにする。
    model_bytes = (result.model_json + "\n").encode("utf-8")
    (root / "model.json").write_bytes(model_bytes)
    manifest = {
        # report.json自体の形式版。意味モデルの版はmodel.jsonが持つ。
        "schema_version": 1, "units": "mm", "cadquery": version("cadquery"),
        "cadquery_ocp": version("cadquery-ocp"),
        "model_sha256": hashlib.sha256(model_bytes).hexdigest(),
        "files_sha256": files, "report": report,
        "notice": "Only listed checks were evaluated. This is not a printability or structural safety certification.",
    }
    (root / "report.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    return manifest


def export(
    model: Model,
    directory: str | Path,
    *,
    timeout_s: float | None = DEFAULT_TIMEOUT_S,
    cache_dir: str | Path | None = None,
    progress: Progress | None = None,
    isolated: bool = True,
) -> dict:
    """モデルを検査し、新規ディレクトリへ出力する。既存成果物は上書きしない。

    既定では子processで実行し、timeout_sを超えるとWorkerTimeoutを送出する。
    isolated=Falseは同一processで実行し、timeout_sを使わない。backendを
    debuggerで追う場合に使う。いずれの経路でも、失敗時はdirectoryを作らない。
    """
    target = Path(directory)
    if target.exists():
        raise FileExistsError(f"output directory already exists: {target}")
    target.parent.mkdir(parents=True, exist_ok=True)
    # stagingの作成と破棄は親が持つ。子がtimeoutで止められても残骸を残さない。
    staging = Path(tempfile.mkdtemp(prefix=".typedsolid-", dir=target.parent))
    cache = None if cache_dir is None else str(cache_dir)
    try:
        if isolated:
            manifest = worker.run(
                _export_staged, model.to_json(), str(staging), cache,
                timeout_s=timeout_s, progress=progress,
            )
        else:
            manifest = _export_staged(progress or _silent, model.to_json(), str(staging), cache)
        # mkdirは存在判定と作成を一体で行う。競合時に他者の出力を置換しない。
        target.mkdir()
        for path in staging.iterdir():
            path.rename(target / path.name)
        return manifest
    finally:
        shutil.rmtree(staging, ignore_errors=True)
