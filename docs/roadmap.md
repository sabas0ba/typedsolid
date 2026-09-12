# 開発順序

## M0: 意味モデルから検査付き出力まで

- Rust core、Python API、schema v1、CadQuery adapter
- 軸平行boxのunion-minus-cuts
- 最終solid数、形状妥当性、keepout、+Zアクセス、部品間干渉
- 成功・失敗作例、単体/統合test、CI、再現手順

実装済み。製造可能性全体を保証する段階ではない。

## M1: 実用筐体の構造と寸法

- 対応済み: +Z円柱、円柱Cutによる穴、boxと組み合わせた穴付きbossの作例、入力検証・CAD統合テスト、矩形開口付き筐体のCLI E2Eと出力STLプレビュー
- 残項目: 面取り、rib・bossの専用API、寸法型と座標変換
- 部品catalog、基板・コネクタ寸法と出典、支持面への接触指定
- 最終肉厚、接続部断面、slendernessの保守的な検査
- 閉空洞、印刷姿勢、overhang/bridgeの検査。slicerでの評価との突合
- STL meshのmanifold性・接続成分・退化三角形検査

合格条件: 開口を持つ筐体で、薄肉・折れやすい接続・孤立・支持不能箇所の既知の失敗fixtureを検出し、Tang Nano等の既存ケースへ適用して比較する。

## M2: 組立と保守アクセス

- 工具・ケーブル・コネクタ抜き差しの掃引領域
- lid取り外し等のassembly state、着脱順序、複数軸のアクセス
- snap fitと任意ネジ固定、材料・積層方向を含む設計profile
- backendをworker processへ隔離し、timeout・再開可能な生成を導入

## M3: 解析連携

- solver候補のライセンス・保守・検証実績を比較して選定
- 材料・荷重・拘束・発熱・熱境界の型付きschema
- 体積mesh、境界対応、mesh品質と収束テスト
- 既知解のある梁と熱伝導問題で検証後、筐体の応力・温度を評価

## 後続判断

Nix環境を追加し、digest固定NixイメージのPodman専用コンテナで検証する。Windows/macOSのwheel、API拡張、可視化は需要とCI費用を見て追加する。初期対応はLinux x86_64 / CPython 3.12。公開レジストリへのpublishとGitHub Pages公開は行わない。
