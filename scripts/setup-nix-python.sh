#!/usr/bin/env bash
# shell.nix内で実行する。lock検証済みwheelをproject専用venvに配置する。
set -euo pipefail
cd "$(dirname "$0")/.."
: "${TYPEDSOLID_DYNAMIC_LINKER:?Run this script inside nix-shell}"
uv venv --allow-existing --python python3.12 .venv
# ELF interpreterの変更がuv cacheへ波及しないようcopyで配置する。
uv pip sync --python .venv/bin/python --require-hashes --only-binary :all: \
  --link-mode copy requirements-dev.lock
# 動的リンクwheelの場合だけNixのloaderへ接続する。静的リンクwheelは変更しない。
if readelf -l .venv/bin/maturin | grep -q INTERP; then
  patchelf --set-interpreter "$TYPEDSOLID_DYNAMIC_LINKER" .venv/bin/maturin
fi
