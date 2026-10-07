#!/usr/bin/env bash
# 3D viewerのJavaScriptをnode:testで検査するためのNode.jsを.work/nodeに置く。
# nodejs.orgの公式tarballを取得し、SHASUMS256.txtの値と照合する。npmのpackageは入れない。
set -euo pipefail
cd "$(dirname "$0")/.."
if [[ "$(uname -sm)" != "Linux x86_64" ]]; then
  echo 'This bootstrap supports Linux x86_64; install Node.js elsewhere and put node on PATH.' >&2
  exit 1
fi
# 版と値はhttps://nodejs.org/dist/v24.21.0/SHASUMS256.txt (release keyで署名済み) による。
version=v24.21.0
digest=fd8e59d5a511510f6a298afb548f18c7d2b1be404d8b4a27d94fbe49f56cb2d6
archive="node-$version-linux-x64"
file=".work/downloads/$archive.tar.xz"
mkdir -p .work/downloads
if [[ ! -f "$file" ]]; then
  curl --fail --location --retry 2 --max-time 300 \
    "https://nodejs.org/dist/$version/$archive.tar.xz" -o "$file.part"
  mv "$file.part" "$file"
fi
echo "$digest  $file" | sha256sum --check
rm -rf .work/node
mkdir -p .work/node
tar -xJf "$file" -C .work/node --strip-components=1
.work/node/bin/node --version
