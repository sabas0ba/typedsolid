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
make example       # .work/board-tray に出力。既存なら拒否。cacheは.work/cache
make clean-cache   # 生成cacheを消す
```

Rust unit testはcoreを対象とする。PyO3 bindingはPythonからnative moduleを読み込む統合testで検証し、libpythonをリンクするembedding用Rust testとは分離する。bindingもworkspace全体のclippy検査に含める。

`tests/test_enclosure_fixtures.py`は、開口を持つ筐体に既知の欠陥を1つずつ入れ、落ちるruleの集合が宣言と完全に一致することを見る。個々のruleは単純形状のtestが検証しており、ここで確かめるのは実形状での成立と過検出の不在である。詳細は [ロードマップ](roadmap.md) を参照する。

別の出力先を使う場合:

```bash
.venv/bin/python examples/board_tray.py --output .work/board-tray-v2
```

出力は`board_tray.stl`、`board_tray.step`、`model.json`、`report.json`。STLは1部品分。作例は60×40×20 mm、底・壁厚2 mmのトレイで、収める基板は [部品catalog](catalog.md) のRaspberry Pi Pico 2である。catalogの取付穴位置にφ5 mmの支持pad 4個を立て、φ1.6 mmのネジ下穴で貫く。Pico 2の取付穴はφ2.1でM2相当である。基板の51×21 mmの領域をz=4 mmに確保し、+Zへ抜けることを検査する。支持面へ接触させるため下面のclearanceだけを0とし、他の面は0.5 mmを確保する。データシートは部品高さを与えないため、確保する高さ5 mmは作例側で決めている。ネジ・インサートの仕様と蓋は含まないため、完成した実機筐体ではない。

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

全座標はmm。`Feature(operation="cut")`は全Addを結合した後に差し引く。`hole`は`cut`のcylinder、`boss`は`add`のcylinderを作るhelperで、いずれも直径で指定する。応力・熱を必須にする場合は`Policy.required`に該当ルールを追加するが、現在は未実装なのでexportが拒否される。

分解手順は`Assembly`に書く。記述した位置を組立完了の状態とし、stepを上から順に実行する。

```python
from typedsolid import Assembly, Model, Move, Step

model = Model(
    parts=(tray, lid),
    assembly=Assembly(
        steps=(Step("open_lid", ("lid",), (Move("minus_x", 4.0), Move("plus_z"))),),
        fit_clearance_mm=0.2,
    ),
)
```

工具・ケーブル・コネクタの通り道と、keepoutの取り出しは`Sweep`に書く。`after_step`を与えると、そのstepを終えた状態で評価する。

```python
from typedsolid import Box, Cylinder, Sweep

sweeps = (
    # 蓋を閉じたまま、右壁の開口へUSBプラグを差し込む。
    Sweep("usb_plug", "minus_x", shape=Box((42, 11, 6), (55, 19, 9)), distance_mm=10.0),
    # 蓋を外した後、ネジ受けの真上からドライバを抜き差しする。
    Sweep("driver", "plus_z", shape=Cylinder("z", (10, 15), 1.5, (8, 12)), after_step="open_lid"),
    # 蓋を外した後、基板を上へ抜く。
    Sweep("pcb_out", "plus_z", keepout="pcb", after_step="open_lid"),
)
```

`Keepout(access=("plus_z",))`は、組立完了の状態で外まで抜く`Sweep("pcb_plus_z", "plus_z", keepout="pcb")`の省略形である。

`Move("plus_z")`は残っている部品の外まで動かす。数値を与えるとその距離だけ動かす。`disassembly_path`は各区間の干渉を、`disassembly_separation`は最後の方向へ動かし続けて外れることを検査する。

収める基板が [部品catalog](catalog.md) にある場合は、外形と取付穴を手で書かずにcatalogから取れる。

```python
from typedsolid import board

pico = board("raspberry_pi_pico_2")
pads = pico.bosses((0.0, 4.0), 5.0, origin=(4.5, 9.5, 4.0))
```

最終肉厚・接続部断面・閉空洞・支持の要否はIRをrasterizeして判定する。格子は`Policy.voxel_mm` (既定0.2 mm) で、細かいほど正確になり、cell数は3乗で増える。作例 (60×40×20 mm) では0.2 mmで約5秒、0.4 mmで約0.6秒かかる。大きなモデルを扱う場合は格子を粗くする。

印刷姿勢は`Policy.build_direction` (既定`plus_z`)、支持なしで許す傾斜は`overhang_angle_deg` (既定45)、渡せる未支持区間の長さは`bridge_max_mm` (既定5.0) で指定する。

## slicerとの突合

`support_free`は幾何のみに基づく近似であり、ノズル径、層厚、冷却、材料を含まない。実機で用いる場合は、出力したSTLをslicerへ読み込み、同じ印刷姿勢でサポート生成の要否を比較する。

```bash
make example
# .work/board-tray/board_tray.stl をslicerで開き、build_directionと同じ向きに置く
# サポート自動生成を有効にし、生成箇所がreport.jsonのsupport_freeと矛盾しないか確認する
```

slicerは開発依存に含めず、CIでも実行しない。突合は手元での確認とし、判定が食い違った場合は`overhang_angle_deg`と`bridge_max_mm`を実機の条件に合わせる。

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

## 打ち切りと再開

`export`は既定で子processにbackendを隔離し、`timeout_s` (既定600秒) を超えると`WorkerTimeout`を送出する。呼び出し元のscriptは子で読み込み直さないため、`if __name__ == "__main__"`による保護は不要で、notebookや標準入力からも呼べる。出力先は作られず、stagingも残らない。進行は`progress`に渡した関数へ、経過秒付きの段階名で届く。

```python
import sys
from typedsolid.cadquery import export

export(model, ".work/out", timeout_s=120, cache_dir=".work/cache",
       progress=lambda stage: print(stage, file=sys.stderr))
```

```
[    0.1 s] part board_tray: building shape
[    0.3 s] keepout and interference checks
[    0.3 s] part board_tray: voxel evaluation
[    7.5 s] part board_tray: writing stl
```

`cache_dir`を与えると、部品ごとの形状とvoxel評価を保存し、再実行では変わっていない部品を再利用する。打ち切られた実行も、完了した部品の分だけ次回が短くなる。作例は`.work/cache`を使い、cacheなしで約12秒、cacheありで約4.5秒かかる。残りは親子2つのprocessでのcadquery読み込みである。

```bash
make clean-cache   # .work/cache/typedsolid-v1 の項目だけを消す
```

cacheのkeyはbackendのsourceとnative moduleを含むため、コードを変更すると古い項目は使われずに残る。容量が気になれば消す。書き込みは一時fileのrenameで行い、読み出し時にdigestを照合するため、中断で壊れた項目は使われない。

`export(..., isolated=False)`は同一processで実行し、`timeout_s`を使わない。debuggerでbackendを追う場合と、`unittest.mock.patch`でbackendを差し替えるtestに使う。

## 作業データ

`.work/`、`.venv/`、`target/`はgit管理外。生成済み出力を消さずに再評価する場合は別名の出力ディレクトリを使用する。開発環境全体を再構築する前に、`.work/`内の必要な作例を退避する。
