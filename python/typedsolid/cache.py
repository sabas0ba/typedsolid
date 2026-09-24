"""部品単位の生成結果を保存し、中断後の再実行で済んだ部品を飛ばす。

keyは入力と、それを処理する実装の両方から作る。backendのsourceやnative moduleが
変わるとkeyが変わり、古い結果は参照されない。書き込みは同じdirectoryの一時fileを
renameして行い、中断しても書きかけの項目を残さない。読み出し時はdigestを照合し、
一致しない項目は捨てて作り直す。

cacheは結果の再利用だけを担い、判定には関与しない。cacheから読んだ形状も、
新たに作った形状と同じ検査を通る。
"""

from __future__ import annotations

import argparse
from functools import cache
import hashlib
from importlib.metadata import version
import json
import os
from pathlib import Path
import tempfile

from . import _native

FORMAT = 1
_SUFFIX = ".bin"
_TEMPORARY_PREFIX = ".tmp-"
_DIGEST_BYTES = hashlib.sha256().digest_size


@cache
def backend_fingerprint() -> str:
    """結果に影響する実装の識別子。backendのsource、native module、CAD kernelの版から作る。"""
    digest = hashlib.sha256()
    for path in (Path(__file__).with_name("cadquery.py"), Path(_native.__file__)):
        digest.update(path.read_bytes())
    for package in ("cadquery", "cadquery-ocp"):
        digest.update(f"{package}={version(package)}".encode())
    return digest.hexdigest()


def _directory(root: str | Path) -> Path:
    return Path(root) / f"typedsolid-v{FORMAT}"


class Cache:
    """root直下の専用directoryに項目を置く。root自体の他のfileには触れない。"""

    def __init__(self, root: str | Path) -> None:
        self.directory = _directory(root)

    def key(self, kind: str, payload: object) -> str:
        text = json.dumps(
            {"format": FORMAT, "kind": kind, "backend": backend_fingerprint(), "payload": payload},
            sort_keys=True,
            separators=(",", ":"),
            allow_nan=False,
        )
        return hashlib.sha256(text.encode()).hexdigest()

    def _path(self, key: str) -> Path:
        return self.directory / f"{key}{_SUFFIX}"

    def get(self, key: str) -> bytes | None:
        """項目を返す。無い場合と、digestが一致しない場合はNone。後者は削除する。"""
        path = self._path(key)
        try:
            blob = path.read_bytes()
        except FileNotFoundError:
            return None
        digest, content = blob[:_DIGEST_BYTES], blob[_DIGEST_BYTES:]
        if hashlib.sha256(content).digest() != digest:
            path.unlink(missing_ok=True)
            return None
        return content

    def put(self, key: str, content: bytes) -> None:
        self.directory.mkdir(parents=True, exist_ok=True)
        descriptor, temporary = tempfile.mkstemp(prefix=_TEMPORARY_PREFIX, dir=self.directory)
        try:
            with os.fdopen(descriptor, "wb") as stream:
                stream.write(hashlib.sha256(content).digest() + content)
            os.replace(temporary, self._path(key))
        except BaseException:
            Path(temporary).unlink(missing_ok=True)
            raise


def _owned(path: Path) -> bool:
    """このmoduleが作ったfileか。keyは64桁の16進数である。"""
    if not path.is_file():
        return False
    if path.name.startswith(_TEMPORARY_PREFIX):
        return True
    stem = path.name.removesuffix(_SUFFIX)
    return path.name.endswith(_SUFFIX) and len(stem) == 64 and all(c in "0123456789abcdef" for c in stem)


def clear(root: str | Path) -> int:
    """cacheの項目を消し、消した数を返す。名前の規則に合わないfileは残す。"""
    directory = _directory(root)
    if not directory.is_dir():
        return 0
    removed = 0
    for path in directory.iterdir():
        if _owned(path):
            path.unlink()
            removed += 1
    # 他のfileが残っていれば空でないため削除されない。
    try:
        directory.rmdir()
    except OSError:
        pass
    return removed


def main() -> None:
    parser = argparse.ArgumentParser(description="typedsolidの生成cacheを操作する")
    commands = parser.add_subparsers(dest="command", required=True)
    remove = commands.add_parser("clear", help="cacheの項目を消す")
    remove.add_argument("root", type=Path)
    arguments = parser.parse_args()
    if arguments.command == "clear":
        print(f"removed {clear(arguments.root)} cache entries from {_directory(arguments.root)}")


if __name__ == "__main__":
    main()
