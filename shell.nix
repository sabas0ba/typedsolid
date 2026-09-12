# dotfiles 2d272319883e37b7213b2910b1036fad751238cc のflake.lockと同じnixpkgsを使う。
# Linux x86_64 / Python 3.12 / Rust 1.97.1。通常のLinux手順はdevelopment.mdを参照。
let
  pkgs = import (builtins.fetchTarball {
    url = "https://github.com/NixOS/nixpkgs/archive/597283ad8aa0b331c788e97c4c262d58877074ef.tar.gz";
    sha256 = "sha256-J+Bx1Z6Oeoj2FgnBhRMKyUhhtDoOpTgXYaVLZpDjW4A=";
  }) { system = "x86_64-linux"; };
  componentLines = builtins.filter (line: line != "") (
    pkgs.lib.splitString "\n" (builtins.readFile ./scripts/rust-linux-x86_64.sha256)
  );
  rust = pkgs.stdenv.mkDerivation {
    pname = "typedsolid-rust";
    version = "1.97.1";
    srcs = map (
      line:
      let
        fields = pkgs.lib.splitString " " line;
      in
      pkgs.fetchurl {
        url = "https://static.rust-lang.org/dist/2026-07-16/${builtins.elemAt fields 0}-1.97.1-x86_64-unknown-linux-gnu.tar.xz";
        sha256 = builtins.elemAt fields 1;
      }
    ) componentLines;
    sourceRoot = ".";
    nativeBuildInputs = [ pkgs.autoPatchelfHook ];
    buildInputs = [
      pkgs.stdenv.cc.cc.lib
      pkgs.zlib
    ];
    dontConfigure = true;
    dontBuild = true;
    dontStrip = true;
    installPhase = ''
      runHook preInstall
      for component in *-1.97.1-x86_64-unknown-linux-gnu; do
        bash "$component/install.sh" --prefix="$out" --disable-ldconfig
      done
      runHook postInstall
    '';
  };
in
pkgs.mkShell {
  packages = [
    rust
    pkgs.python312
    pkgs.uv
    pkgs.gnumake
    pkgs.git
    pkgs.curl
    pkgs.patchelf
    pkgs.stdenv.cc
    pkgs.ripgrep
  ];
  # manylinux wheelが同梱しないC++/OpenGL/X11の共有library。
  LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath [
    pkgs.stdenv.cc.cc.lib
    pkgs.zlib
    pkgs.expat
    pkgs.libGL
    pkgs.libGLU
    pkgs.libx11
    pkgs.libxext
    pkgs.libxrender
    pkgs.libsm
    pkgs.libice
  ];
  TYPEDSOLID_DYNAMIC_LINKER = pkgs.stdenv.cc.bintools.dynamicLinker;
  UV_CACHE_DIR = toString ./.work/uv-cache;
  UV_PYTHON_DOWNLOADS = "never";
  shellHook = ''
    mkdir -p .work/tmp
    export TMPDIR="$PWD/.work/tmp"
    export CARGO_HOME="$PWD/.work/cargo"
    echo "TypedSolid: Python 3.12 / Rust 1.97.1 (dotfiles pinned nixpkgs)"
  '';
}
