# 依存調査

調査日: 2026-09-05。利用者の承認に基づき、project専用環境に導入した。lock対象はRust 21 package、Python 45 distribution、合計66件。OSV APIの版指定照合で該当advisoryは0件、全版の公開日時はcutoffの2026-08-29以前である。これは未知の脆弱性や配布物の安全性を保証するものではない。

## 直接依存とtoolchain

| 対象 | 固定版 | 目的・保守・publishの確認 |
|---|---|---|
| Rust | 1.97.1 | Rust公式release、2026-07-16。1.97.0等のLLVM誤compile修正を含む。公式manifestのSHA-256で各componentを固定 |
| PyO3 | 0.29.2 | 2026-08-05。Python native binding。複数contributorの継続開発。現行maintainerの権限人数は未確定 |
| serde | 1.0.228 | 2025-09-27。型付きIRのserialization。保守は特定maintainerへの集中がある。境界をJSON schemaに限定し、代替移行可能にする |
| serde_json | 1.0.149 | 2026-01-06。JSON入出力。serde系の保守集中を共有する |
| maturin | 1.15.0 | 2026-08-24。PyPI表示のmaintainerは2名。Trusted Publishingとrelease workflowのprovenanceを確認 |
| CadQuery | 2.8.0 | 2026-06-21。PyPI表示のmaintainerは3名。Trusted Publishingは未使用。coreから分離したoptional backendとして採用 |
| actions/checkout | v7.0.1 / `3d3c42e5aac5ba805825da76410c181273ba90b1` | GitHub公式。2026-07-20公開、releaseとcommitを照合。credential永続化を無効化 |

CadQueryは13の通常依存を宣言し、OCCT/VTK、数値計算、trame系の推移依存を持つ。Python lock全45件のうちmaturin以外はこのbackend側である。代表的な経路は`cadquery → trame → trame-server → wslink → aiohttp → multidict`で5段ある。coreだけの利用時にCadQueryをimportしないテストを設ける。solver、GUI server、viewer追加依存は導入しない。trame等はCadQueryの通常依存として含まれるが、このprojectからserverを起動しない。

Rustの直接外部依存は3件、推移を含め21件。手動FFIや独自JSON parserはメモリ安全性・検証負担が増すため採用しない。serdeの代替は将来検討可能だが、初期段階では追加実装の複雑化を避ける。

## セキュリティ照合

- [RustSec: PyO3](https://rustsec.org/packages/pyo3.html)、[RUSTSEC-2026-0176](https://rustsec.org/advisories/RUSTSEC-2026-0176.html)、[RUSTSEC-2026-0177](https://rustsec.org/advisories/RUSTSEC-2026-0177.html)を確認。iteratorの範囲外readとclosureのSync bound欠落は0.29.0で修正されており、採用版は修正後。
- [NVD CVE-2024-9979](https://nvd.nist.gov/vuln/detail/CVE-2024-9979)と[GitHub Advisory](https://github.com/advisories/GHSA-6jgw-rgmm-7cv6)の弱参照問題は旧版が対象。採用版への該当はOSV照合でも検出されなかった。
- [CadQuery security](https://github.com/CadQuery/cadquery/security)、[maturin security](https://github.com/PyO3/maturin/security)には公開advisoryの表示がなかった。NVDでmaturin関連として見つかる配布packageの推移依存問題は、製品名だけで採用版への該当と判断しない。maturin v1.15.0のCargo.lockのringは0.17.14。
- 全lock対象を[OSV API](https://google.github.io/osv.dev/api/)へecosystem/name/version指定で照合した。照合失敗時には成功として扱わず、audit commandを失敗させる。
- 上記直接依存についてmaintainer侵害・悪意あるpublish・install時情報窃取を検索した範囲では、採用を停止すべき該当報告は確認できなかった。全推移依存の各maintainerアカウントや2FA状態を独立に検証したわけではない。公開情報で確認できない権限情報は推測しない。

出典: [Rust 1.97.1](https://blog.rust-lang.org/2026/07/16/Rust-1.97.1/)、[PyO3 contributing](https://pyo3.rs/v0.29.2/contributing.html)、[serde](https://github.com/serde-rs/serde)、[maturin配布情報](https://pypi.org/project/maturin/1.15.0/)、[CadQuery配布情報](https://pypi.org/project/cadquery/2.8.0/)、[checkout releases](https://github.com/actions/checkout/releases/tag/v7.0.1)。

## 固定と実行範囲

Pythonは完全版指定・配布hash付きlockとwheel限定installを用い、sdist build hookの自動実行を避ける。Rust crateはCargo.lockのchecksumで固定し、build script/proc macroはコンパイル時に実行される。公式Rust installerはcomponent hashを確認してから実行する。

auditはinstall前の調査であり、通常のローカルtestとCI unit testにnetwork依存を持ち込まない。OSVのデータは時間経過で変わるため更新前に再実行する。maturin wheel内部の全Rust依存、OCCT/VTK等の全native依存、runner image、既存Python/uv/OSの全componentをこの66件照合が網羅するものではない。

依存更新は手動で承認を得て行う。公開後7日未満の版を通常更新として採用せず、自動更新は有効化しない。既存toolchain、Python、uvはセットアップ前提であり、OS全体のbit-for-bit再現は未対応。

## M1開発時の再照合（2026-09-13）

既存のRust 21 package / Python 45 distributionを変更せず、`scripts/audit-dependencies.py`でOSVと公開日時を再照合した。全66件に該当advisoryはなく、既存cutoff（2026-08-29）を満たした。照合結果は専用コンテナの`.work/dependency-audit.json`に保存した。

CadQuery 2.8.0とmaturin 1.15.0の[PyPI配布情報](https://pypi.org/project/cadquery/2.8.0/)・[maturin配布情報](https://pypi.org/project/maturin/1.15.0/)、両projectの[CadQuery security](https://github.com/CadQuery/cadquery/security)・[maturin security](https://github.com/PyO3/maturin/security)も再確認した。依存数と保守集中の評価は上記の既存調査を引き継ぐ。今回の確認範囲で導入停止に該当する公開情報は確認できなかったが、全推移依存のmaintainer権限や未知の侵害を保証しない。

開発環境にはdotfilesの固定nixpkgs `597283ad8aa0b331c788e97c4c262d58877074ef`を使用する。Nixのsource hashを固定し、Rust公式archiveは既存hashを再利用する。Python 3.12、uv、compiler、CAD用共有library等の開発環境依存をこの固定nixpkgsから供給し、アプリのlockは変更しない。この66件のOSV照合はNix store全体やnative library全体の脆弱性調査を代替しない。
