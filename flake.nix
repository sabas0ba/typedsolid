{
  description = "TypedSolid: 意味モデルと検査付きソリッドモデリングの開発環境";

  inputs = {
    # 入力はrevisionで固定する。branch名による参照はflake.lockが無い環境で
    # 取得結果が変動するため使用しない。
    #
    # `github:` ではなくgit protocolで参照する。前者はGitHubのtarball APIを使い、
    # APIへの到達が制限された環境で取得できない。git参照はflake.lockに同じrevisionと
    # narHashを記録するため、固定の強さは変わらない。
    nixpkgs.url = "git+https://github.com/NixOS/nixpkgs?rev=597283ad8aa0b331c788e97c4c262d58877074ef&shallow=1"; # nixos-26.05

    # rust-toolchain.tomlの指定どおりにRust toolchainを構成する。取得元はRust公式の
    # rustup manifestであり、scripts/bootstrap-rust.shと同じ配布物を使う。
    #
    # rust-analyzer-srcはfenixがnightlyのrust-analyzerを構成するためだけに持つ入力で、
    # 本projectは使用しない。取り除いてflake.lockに不要な入力を残さない。
    fenix = {
      url = "git+https://github.com/nix-community/fenix?rev=9efa138447c5773995d98d9aafa9eba4982aceab&shallow=1"; # 2026-09-08
      inputs.nixpkgs.follows = "nixpkgs";
      inputs.rust-analyzer-src.follows = "";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      fenix,
    }:
    let
      # 検証対象はdocs/development.mdに合わせてLinux x86_64に限定する。
      # 他のsystemは検証していないため提供しない。
      system = "x86_64-linux";
      pkgs = import nixpkgs {
        inherit system;
        config = { };
        overlays = [ ];
      };

      # rust-toolchain.tomlが版とcomponentの単一情報源である。sha256はtoolchain全体の
      # 内容を固定する。toolchainを変更した場合は同時に更新する。
      rustToolchain = fenix.packages.${system}.fromToolchainFile {
        file = ./rust-toolchain.toml;
        sha256 = "sha256-A1abGIbOtcBSdrUMhDGrER3pRM1hQP4fp9gh3Y4PKc8=";
      };
    in
    {
      devShells.${system}.default = import ./nix/devshell.nix { inherit pkgs rustToolchain; };

      formatter.${system} = pkgs.nixfmt;

      # `nix flake check` が評価する。開発シェルが構成できることを確認する。
      checks.${system}.devshell = self.devShells.${system}.default;
    };
}
