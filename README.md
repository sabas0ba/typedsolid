# TypedSolid

意味情報と設計・製造ルールを持つソリッドモデリング基盤。Rust coreとPython APIを持ち、初期の形状生成にCadQueryを使用します。

現在は実験段階です。軸平行boxと円柱による部品生成、最終solid数、基板用keepout、6方向アクセス、部品間干渉に加え、IRからrasterizeしたvoxel上で最終肉厚・接続部断面・閉空洞・サポート要否を検査し、1 STL＝1部品で出力します。基板の外形と取付穴は公式資料に基づく部品catalogから取れます。応力・熱解析は未対応で、検査レポートにも未評価として残します。

- [設計と検査範囲](docs/design.md)
- [部品catalog](docs/catalog.md)
- [セットアップ・作例・テスト](docs/development.md)
- [ロードマップ](docs/roadmap.md)
- [依存調査と固定方針](docs/dependencies.md)

Linux x86_64 / CPython 3.12を初期検証対象とします。Apache-2.0 license。
