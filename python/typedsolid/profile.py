"""材料・印刷機・設計値をまとめた設計profile。

値はすべて利用者が与える。材料の許容ひずみや印刷機のはめ合い隙間は機種・材料の
ロットや設定で変わり、根拠のない既定値を組み込まないためである。材料はIRの
`Material`であり、Model.materialsに渡して部品へ割り当てる。印刷機と設計値は
IRに現れず、Policy、FDMの製造案、Assemblyを組み立てる入力として使う。
"""

from dataclasses import dataclass, replace
from typing import Any

from .model import Assembly, Direction, Fdm, ManufacturingPlan, Material, Orientation, Policy, Step


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
        base = Policy(min_feature_mm=self.min_feature_mm, min_neck_mm=self.min_neck_mm)
        return replace(base, **overrides)

    def plan(self, id: str = "fdm", up: Direction | None = None, turn_deg: int = 0) -> ManufacturingPlan:
        """profileの印刷機と材料によるFDMの製造案。upを省くとprofileの積層方向とする。"""
        return ManufacturingPlan(
            id,
            Fdm(self.min_wall_mm, self.overhang_angle_deg, self.bridge_max_mm),
            f"profile: printer {self.printer.name}, material {self.material.name}",
            Orientation(up or self.build_direction, turn_deg),
            self.material.id,
        )

    def assembly(self, steps: tuple[Step, ...]) -> Assembly:
        """印刷機のはめ合い隙間を既定値とするAssembly。stepごとの上書きはStepで行う。"""
        return Assembly(steps, self.printer.fit_clearance_mm)
