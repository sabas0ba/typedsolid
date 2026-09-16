"""Python側はモデルの組み立てを担当し、検証はRustに委譲する。"""

from dataclasses import asdict, dataclass, field
import json
from typing import Literal

from . import _native

Vec3 = tuple[float, float, float]
Rule = Literal[
    "feature_thickness", "valid_solid", "single_solid", "keepout_clearance",
    "access_clearance", "part_interference", "mesh_manifold", "mesh_volume",
    "final_wall_thickness", "support_free", "strength", "thermal",
]


@dataclass(frozen=True)
class Box:
    min: Vec3
    max: Vec3


@dataclass(frozen=True)
class Feature:
    id: str
    bounds: Box
    role: Literal["base", "wall", "mount", "rib", "generic"] = "generic"
    operation: Literal["add", "cut"] = "add"


@dataclass(frozen=True)
class Part:
    id: str
    features: tuple[Feature, ...]


@dataclass(frozen=True)
class Keepout:
    id: str
    bounds: Box
    clearance_mm: float = 0.5
    access: Literal["plus_z"] | None = None


@dataclass(frozen=True)
class Policy:
    min_feature_mm: float = 1.2
    # tessellationの弦誤差を吸収する、出力STLとsolidの体積差の相対許容量。
    mesh_volume_tolerance: float = 0.01
    required: tuple[Rule, ...] = (
        "feature_thickness", "valid_solid", "single_solid", "keepout_clearance",
        "access_clearance", "part_interference",
    )


@dataclass(frozen=True)
class Model:
    parts: tuple[Part, ...]
    keepouts: tuple[Keepout, ...] = ()
    policy: Policy = field(default_factory=Policy)
    schema_version: int = 1
    units: Literal["mm"] = "mm"

    def to_json(self) -> str:
        return _native.normalize_model(json.dumps(asdict(self), allow_nan=False))

    def preflight(self) -> dict:
        return json.loads(_native.preflight(self.to_json()))
