"""Python側はモデルの組み立てを担当し、検証はRustに委譲する。"""

from dataclasses import asdict, dataclass, field
import json
from typing import Any, Literal

from . import _native

Vec2 = tuple[float, float]
Vec3 = tuple[float, float, float]
Axis = Literal["x", "y", "z"]
Direction = Literal["minus_x", "plus_x", "minus_y", "plus_y", "minus_z", "plus_z"]
Role = Literal["base", "wall", "mount", "rib", "generic"]
Operation = Literal["add", "cut"]
Rule = Literal[
    "feature_thickness", "valid_solid", "single_solid", "keepout_clearance",
    "access_clearance", "part_interference", "mesh_manifold", "mesh_volume",
    "final_wall_thickness", "neck_section", "closed_cavity",
    "support_free", "strength", "thermal",
]


@dataclass(frozen=True)
class Box:
    min: Vec3
    max: Vec3
    kind: Literal["box"] = "box"


@dataclass(frozen=True)
class Cylinder:
    """軸平行の円柱。centerは軸に垂直な平面上の2座標、spanは軸方向の範囲。"""

    axis: Axis
    center: Vec2
    radius: float
    span: Vec2
    kind: Literal["cylinder"] = "cylinder"


Shape = Box | Cylinder


@dataclass(frozen=True)
class Feature:
    id: str
    shape: Shape
    role: Role = "generic"
    operation: Operation = "add"


def hole(id: str, axis: Axis, center: Vec2, diameter: float, span: Vec2, role: Role = "mount") -> Feature:
    """軸平行の穴。全Addを結合した後に差し引く。"""
    return Feature(id, Cylinder(axis, center, diameter / 2.0, span), role, "cut")


def boss(id: str, axis: Axis, center: Vec2, diameter: float, span: Vec2, role: Role = "mount") -> Feature:
    """軸平行の円柱台座。ネジ穴を設ける場合はholeと組み合わせる。"""
    return Feature(id, Cylinder(axis, center, diameter / 2.0, span), role, "add")


@dataclass(frozen=True)
class Part:
    id: str
    features: tuple[Feature, ...]


@dataclass(frozen=True)
class Clearance:
    """面ごとのclearance。指定のない面にはdefaultを適用する。"""

    default: float = 0.5
    minus_x: float | None = None
    plus_x: float | None = None
    minus_y: float | None = None
    plus_y: float | None = None
    minus_z: float | None = None
    plus_z: float | None = None


@dataclass(frozen=True)
class Keepout:
    id: str
    shape: Shape
    clearance_mm: Clearance = field(default_factory=Clearance)
    access: tuple[Direction, ...] = ()


@dataclass(frozen=True)
class Policy:
    min_feature_mm: float = 1.2
    # tessellationの弦誤差を吸収する、出力STLとsolidの体積差の相対許容量。
    mesh_volume_tolerance: float = 0.01
    # 最終形状をrasterizeする格子の間隔。細かいほど正確になり、cell数は3乗で増える。
    voxel_mm: float = 0.2
    # 最終形状に要求する最小肉厚。primitive寸法 (min_feature_mm) とは別に指定する。
    min_wall_mm: float = 1.2
    # 接続部に要求する最小断面。
    min_neck_mm: float = 1.2
    # 印刷時に上となる方向。層はこの軸に沿って積む。
    build_direction: Direction = "plus_z"
    # 支持なしで許す、印刷方向に対する最大傾斜角。単位は度。
    overhang_angle_deg: float = 45.0
    # 両端が支持された未支持区間の許容長。
    bridge_max_mm: float = 5.0
    required: tuple[Rule, ...] = (
        "feature_thickness", "valid_solid", "single_solid", "keepout_clearance",
        "access_clearance", "part_interference",
        "final_wall_thickness", "neck_section", "closed_cavity", "support_free",
    )


def _without_unset(value: Any) -> Any:
    """未指定の面別clearanceをJSONから除く。Noneを持つfieldは他に無い。

    asdictはtupleをtupleのまま返すため、listと同じに扱わないと入れ子を降りられない。
    """
    if isinstance(value, dict):
        return {key: _without_unset(item) for key, item in value.items() if item is not None}
    if isinstance(value, (list, tuple)):
        return [_without_unset(item) for item in value]
    return value


@dataclass(frozen=True)
class Model:
    parts: tuple[Part, ...]
    keepouts: tuple[Keepout, ...] = ()
    policy: Policy = field(default_factory=Policy)
    schema_version: int = 2
    units: Literal["mm"] = "mm"

    def to_json(self) -> str:
        return _native.normalize_model(json.dumps(_without_unset(asdict(self)), allow_nan=False))

    def preflight(self) -> dict:
        return json.loads(_native.preflight(self.to_json()))
