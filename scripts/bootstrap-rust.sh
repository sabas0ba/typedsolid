#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
if [[ "$(uname -sm)" != "Linux x86_64" ]]; then
  echo 'This bootstrap supports Linux x86_64; use rustup with rust-toolchain.toml elsewhere.' >&2
  exit 1
fi
mkdir -p .work/downloads .work/toolchain
while read -r component digest; do
  archive="$component-1.97.1-x86_64-unknown-linux-gnu"
  file=".work/downloads/$archive.tar.xz"
  if [[ ! -f "$file" ]]; then
    curl --fail --location --retry 2 --max-time 300 \
      "https://static.rust-lang.org/dist/2026-07-16/$archive.tar.xz" -o "$file.part"
    mv "$file.part" "$file"
  fi
  echo "$digest  $file" | sha256sum --check
  tar -xJf "$file" -C .work/downloads
  env -i PATH=/usr/bin:/bin bash ".work/downloads/$archive/install.sh" \
    --prefix="$PWD/.work/toolchain" --disable-ldconfig
done < scripts/rust-linux-x86_64.sha256
