# 開発・実行

## 対象環境

初期検証対象はLinux x86_64 / CPython 3.12、Rust 1.97.1。Rust、Python packageはproject専用ディレクトリに配置する。Nixは必須ではなく、後述の2経路のいずれでも同じ`make check`を実行できる。Dockerを用いた環境と他OSの検証は後続項目である。

## セットアップ (Nix)

`nix develop`はrust-toolchain.tomlの指定どおりのRust toolchain、Python 3.12、uv、C compiler、poppler-utilsを提供する。`.envrc`があるためdirenvでも入れる。

```bash
nix develop
uv venv --python python3.12 .venv
uv pip sync --python .venv/bin/python --require-hashes --only-binary :all: requirements-dev.lock
make check
```

flake inputは`flake.lock`にrevisionとnarHashで固定する。Rust toolchainはfenixがRust公式のrustup manifestから構成し、`flake.nix`の`sha256`が内容を固定する。取得元と版は`scripts/bootstrap-rust.sh`と同一である。

開発シェル内で`scripts/bootstrap-rust.sh`を実行しない。`.work/toolchain`が残っている場合、Makefileはそちらを優先するため、不要なら削除する。

`.venv`は作成時のinterpreterへの参照を持つ。開発シェル内で作成した`.venv`はNixのinterpreterを指し、シェル外ではcadquery-ocpが必要とする共有ライブラリを解決できない。経路を切り替える場合は`.venv`を削除して作り直す。

提供するsystemはx86_64-linuxのみである。他のsystemはtoolchainのhashを検証していないため定義しない。

## セットアップ (Nixを使わない場合)

Python 3.12、uv、C compiler、make、curl、tar、xzを用意する。uvは環境の既存ツールを使用し、この手順はglobal installを行わない。Python依存の正確な版・hashは`requirements-dev.lock`、Rust依存は`Cargo.lock`に固定する。

Linux x86_64でRustが未導入の場合、次を実行する。公式配布archiveのSHA-256を照合し、`.work/toolchain`にだけ導入する。再実行時はdownload済みarchiveを検証して再利用する。

```bash
bash scripts/bootstrap-rust.sh
uv venv --python python3.12 .venv
uv pip sync --python .venv/bin/python --require-hashes --only-binary :all: requirements-dev.lock
make check
```

既存のrustupを使う場合はbootstrapを省略できる。`rust-toolchain.toml`の固定版を使用する。`make`は`.work/toolchain/bin`があれば優先する。Pythonだけの代替ルール実装や、native moduleがない場合の自動fallbackは用意しない。

## 検証

```bash
make rust-check    # fmt / clippy / Rust tests
make python-test   # native module build / Python integration tests
make check         # 両方
make example       # .work/board-tray に出力。既存なら拒否
```

Rust unit testはcoreを対象とする。PyO3 bindingはPythonからnative moduleを読み込む統合testで検証し、libpythonをリンクするembedding用Rust testとは分離する。bindingもworkspace全体のclippy検査に含める。

別の出力先を使う場合:

```bash
.venv/bin/python examples/board_tray.py --output .work/board-tray-v2
```

出力は`board_tray.stl`、`board_tray.step`、`model.json`、`report.json`。STLは1部品分。作例は60×40×20 mm、底・壁厚2 mm、φ6 mmの支持pad 4個と、それを貫くφ2.5 mmのネジ下穴を持つトレイである。44×24×3 mmの説明用基板領域をz=4 mmに確保し、+Zへ抜けることを検査する。支持面へ接触させるため下面のclearanceだけを0とし、他の面は0.5 mmを確保する。壁との間隔は6 mm以上ある。実基板寸法、ネジ・インサートの仕様、蓋は含まないため、完成した実機筐体ではない。

![最終形状と基板確保領域](assets/board-tray.svg)

濃色の線が出力対象、緑色がSTLに含まれない基板確保領域である。previewは最終CadQuery shapeにOCCTの隠線処理を適用して生成する。+Zアクセスは検査結果としてreport.jsonに記録する。

```bash
.venv/bin/python scripts/render-example.py --output docs/assets/board-tray.svg
```

作例を変更した場合はSVGを再生成する。ラスタ画像は変換ツールを開発依存に加えることになるため用意しない。

API使用例:

```python
from typedsolid import Box, Feature, Model, Part, hole
from typedsolid.cadquery import export

model = Model(parts=(Part("plate", (
    Feature("base", Box((0, 0, 0), (40, 30, 2)), role="base"),
    hole("bore", "z", (20, 15), 3.0, (-1, 3)),
)),))
print(model.preflight())
export(model, ".work/plate")
```

全座標はmm。`Feature(operation="cut")`は全Addを結合した後に差し引く。`hole`は`cut`のcylinder、`boss`は`add`のcylinderを作るhelperで、いずれも直径で指定する。サポート不要や応力・熱を必須にする場合は`Policy.required`に該当ルールを追加するが、現在は未実装なのでexportが拒否される。

最終肉厚・接続部断面・閉空洞はIRをrasterizeして判定する。格子は`Policy.voxel_mm` (既定0.2 mm) で、細かいほど正確になり、cell数は3乗で増える。作例 (60×40×20 mm) では0.2 mmで約4秒、0.4 mmで約0.5秒かかる。大きなモデルを扱う場合は格子を粗くする。

## 依存更新

更新前に [依存調査](dependencies.md) を行う。承認後、cutoffを公開後7日以上となる日付に指定する。lockfileを削除して検査を回避しない。

```bash
uv pip compile --no-config --exclude-newer 2026-08-29 --generate-hashes --only-binary :all: requirements-dev.in -o requirements-dev.lock
make audit
make check
```

Rustのtransitive依存更新も`Cargo.lock`を差分レビューし、auditに含める。外部advisory照合はnetworkを必要とするため、通常のunit testと分離する。

## CI方針

PRはLinux上の軽量core検査、main更新・手動実行はCadQuery統合テストを含む検査を行う。CadQueryはVTK等の大きな依存を持つため、通常のPRごとに取得・統合buildを強制しない。初回PRはローカルの統合テスト結果を添付する。Windows/macOS jobと自動publishは追加しない。

## 作業データ

`.work/`、`.venv/`、`target/`はgit管理外。生成済み出力を消さずに再評価する場合は別名の出力ディレクトリを使用する。開発環境全体を再構築する前に、`.work/`内の必要な作例を退避する。
