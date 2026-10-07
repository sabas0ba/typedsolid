"""部品間の欠陥fixtureを検査し、検出箇所の投影図を描く。CadQueryを使う。

docs/assets/projectionsに置く説明図を作る。CIのintegration jobが再生成して照合する。
"""

import argparse
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from examples.assembly_defects import ASSEMBLY_DEFECTS
from typedsolid.cadquery import build, write_projection_figures


def render(output: Path) -> list[Path]:
    """fixtureごとに検査し、failしたcheckの投影図をoutput/<fixture>/へ書く。書いたfileのpathを返す。"""
    written = []
    for factory, _ in ASSEMBLY_DEFECTS:
        directory = output / factory.__name__
        names = write_projection_figures(build(factory()), directory, overview=False)
        written += [directory / name for name in names]
    return written


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--output", type=Path, default=Path(".work/projections"))
    args = parser.parse_args(argv)
    for path in render(args.output):
        print(path)
    return 0


if __name__ == "__main__":
    sys.exit(main())
