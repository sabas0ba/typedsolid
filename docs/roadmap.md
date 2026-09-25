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

- 工具・ケーブル・コネクタ抜き差しの掃引領域 (対応済み)
- lid取り外し等のassembly state、着脱順序、複数軸のアクセス (対応済み。keepoutと部品の取付関係は後続)
- 任意ネジ固定 (セルフタップ、熱圧入インサート)、材料・積層方向を含む設計profile (対応済み)
- snap fit (対応済み。矩形断面の片持ち梁)
- backendをworker processへ隔離し、timeout・再開可能な生成を導入 (対応済み)

実装順はworker隔離、分解stepと着脱検査、掃引、profileとネジ固定、snap fitとする。状態ごとに評価が増えるため、打ち切りとcacheを先に入れる。

- **組立状態**: IRに記述した部品位置を組立状態とし、分解手順を順序付きのstepで表す。各stepは部品を軸平行の直線区間を連ねた経路で動かして取り除く。組立順序は分解の逆とする。回転と任意軸の配置変換は扱わない。
- **干渉判定**: 着脱と掃引の干渉はOCCTのBooleanで判定し、clearanceは形状間の最短距離で見る。はめ合い隙間は0.2〜0.3 mm程度でvoxelの既定pitchと同じ桁にあり、量子化を失敗側に倒すと正しい設計まで落ちるためである。既存の`part_interference`と同じ方式になる。
- **掃引**: 現在の`access`を一般化し、box又は軸平行cylinderの断面、方向、距離、必要となる分解stepを持つ`Sweep`とする。
- **数値の出典**: 材料の許容ひずみ、ネジ・インサート寸法は型だけを用意し、値は利用者が与える。catalogと同じく根拠のない既定値を組み込まない。
- **設計profile**: 材料はIRの`Material`とし、部品へ割り当てる。snap fitの許容ひずみを材料に持たせ、M3の材料定数も同じ型へ追加する。印刷機と設計値はPythonのhelperに留め、IRに現れるのはprofileから作った`Policy`とはめ合い隙間だけである。
- **ネジ固定**: IRの`Fastener`がネジ・インサートの寸法を持ち、`fastener_fit`が貫通穴、座面、かかり長さ、先端の逃げ、bossの肉厚を実形状と照合する。締結力やねじ山の強度はM3の解析で扱う。
- **snap fit**: 矩形断面の片持ち梁に限る。ひずみははりの理論から求め、梁の長さ方向が積層方向と平行でないことを検査する。外すstepではフックをたわませた部品で分解経路を掃引し、たわませないフックが経路を塞ぐこと (保持) も検査する。たわみ量は利用者が宣言する。

## M3: 解析連携

- solver候補のライセンス・保守・検証実績を比較して選定
- 材料・荷重・拘束・発熱・熱境界の型付きschema
- 体積mesh、境界対応、mesh品質と収束テスト
- 既知解のある梁と熱伝導問題で検証後、筐体の応力・温度を評価

## 後続判断

Nix/Docker環境、Windows/macOSのwheel、API拡張、可視化は需要とCI費用を見て追加する。初期対応はLinux x86_64 / CPython 3.12。公開レジストリへのpublishとGitHub Pages公開はこのPRでは行わない。
