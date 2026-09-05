# TypedSolid 開発規約

設計方針は [docs/design.md](docs/design.md)、実行方法は [docs/development.md](docs/development.md)、開発順序は [docs/roadmap.md](docs/roadmap.md) を参照する。

- 日本語の簡潔な技術文書を使用する。READMEは概要と導線、詳細はdocs/に置く。
- 機能追加はfeature branchで行い、Conventional Commitsを使用する。mainへの直接変更・自動mergeはしない。
- `make check` を実行し、失敗・未実行項目をPR本文に明記する。
- Rustコアに意味モデルと判定方針を置く。PythonはAPI・外部CAD接続を担当する。
- CadQuery/OCCTのface番号を永続IDにしない。幾何演算後に不確かな意味対応を推測して保持しない。
- 未評価を合格に変えない。未対応の必須ルール・backend例外は出力を拒否する。
- 設計入力、STL、解析mesh、材料・境界条件を混同しない。単位と検査範囲を明記する。
- 依存導入・更新は事前承認を得て、既知脆弱性、publish経路の問題、保守状況、依存の規模を調査する。公開後7日以上の版を完全固定し、lockを更新する。
- 一時データとtoolchainは`.work/`、Python環境は`.venv/`に置く。system環境や他projectを変更しない。
- 生成STL/STEPをソース管理しない。説明用の小さなpreviewのみdocs/assets/に置いてよい。
- PRに作例の再現手順、利用者に可能になったこと、未対応項目を記載する。
