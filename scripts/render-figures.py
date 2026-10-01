"""欠陥fixtureの検出箇所の断面図を、Rust coreの検査だけから描く。CadQueryを使わない。

PRのCI artifactと、docs/assets/figuresに置く説明図を同じ手順で作る。
"""

import argparse
import json
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from examples.defects import DEFECTS, baseline
from typedsolid import _native
from typedsolid.figures import write_model_figures


def _checks(model_json: str) -> list[dict]:
    """CadQueryを使わずに求まる検査。最終形状の4 ruleとコネクタ開口の整合である。"""
    return json.loads(_native.evaluate_voxels(model_json)) + json.loads(_native.evaluate_connectors(model_json))


def _factories(defects_only: bool):
    return (() if defects_only else (baseline,)) + DEFECTS


def summary(defects_only: bool) -> str:
    """failしたcheckと検出箇所の表。CIのjob summaryに書く。"""
    lines = [
        "## Detected locations (defect fixtures)",
        "",
        "| fixture | rule | target | locations | largest location (min - max, mm) |",
        "| --- | --- | --- | --- | --- |",
    ]
    for factory in _factories(defects_only):
        for check in _checks(factory().to_json()):
            if check["status"] != "fail":
                continue
            locations = check.get("locations", [])
            largest = (
                " - ".join("(" + ", ".join(f"{v:.2f}" for v in locations[0][key]) + ")" for key in ("min", "max"))
                if locations else ""
            )
            lines.append(f"| {factory.__name__} | {check['rule']} | {check['target']} | {len(locations)} | {largest} |")
    return "\n".join(lines) + "\n"


def render(output: Path, defects_only: bool) -> list[Path]:
    """fixtureごとに検査し、図をoutput/<fixture>/へ書く。書いたfileのpathを返す。"""
    written = []
    for factory in _factories(defects_only):
        model_json = factory().to_json()
        checks = _checks(model_json)
        directory = output / factory.__name__
        for name in write_model_figures(model_json, checks, directory):
            if defects_only and name.endswith("--overview.svg"):
                (directory / name).unlink()
                continue
            written.append(directory / name)
    return written


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--output", type=Path, default=Path(".work/figures"))
    parser.add_argument("--defects-only", action="store_true", help="概観を書かず、検出箇所の図だけを書く")
    parser.add_argument("--summary", type=Path, help="検出箇所の表をMarkdownで追記するfile")
    args = parser.parse_args(argv)
    for path in render(args.output, args.defects_only):
        print(path)
    if args.summary is not None:
        with args.summary.open("a", encoding="utf-8") as handle:
            handle.write(summary(args.defects_only))
    return 0


if __name__ == "__main__":
    sys.exit(main())
