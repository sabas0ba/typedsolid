"""材料・印刷機・設計値をまとめた設計profile。

値はすべて利用者が与える。材料の許容ひずみや印刷機のはめ合い隙間は機種・材料の
ロットや設定で変わり、根拠のない既定値を組み込まないためである。profileは
IRに現れず、PolicyとAssemblyを組み立てる入力として使う。
"""

from dataclasses import dataclass, replace
from typing import Any

from .model import Assembly, Direction, Policy, Step


@dataclass(frozen=True)
class Material:
    """sourceは値の出典 (データシートの版、社内試験の記録など)。

    allowable_strainはsnap fitの許容たわみを求めるための曲げの許容ひずみ (無次元)。
    snap fitを使わない場合はNoneでよい。
    """

    name: str
    source: str
    allowable_strain: float | None = None


@dataclass(frozen=True)
class Printer:
    """fit_clearance_mmは実機で確かめた、はめ合いに要する片側の隙間。"""

    name: str
    nozzle_mm: float
    layer_mm: float
    fit_clearance_mm: float


@dataclass(frozen=True)
class Profile:
    material: Material
    printer: Printer
    min_wall_mm: float
    min_neck_mm: float
    min_feature_mm: float
    overhang_angle_deg: float
    bridge_max_mm: float
    build_direction: Direction

    def policy(self, **overrides: Any) -> Policy:
        """profileの値を持つPolicy。voxel_mm、requiredなどprofileにない値はoverridesで与える。"""
        base = Policy(
            min_feature_mm=self.min_feature_mm,
            min_wall_mm=self.min_wall_mm,
            min_neck_mm=self.min_neck_mm,
            build_direction=self.build_direction,
            overhang_angle_deg=self.overhang_angle_deg,
            bridge_max_mm=self.bridge_max_mm,
        )
        return replace(base, **overrides)

    def assembly(self, steps: tuple[Step, ...]) -> Assembly:
        """印刷機のはめ合い隙間を既定値とするAssembly。stepごとの上書きはStepで行う。"""
        return Assembly(steps, self.printer.fit_clearance_mm)
