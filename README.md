# TypedSolid

意味情報と設計・製造ルールを持つソリッドモデリング基盤。Rust coreとPython APIを持ち、初期の形状生成にCadQueryを使用します。

現在は実験段階です。軸平行boxと円柱による部品生成、最終solid数、基板用keepout、6方向アクセス、部品間干渉に加え、IRからrasterizeしたvoxel上で最終肉厚・接続部断面・閉空洞・サポート要否を検査します。分解手順を記述すると、蓋や部品が干渉なく外れること、工具・ケーブル・基板がその状態で抜き差しできることも検査します。ネジ固定 (セルフタップ、熱圧入インサート) は、利用者が与えたネジの寸法と、貫通穴・座面・かかり長さ・先端の逃げ・bossの肉厚が実形状で整合するかを検査します。snap fit (矩形断面の片持ち梁) は、材料の許容ひずみ、積層方向、たわむ空間、保持、外れる経路を検査します。出力は1 STL＝1部品で、backendは子processで動き、timeoutで打ち切れます。基板の外形と取付穴は公式資料に基づく部品catalogから取れます。応力・熱解析は未対応で、検査レポートにも未評価として残します。

- [設計と検査範囲](docs/design.md)
- [部品catalog](docs/catalog.md)
- [セットアップ・作例・テスト](docs/development.md)
- [開放筐体の作例とE2E test](docs/development.md#開放筐体の作例)
- [ロードマップ](docs/roadmap.md)
- [依存調査と固定方針](docs/dependencies.md)

Linux x86_64 / CPython 3.12を初期検証対象とします。Apache-2.0 license。
