"""外部のSTL/STEPを読み、IRを介さずに最終形状のruleを評価する。

既存の筐体と比較するための入口である。評価するのは`Rule::VOXEL`の4 rule
(final_wall_thickness、neck_section、closed_cavity、support_free) と、製造法ごとのrule
(UV樹脂のresin_drain、resin_suction、切削のmilling_reach、milling_corner、射出成形の
mold_undercut、mold_thick_wall、mold_draft) だけで、IRの意味を要する検査 (keepout、掃引、
分解、ネジ、snap fit、コネクタ開口) は行わない。製造案は1つで、既定はFDMである。座標の
単位はmmとみなす。STEPはCadQueryで読み、STLへ三角形分割してから同じ経路で評価する。
第三者のファイルはリポジトリに置かず、利用者が取得して渡す。
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
from .model import DEFAULT_PLAN, Direction, Fdm, ManufacturingPlan, Milling, Molding, Orientation, Policy, Resin
from .viewer import MeshPart, stl_part, write_viewer

__all__ = ["inspect_file", "main"]

# STEPを三角形分割するときの弦誤差の、voxel間隔に対する比。格子より十分細かくする。
CHORD_RATIO = 0.25
# figuresのdirectoryに書く3D viewer。
VIEWER_FILE = "viewer.html"
# 製造法と、その特性を与えるoption (属性名)。fdmだけはFdm()の既定値で補い、他は既定値を持たない。
PROCESSES = {
    "fdm": (Fdm, ("min_wall_mm", "overhang_angle_deg", "bridge_max_mm")),
    "resin": (Resin, ("min_wall_mm", "overhang_angle_deg", "bridge_max_mm", "min_drain_mm")),
    "milling": (Milling, ("min_wall_mm", "tool_diameter_mm", "tool_length_mm")),
    "molding": (Molding, ("min_wall_mm", "max_wall_mm")),
}
# failは無いが、評価していないcheck (抜き勾配など) がある場合の終了コード。
EXIT_NOT_EVALUATED = 3


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
    plan: ManufacturingPlan = DEFAULT_PLAN,
) -> list[dict]:
    """ファイルを読み、最終形状のruleの結果を返す。meshが閉じていなければ例外を送出する。

    肉厚、支持、UV樹脂のruleはplan (製造案) の製造法、特性、姿勢で判定する。
    figuresを与えると、概観と検出箇所の断面図と、3D viewer (viewer.html) をそのdirectoryへ書く。
    viewerは判定に使ったSTLの三角形をそのまま描き、間引かない。
    """
    return _inspect(Path(path), policy or Policy(), plan, target, figures)[0]


def _inspect(
    path: Path, policy: Policy, plan: ManufacturingPlan, target: str | None, figures: str | Path | None,
) -> tuple[list[dict], MeshPart | None]:
    """判定の結果と、viewerを書いた場合はその部品を返す。"""
    target = target or path.stem
    stl = _stl_bytes(path, policy.voxel_mm)
    policy_json = json.dumps(asdict(policy))
    plan_json = json.dumps(_plan_dict(plan))
    checks = json.loads(_native.evaluate_stl_voxels(stl, target, policy_json, plan_json))
    if figures is None:
        return checks, None
    write_stl_figures(stl, policy_json, plan_json, target, checks, figures)
    part = stl_part(target, stl)
    write_viewer([part], checks, Path(figures) / VIEWER_FILE, title=f"TypedSolid: {target} ({path.name})")
    return checks, part


def _plan_dict(plan: ManufacturingPlan) -> dict:
    """製造案のJSON。省略した材料は書かない。"""
    return {key: value for key, value in asdict(plan).items() if value is not None}


def main(argv: list[str] | None = None) -> int:
    defaults = Policy()
    fdm = Fdm()
    parser = argparse.ArgumentParser(description="外部のSTL/STEPに最終形状のruleを適用する")
    parser.add_argument("path", type=Path, help="評価するSTL (binary又はASCII) 又はSTEP。単位はmm")
    parser.add_argument("--voxel-mm", type=float, default=defaults.voxel_mm)
    parser.add_argument("--min-neck-mm", type=float, default=defaults.min_neck_mm)
    parser.add_argument(
        "--process", choices=tuple(PROCESSES), default="fdm",
        help="製造法。fdm以外は、その製造法の特性のoptionをすべて与える",
    )
    parser.add_argument(
        "--build-direction", choices=get_args(Direction), default="plus_z",
        help="製造案の姿勢のup。積層方向、切削で工具を下ろす側、成形で型を開く軸",
    )
    parser.add_argument("--min-wall-mm", type=float, help=f"fdmの既定値は{fdm.min_wall_mm}")
    parser.add_argument("--overhang-angle-deg", type=float, help=f"fdmとresin。fdmの既定値は{fdm.overhang_angle_deg}")
    parser.add_argument("--bridge-max-mm", type=float, help=f"fdmとresin。fdmの既定値は{fdm.bridge_max_mm}")
    parser.add_argument("--min-drain-mm", type=float, help="resin: 排出路に要求する最小幅")
    parser.add_argument("--tool-diameter-mm", type=float, help="milling: 工具の直径")
    parser.add_argument("--tool-length-mm", type=float, help="milling: 素材の上面から工具が届く深さ")
    parser.add_argument(
        "--setup", action="append", choices=get_args(Direction), default=[],
        help="milling: --build-directionに加える段取りの向き。繰り返して与える",
    )
    parser.add_argument("--max-wall-mm", type=float, help="molding: 許す最大の肉厚")
    parser.add_argument("--json", action="store_true", help="結果をJSONで出力する")
    parser.add_argument("--figures", type=Path, help="概観と検出箇所の断面図と、3D viewerを書くdirectory")
    args = parser.parse_args(argv)
    policy = replace(defaults, voxel_mm=args.voxel_mm, min_neck_mm=args.min_neck_mm)
    process = _process(parser, args)
    plan = ManufacturingPlan(args.process, process, "command line", Orientation(args.build_direction))
    checks, part = _inspect(args.path, policy, plan, None, args.figures)
    if part is not None:
        # 標準出力は結果 (--jsonではJSON) だけとし、viewerの大きさは標準エラーに書く。
        viewer = args.figures / VIEWER_FILE
        size_mb = viewer.stat().st_size / 1e6
        print(f"viewer: {viewer} ({part.triangle_count()} triangles, {size_mb:.1f} MB)", file=sys.stderr)
    if args.json:
        print(json.dumps(checks, ensure_ascii=False, indent=2))
    else:
        for check in checks:
            print(f"{check['rule']}\t{check['status']}\t{check['message']}")
    statuses = {check["status"] for check in checks}
    if "fail" in statuses:
        return 1
    return EXIT_NOT_EVALUATED if "not_evaluated" in statuses else 0


def _option(name: str) -> str:
    return "--" + name.replace("_", "-")


def _process(parser: argparse.ArgumentParser, args: argparse.Namespace) -> Fdm | Resin | Milling | Molding:
    """optionから製造法の特性を作る。他の製造法のoptionを与えた場合と、値が欠ける場合は拒否する。"""
    kind, names = PROCESSES[args.process]
    others = {name for _, used in PROCESSES.values() for name in used} - set(names)
    extra = [_option(name) for name in sorted(others) if getattr(args, name) is not None]
    if args.setup and args.process != "milling":
        extra.append("--setup")
    if extra:
        parser.error(f"{', '.join(extra)} do not apply to --process {args.process}")
    values = {name: getattr(args, name) for name in names}
    if args.process == "fdm":
        return Fdm(**{name: value for name, value in values.items() if value is not None})
    missing = [_option(name) for name, value in values.items() if value is None]
    if missing:
        parser.error(f"--process {args.process} requires {', '.join(missing)}")
    if args.process == "milling":
        values["additional_setups"] = tuple(Orientation(up) for up in args.setup)
    return kind(**values)


if __name__ == "__main__":
    sys.exit(main())
