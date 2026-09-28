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
    "support_free", "disassembly_path", "disassembly_separation", "fastener_fit", "snap_fit",
    "strength", "thermal",
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
    # Model.materialsのid。snap fitを持つ部品では必須。
    material: str | None = None


@dataclass(frozen=True)
class Material:
    """材料。値は利用者が与え、sourceに出典 (データシートの版、試験記録など) を書く。

    allowable_strainは曲げの許容ひずみ (無次元) で、snap fitを持つ部品の材料では必須。
    """

    id: str
    name: str
    source: str
    allowable_strain: float | None = None


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
    """確保領域。accessは組立完了の状態で外まで抜く掃引の省略形である。

    to_jsonはaccessの各方向を、idが`<keepout>_<direction>`のSweepへ展開する。
    分解stepの後で抜く場合や距離を限る場合は、Sweepを直接書く。

    keepoutは分解経路の障害物になる。attached_toを与えると、その部品と一緒に
    分解stepで動き、以降の状態から除かれる。省くと外部に固定され、最後まで残る。
    """

    id: str
    shape: Shape
    clearance_mm: Clearance = field(default_factory=Clearance)
    access: tuple[Direction, ...] = ()
    attached_to: str | None = None


@dataclass(frozen=True)
class Sweep:
    """工具・ケーブル・コネクタ、またはkeepoutを取り出す際に通る領域。

    形状はshapeで直接与えるか、keepoutのidを与えてそのclearance込みのboxを使う。
    形状は包絡であり、指の入る余地などの余裕を含める。after_stepを与えると
    そのstepを終えた状態で評価し、取り外した部品は障害物にならない。
    """

    id: str
    direction: Direction
    shape: Shape | None = None
    keepout: str | None = None
    distance_mm: float | Literal["exit"] = "exit"
    after_step: str | None = None


@dataclass(frozen=True)
class Move:
    """軸平行の直線区間。"exit"は残っている部品のAABBの外まで動かす。最後の区間に限る。"""

    direction: Direction
    distance_mm: float | Literal["exit"] = "exit"


@dataclass(frozen=True)
class Step:
    """分解の1手順。partsを一体として経路に沿って動かし、以降の状態から除く。"""

    id: str
    parts: tuple[str, ...]
    path: tuple[Move, ...]
    # Noneならassemblyの値を使う。
    fit_clearance_mm: float | None = None


@dataclass(frozen=True)
class Assembly:
    """記述した部品位置を組立完了の状態とし、stepsを順に実行して分解する。

    組立順序は分解の逆とする。どのstepにも現れない部品は最後まで残る。
    fit_clearance_mmは移動方向に垂直な向きに要求する隙間で、0は硬い干渉だけを見る。
    """

    steps: tuple[Step, ...] = ()
    fit_clearance_mm: float = 0.0


@dataclass(frozen=True)
class Screw:
    """ネジの寸法。lengthは頭の座面から先端まで、majorはねじ部の外径、headは頭の外径。"""

    length_mm: float
    major_mm: float
    head_mm: float


@dataclass(frozen=True)
class SelfTapping:
    """印刷した下穴へ直接ねじ込む。"""

    pilot_mm: float
    kind: Literal["self_tapping"] = "self_tapping"


@dataclass(frozen=True)
class Insert:
    """熱圧入インサート。baseの境目と面一に埋まる。holeは圧入前の下穴の径。"""

    hole_mm: float
    length_mm: float
    kind: Literal["insert"] = "insert"


@dataclass(frozen=True)
class Fastener:
    """ネジ固定。clampの部品をbaseの部品へ締める。組立完了の状態で評価する。

    directionは締め込む向き (頭から先端へ)、centerは軸に垂直な面上の座標で
    Cylinderと同じ順に並ぶ。seat_mmは頭が当たる面、joint_mmはclampとbaseの境目の
    軸方向の座標である。clampが空の場合、締める対象は部品として記述されていない。
    through_mmはclampに開ける貫通穴の径で、頭の座面の内径になる。
    """

    id: str
    base: str
    clamp: tuple[str, ...]
    direction: Direction
    center: Vec2
    seat_mm: float
    joint_mm: float
    screw: Screw
    through_mm: float
    anchor: SelfTapping | Insert
    min_engagement_mm: float
    min_boss_wall_mm: float


@dataclass(frozen=True)
class SnapFit:
    """矩形断面の片持ち梁によるsnap fit。

    beamとhookはpartのadd boxのfeature id。length_directionは梁の根元から先端への向き、
    deflectionは外すときにフックが動く向きで、deflection_mmだけたわませると外れる。
    stepはpartかmateの一方を動かし、この結合を外す分解stepである。
    """

    id: str
    part: str
    beam: str
    hook: str
    length_direction: Direction
    deflection: Direction
    deflection_mm: float
    mate: str
    step: str


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
        "disassembly_path", "disassembly_separation", "fastener_fit", "snap_fit",
    )


def _without_unset(value: Any) -> Any:
    """未指定の値をJSONから除く。対象は面別clearance、stepのfit_clearance_mm、Sweepの省略可能な値、
    部品のmaterial、材料のallowable_strain、keepoutのattached_toである。

    asdictはtupleをtupleのまま返すため、listと同じに扱わないと入れ子を降りられない。
    """
    if isinstance(value, dict):
        return {key: _without_unset(item) for key, item in value.items() if item is not None}
    if isinstance(value, (list, tuple)):
        return [_without_unset(item) for item in value]
    return value


def _expand_access(data: dict) -> dict:
    """keepoutのaccessをSweepへ展開する。Rust側がv3のaccessを昇格する規則と同じ順とする。"""
    derived = []
    for keepout in data["keepouts"]:
        for direction in keepout.pop("access"):
            derived.append({
                "id": f"{keepout['id']}_{direction}", "keepout": keepout["id"],
                "direction": direction, "distance_mm": "exit",
            })
    return {**data, "sweeps": derived + list(data["sweeps"])}


@dataclass(frozen=True)
class Model:
    parts: tuple[Part, ...]
    keepouts: tuple[Keepout, ...] = ()
    policy: Policy = field(default_factory=Policy)
    schema_version: int = 7
    units: Literal["mm"] = "mm"
    # 後から加えたfieldは末尾に置き、既存の位置引数 (parts, keepouts, policy) を保つ。
    assembly: Assembly = field(default_factory=Assembly)
    sweeps: tuple[Sweep, ...] = ()
    fasteners: tuple[Fastener, ...] = ()
    materials: tuple[Material, ...] = ()
    snap_fits: tuple[SnapFit, ...] = ()

    def to_json(self) -> str:
        data = _expand_access(_without_unset(asdict(self)))
        return _native.normalize_model(json.dumps(data, allow_nan=False))

    def preflight(self) -> dict:
        return json.loads(_native.preflight(self.to_json()))
