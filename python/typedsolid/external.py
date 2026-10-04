"""外部のSTL/STEPを読み、IRを介さずに最終形状のruleを評価する。

既存の筐体と比較するための入口である。評価するのは`Rule::VOXEL`の4 rule
(final_wall_thickness、neck_section、closed_cavity、support_free) だけで、
IRの意味を要する検査 (keepout、掃引、分解、ネジ、snap fit、コネクタ開口) は
行わない。座標の単位はmmとみなす。STEPはCadQueryで読み、STLへ三角形分割してから
同じ経路で評価する。第三者のファイルはリポジトリに置かず、利用者が取得して渡す。
"""

import argparse
from dataclasses import asdict, replace
import json
from pathlib import Path
import sys
import tempfile
from typing import get_args

from . import _native
from .figures import write_stl_figures
from .model import Direction, Policy

__all__ = ["inspect_file", "main"]

# STEPを三角形分割するときの弦誤差の、voxel間隔に対する比。格子より十分細かくする。
CHORD_RATIO = 0.25


def _stl_bytes(path: Path, pitch: float) -> bytes:
    suffix = path.suffix.lower()
    if suffix == ".stl":
        return path.read_bytes()
    if suffix not in (".step", ".stp"):
        raise ValueError(f"{path}: 対応する形式は.stl、.step、.stpである")
    import cadquery as cq

    shape = cq.importers.importStep(str(path))
    with tempfile.TemporaryDirectory() as directory:
        target = Path(directory) / "shape.stl"
        cq.exporters.export(shape, str(target), tolerance=pitch * CHORD_RATIO, angularTolerance=0.1)
        return target.read_bytes()


def inspect_file(
    path: str | Path,
    policy: Policy | None = None,
    target: str | None = None,
    figures: str | Path | None = None,
) -> list[dict]:
    """ファイルを読み、4つの最終形状ruleの結果を返す。meshが閉じていなければ例外を送出する。

    figuresを与えると、概観と検出箇所の断面図をそのdirectoryへ書く。
    """
    path = Path(path)
    policy = policy or Policy()
    target = target or path.stem
    stl = _stl_bytes(path, policy.voxel_mm)
    policy_json = json.dumps(asdict(policy))
    checks = json.loads(_native.evaluate_stl_voxels(stl, target, policy_json))
    if figures is not None:
        write_stl_figures(stl, policy_json, target, checks, figures)
    return checks


def main(argv: list[str] | None = None) -> int:
    defaults = Policy()
    parser = argparse.ArgumentParser(description="外部のSTL/STEPに最終形状のruleを適用する")
    parser.add_argument("path", type=Path, help="評価するSTL (binary又はASCII) 又はSTEP。単位はmm")
    parser.add_argument("--voxel-mm", type=float, default=defaults.voxel_mm)
    parser.add_argument("--min-wall-mm", type=float, default=defaults.min_wall_mm)
    parser.add_argument("--min-neck-mm", type=float, default=defaults.min_neck_mm)
    parser.add_argument("--build-direction", choices=get_args(Direction), default=defaults.build_direction)
    parser.add_argument("--overhang-angle-deg", type=float, default=defaults.overhang_angle_deg)
    parser.add_argument("--bridge-max-mm", type=float, default=defaults.bridge_max_mm)
    parser.add_argument("--json", action="store_true", help="結果をJSONで出力する")
    parser.add_argument("--figures", type=Path, help="概観と検出箇所の断面図を書くdirectory")
    args = parser.parse_args(argv)
    policy = replace(
        defaults, voxel_mm=args.voxel_mm, min_wall_mm=args.min_wall_mm, min_neck_mm=args.min_neck_mm,
        build_direction=args.build_direction, overhang_angle_deg=args.overhang_angle_deg,
        bridge_max_mm=args.bridge_max_mm,
    )
    checks = inspect_file(args.path, policy, figures=args.figures)
    if args.json:
        print(json.dumps(checks, ensure_ascii=False, indent=2))
    else:
        for check in checks:
            print(f"{check['rule']}\t{check['status']}\t{check['message']}")
    return 0 if all(check["status"] == "pass" for check in checks) else 1


if __name__ == "__main__":
    sys.exit(main())
