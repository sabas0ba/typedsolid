# 開発順序

## M0: 意味モデルから検査付き出力まで

- Rust core、Python API、schema v1、CadQuery adapter
- 軸平行boxのunion-minus-cuts
- 最終solid数、形状妥当性、keepout、+Zアクセス、部品間干渉
- 成功・失敗作例、単体/統合test、CI、再現手順

今回のPRの範囲。製造可能性全体を保証する段階ではない。

## M1: 実用筐体の構造と寸法

- 円柱・穴・boss (対応済み)、寸法型と座標変換。面取りはOCCTのedge選択に依存するため見送る
- 部品catalog、基板寸法と出典、支持面への接触指定 (対応済み)。コネクタ開口は後続
- 最終肉厚、接続部断面の保守的な検査 (対応済み)。slendernessは後続
- 閉空洞、印刷姿勢、overhang/bridgeの検査 (対応済み)。slicerでの評価との突合
- STL meshのmanifold性・接続成分・退化三角形検査 (対応済み)

最終形状に対する検査はIRから直接rasterizeしたvoxel上で行い、OCCTのface/edge topologyに依存させない。解像度は`policy.voxel_mm`で指定し、量子化分は失敗側に倒す。overhang/bridgeも同じoccupancyから判定する。

catalogは公式資料が寸法線として与える値だけを持ち、記載のない値は`None`とする。コネクタ開口は、登録した3基板のいずれの資料も開口寸法を寸法線で与えないため見送る。開口寸法をUSB・HDMIの規格書から採るかどうかは、出典の到達性と規格の取り扱いを含めて別途判断する。詳細は [部品catalog](catalog.md) を参照する。

合格条件: 開口を持つ筐体で、薄肉・折れやすい接続・孤立・支持不能箇所の既知の失敗fixtureを検出し、Tang Nano等の既存ケースへ適用して比較する。

fixtureは `tests/test_enclosure_fixtures.py` が持つ。上面の開いた36×26×14 mmの筐体を基準形状とし、欠陥を1つずつ入れて、落ちるruleの集合が宣言と完全に一致することを見る。検出漏れと過検出のどちらもこの比較で落ちる。

| fixture | 欠陥 | 落ちるrule |
| --- | --- | --- |
| `baseline` | なし | なし |
| `thin_wall` | 右壁の内側を削り残り1 mm | `final_wall_thickness` |
| `narrow_neck` | 幅1.6 mmの桟だけで左右を繋ぐ | `neck_section` |
| `severed_corner` | 同じスリットを桟を残さず通す | `single_solid`, `neck_section` |
| `cantilever` | 壁から6 mm張り出す棚 | `support_free` |
| `sealed_void` | 塊の内部に外へ通じない空洞 | `closed_cavity` |

`min_neck_mm`は2.0とし、既定の`min_wall_mm` 1.2と分けている。両者が同じ値だと、断面が足りない箇所は必ず肉厚も足りず、2つのruleを区別できない。

既存ケースへの適用は未実施である。第三者の筐体モデルを取り込むことになるため、出典と再配布条件を確認してから行う。

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

Nix/Docker環境、Windows/macOSのwheel、API拡張、可視化は需要とCI費用を見て追加する。初期対応はLinux x86_64 / CPython 3.12。公開レジストリへのpublishとGitHub Pages公開はこのPRでは行わない。
