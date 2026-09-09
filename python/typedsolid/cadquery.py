"""軸平行boxのunion-minus-cutsをCadQueryに変換する実験的backend。"""

from __future__ import annotations

from dataclasses import dataclass
import hashlib
from importlib.metadata import version
from itertools import combinations
import json
import math
from pathlib import Path
import tempfile
from typing import Any

import cadquery as cq

from . import _native
from .model import Model

# OCCTの境界演算誤差を吸収する接触判定の絶対体積許容差。単位はmm³。
VOLUME_TOLERANCE = 1e-7
# 立体が空でないとみなす下限体積。単位はmm³。実装上の最小box寸法0.001 mmの立方体は
# 1e-9 mm³であり、接触判定の許容差を流用すると正当な最小形状を無効と判定するため、
# 目的の異なる閾値として独立に定義する。
EMPTY_VOLUME_TOLERANCE = 1e-12
GEOMETRY_RULES = (
    "valid_solid", "single_solid", "keepout_clearance", "access_clearance", "part_interference",
)


def _box(bounds: dict) -> cq.Solid:
    lo, hi = bounds["min"], bounds["max"]
    return cq.Solid.makeBox(*(hi[i] - lo[i] for i in range(3)), pnt=cq.Vector(*lo))


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


def build(model: Model) -> Build:
    """検査結果を返す。CAD例外はfailに変換し、形状は出力しない。"""
    model_json = model.to_json()
    data = json.loads(model_json)
    report = json.loads(_native.preflight(model_json))
    checks = report["checks"]
    shapes: dict[str, cq.Shape] = {}
    geometry: list[dict] = []
    try:
        for part in data["parts"]:
            additives = [_box(f["bounds"]) for f in part["features"] if f["operation"] == "add"]
            shape: cq.Shape = additives[0]
            for additive in additives[1:]:
                shape = shape.fuse(additive)
            for feature in part["features"]:
                if feature["operation"] == "cut":
                    shape = shape.cut(_box(feature["bounds"]))
            shape = shape.clean()
            solids = shape.Solids()
            volume = sum(abs(s.Volume()) for s in solids)
            valid = bool(solids) and shape.isValid() and math.isfinite(volume) and volume > EMPTY_VOLUME_TOLERANCE
            geometry.append(_check("valid_solid", part["id"], valid, f"valid={shape.isValid()}, volume={volume:.9g} mm³"))
            geometry.append(_check("single_solid", part["id"], len(solids) == 1, f"final solid count: {len(solids)}"))
            shapes[part["id"]] = shape

        # 全部品の最高点まで延長することで、上方に別部品がある場合も検査する。
        z_exit = max(f["bounds"]["max"][2] for p in data["parts"] for f in p["features"] if f["operation"] == "add") + 2.0
        for keepout in data["keepouts"]:
            margin = keepout["clearance_mm"]
            bounds = {"min": [v - margin for v in keepout["bounds"]["min"]], "max": [v + margin for v in keepout["bounds"]["max"]]}
            volume_shape = _box(bounds)
            for part_id, shape in shapes.items():
                overlap = _overlap(shape, volume_shape)
                geometry.append(_check("keepout_clearance", f"{keepout['id']}/{part_id}", overlap <= VOLUME_TOLERANCE, f"overlap: {overlap:.9g} mm³; clearance: {margin} mm"))
            if keepout["access"] == "plus_z":
                corridor = {"min": bounds["min"], "max": [bounds["max"][0], bounds["max"][1], max(z_exit, bounds["max"][2])]}
                sweep = _box(corridor)
                for part_id, shape in shapes.items():
                    overlap = _overlap(shape, sweep)
                    geometry.append(_check("access_clearance", f"{keepout['id']}/{part_id}", overlap <= VOLUME_TOLERANCE, f"+Z straight access overlap: {overlap:.9g} mm³; exit z={corridor['max'][2]} mm"))
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
    return Build(shapes, report, model_json)


def export(model: Model, directory: str | Path) -> dict:
    """モデルを再buildして検査し、新規ディレクトリへ出力する。既存成果物は上書きしない。"""
    result = build(model)
    if not result.export_allowed:
        failures = [c for c in result.report["checks"] if c["status"] == "fail" or (c["rule"] in result.report["required"] and c["status"] != "pass")]
        raise ValueError("export blocked: " + json.dumps(failures, ensure_ascii=False))
    target = Path(directory)
    if target.exists():
        raise FileExistsError(f"output directory already exists: {target}")
    target.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".typedsolid-", dir=target.parent) as staging:
        root = Path(staging)
        files = {}
        for part_id, shape in result.shapes.items():
            # 公開Buildを変更してもexportには流用しない。必ず上で検査したshapeを使う。
            for suffix in ("stl", "step"):
                path = root / f"{part_id}.{suffix}"
                cq.exporters.export(shape, str(path), tolerance=0.01, angularTolerance=0.1)
                if path.stat().st_size == 0:
                    raise ValueError(f"empty export: {part_id}.{suffix}")
                files[path.name] = hashlib.sha256(path.read_bytes()).hexdigest()
        # 保存bytesとdigestを同一の値から得る。model.jsonの再hashで照合できるようにする。
        model_bytes = (result.model_json + "\n").encode("utf-8")
        (root / "model.json").write_bytes(model_bytes)
        manifest = {
            "schema_version": 1, "units": "mm", "cadquery": version("cadquery"),
            "cadquery_ocp": version("cadquery-ocp"),
            "model_sha256": hashlib.sha256(model_bytes).hexdigest(),
            "files_sha256": files, "report": result.report,
            "notice": "Only listed checks were evaluated. This is not a printability or structural safety certification.",
        }
        (root / "report.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
        # mkdirは存在判定と作成を一体で行う。競合時に他者の出力を置換しない。
        target.mkdir()
        for path in root.iterdir():
            path.rename(target / path.name)
    return manifest
