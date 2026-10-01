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


def render(output: Path, defects_only: bool) -> list[Path]:
    """fixtureごとに検査し、図をoutput/<fixture>/へ書く。書いたfileのpathを返す。"""
    written = []
    for factory in ((() if defects_only else (baseline,)) + DEFECTS):
        model_json = factory().to_json()
        checks = json.loads(_native.evaluate_voxels(model_json))
        checks += json.loads(_native.evaluate_connectors(model_json))
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
    args = parser.parse_args(argv)
    for path in render(args.output, args.defects_only):
        print(path)
    return 0


if __name__ == "__main__":
    sys.exit(main())
