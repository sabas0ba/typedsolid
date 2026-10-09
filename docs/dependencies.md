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
| nixpkgs | `597283ad8aa0b331c788e97c4c262d58877074ef` (nixos-26.05) | 開発シェルの周辺ツール。NixOS公式。flake.lockにrevisionとnarHashで固定 |
| fenix | `9efa138447c5773995d98d9aafa9eba4982aceab` (2026-09-08) | 開発シェルのRust toolchain構成。nix-community org、maintainerは2名 |

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

## M1開発時の再照合 (2026-09-13)

PR #2 の作業時に、既存のRust 21 package / Python 45 distributionを変更せず、`scripts/audit-dependencies.py`でOSVと公開日時を再照合した。全66件に該当advisoryはなく、全版が既存cutoff (2026-08-29) を満たした。あわせて[CadQuery 2.8.0](https://pypi.org/project/cadquery/2.8.0/)・[maturin 1.15.0](https://pypi.org/project/maturin/1.15.0/)の配布情報と、[CadQuery security](https://github.com/CadQuery/cadquery/security)・[maturin security](https://github.com/PyO3/maturin/security)を再確認した。確認した範囲で導入停止に該当する公開情報はなかった。依存数と保守集中の評価は上記の調査を引き継ぐ。全推移依存のmaintainer権限や未知の侵害を保証するものではない。

照合の対象は2026-09-13時点のlockであり、以後`Cargo.lock`と`requirements-dev.lock`は変更していない。`requirements-figures.lock`は、CIのcore検査で図を再生成するためにmaturinだけを持つlockである。`requirements-dev.lock`と同じ`uv pip compile`の条件 (`--exclude-newer 2026-08-29`) で生成し、maturin 1.15.0の配布hashは`requirements-dev.lock`と一致する。新たな依存は加えていない。OSVのデータは時間経過で変わるため、この記録は現在の無該当を示さない。

## 3D viewerの検査 (2026-10-07)

3D viewer自体はJavaScriptのpackageを使わず、自作のWebGL 1と標準のAPIだけで書く。検査のために次の3件を利用者の承認を得て加えた。いずれもnpmのpackageを入れない。cutoffは2026-09-30とした。

| 対象 | 固定版 | 目的・確認 |
|---|---|---|
| Node.js | v24.21.0 (LTS、2026-09-08公開) | `node:test`でviewerの純粋な関数を検査する。`scripts/bootstrap-node.sh`がnodejs.orgの公式tarballを取得し、sha256 `fd8e59d5a511510f6a298afb548f18c7d2b1be404d8b4a27d94fbe49f56cb2d6`と照合する。値の出典である`SHASUMS256.txt`は、release key `5BE8A3F6C8A5C01D106C0AD820B1A390B168D356`による署名を検証した |
| Node.js (Nix) | nixpkgsの`nodejs_24` 24.18.0 | 既存のnixpkgs revisionのまま開発シェルに加えた。2026-07のsecurity releaseより前の版だが、修正対象 (HTTP/2、TLS、DNS、Permission Model等) は`node --test`でローカルのtestを走らせる用途に影響しない。nixpkgsの更新は別に扱う |
| debian:trixie-slim | `trixie-20260918-slim`、index digest `sha256:a99cfc517144bc59b1978475ec53b46ecabec7e43635402ee5b77cc54cd1b20a` | 撮影用imageの基礎。Docker公式image。2026-09-19公開 |
| chromium (Debian) | `154.0.8037.57-1~deb13u1` (snapshot.debian.org `debian-security/20260929T215738Z`) | 撮影用imageでviewerを描く。依存packageは`debian/20260930T203251Z`から入る。aptはimage内のDebianの鍵でRelease署名を検証する |

- nodejs.orgの配布とrelease経路について、侵害の報告は確認できなかった。v24.21.0より後のsecurity releaseは調査時点でなかった。
- Debian公式imageの現行tagについて、侵害の報告は確認できなかった。
- chromiumは、採用版より後の`154.0.8037.92-1~deb13u1` (2026-10-01、cutoffより後) で32件のCVEが修正されている ([OSV](https://osv.dev/list?ecosystem=Debian%3A13&q=chromium))。撮影は`docker run --network none`で行い、自分で書き出したローカルのHTMLだけを開くため、外部のweb contentが入る経路はない。この前提を外す用途 (外部のpageを開く等) に使わない。Debian security trackerのDSA番号は、調査環境から到達できず確認していない。
- security archiveのReleaseは7日で期限が切れるため、固定した時刻の内容を使うには`Check-Valid-Until: no`が要る。slim imageはCA証明書を持たないため、取得はDebianの既定と同じHTTPで行い、真正性はRelease署名とhashで確かめる。
- imageの大きさは約1.1 GB、buildは約2分である。CIではmain更新時のintegration jobだけで作る。

出典: [Node.js releases](https://nodejs.org/dist/index.json)、[v24.21.0 SHASUMS256.txt](https://nodejs.org/dist/v24.21.0/SHASUMS256.txt)、[Node.js release keys](https://github.com/nodejs/node#release-keys)、[Node.js vulnerability feed](https://nodejs.org/en/feed/vulnerability.xml)、[debian tags](https://hub.docker.com/_/debian/tags)、[snapshot.debian.org chromium](https://snapshot.debian.org/package/chromium/)。
