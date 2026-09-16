# `nix develop` および direnv が使用する開発シェル。
#
# 本ファイルが開発環境に導入するツールの単一情報源である。
#
# 版の固定はツールの種類ごとに正を分ける。Rust toolchainはrust-toolchain.toml、
# Rust依存はCargo.lock、Python依存はrequirements-dev.lockが正であり、本ファイルは
# それらを実行するための周辺ツールと、正に従って構成したtoolchainを置く。
{ pkgs, rustToolchain }:

pkgs.mkShellNoCC {
  name = "typedsolid";

  packages = [
    # rust-toolchain.tomlから構成したrustc/cargo/rustfmt/clippy。
    rustToolchain

    # PyO3のnative moduleをbuildするために必要。
    pkgs.stdenv.cc

    # Python環境はuvがrequirements-dev.lockから構成する。ここではinterpreterと
    # uv自体を提供する。lockのhash固定とwheel限定installは手順側で指定する。
    pkgs.python312
    pkgs.uv

    # 基本ツール
    pkgs.gnumake
    pkgs.git
    pkgs.curl
    pkgs.jq

    # 本ディレクトリのNixファイルを整形する。
    pkgs.nixfmt

    # 部品catalogの一次情報となるPDFから寸法を読む。
    pkgs.poppler-utils
  ];

  env = {
    # cadquery-ocpのmanylinux wheelはsystemの共有ライブラリを動的に開く。Nix環境には
    # 標準パスが無いため、wheelが要求するものを明示して与える。ここに挙げるのはwheelの
    # 実行に必要なruntime libraryであり、版固定の対象であるPython依存とは別である。
    LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath [
      pkgs.stdenv.cc.cc.lib
      pkgs.zlib
      pkgs.libGL
      pkgs.libglvnd
      pkgs.expat
      pkgs.fontconfig
      pkgs.freetype
      pkgs.libx11
      pkgs.libxext
      pkgs.libxrender
      pkgs.libsm
      pkgs.libice
    ];

    # ロケールによる挙動の差異を排除する。
    LC_ALL = "C.UTF-8";
  };

  shellHook = ''
    # Makefileと同じ位置にPython環境とcargoのstateを置く。
    if root="$(git rev-parse --show-toplevel 2>/dev/null)"; then
      export PYO3_PYTHON="$root/.venv/bin/python"
      export VIRTUAL_ENV="$root/.venv"
      export CARGO_HOME="$root/.work/cargo"
      export TMPDIR="$root/.work/tmp"
      export PATH="$root/.venv/bin:$PATH"
      mkdir -p "$root/.work/tmp"
    fi

    echo "typedsolid dev shell"
    echo "  rustc $(rustc --version 2>/dev/null | cut -d' ' -f2), python $(python3 --version 2>/dev/null | cut -d' ' -f2), uv $(uv --version 2>/dev/null | cut -d' ' -f2)"
    echo "  初回は docs/development.md の uv venv / uv pip sync を実行する"
  '';
}
