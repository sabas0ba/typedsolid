# TypedSolid 設計

## 目的

意味情報と設計・製造ルールを持つプログラマブルなソリッドモデリング基盤を構築する。主用途は3Dプリント向けの筐体・支持具である。形状の生成だけでなく、部品の独立性、強度上の弱点、基板やコネクタの空間、組立・保守時のアクセスを検証対象にする。

新規B-Repカーネルは開発しない。Rustが意味付き中間表現（IR）とルール判定を管理し、Python APIから使用する。初期backendはCadQuery/OCCTとし、将来のbackend変更が公開モデル全体に波及しない境界を設ける。

## 要求と評価段階

| 要求 | 対応方針 | 初期実装 |
|---|---|---|
| 孤立パーツを避ける | 最終Boolean結果のsolid数を検査 | 対応 |
| 1 STL＝1部品 | 検査済みPartごとにSTL/STEPを出力 | 対応 |
| 出力STLが閉じた立体である | 書き出したmeshのmanifold性、向き、連結成分、体積を検査 | 対応 |
| 細く折れやすい箇所を減らす | primitive寸法、最終肉厚、接続部断面、荷重・積層方向を段階的に評価 | primitive寸法、最終肉厚、接続部断面 |
| 閉じた空洞を避ける | 最終形状の空領域が外部へ通じているかを検査 | 対応 |
| 基板等のスペース確保 | clearanceで拡張したkeepoutと最終形状の交差体積を評価 | 軸平行box、面ごとのclearance |
| アクセスの確保 | 部品・工具の移動領域と形状の干渉検査 | 全部品に対する6方向の直線経路 |
| サポート不要化 | 印刷方向、overhang、bridge、閉空洞、層ごとの島を評価 | 印刷方向、overhang、bridge |
| 応力・熱解析 | 材料、境界条件、解析mesh、solverの条件を明示して連携 | 未対応 |

「中空を減らす」は内部を一律に埋める規則にはしない。基板空間・軽量化・放熱と競合するため、閉じた空洞や支持のない天井を制約し、必要な開放空間を保持する。infill率はslicer設定であり、CADの空洞と区別する。

## 境界と責務

| 層 | 責務 | 持たない責務 |
|---|---|---|
| Python API | typed dataclassによるモデル組立、serialization、部品catalog | 検証の独自再実装 |
| Rust core | schema、単位、ID、role、入力検証、preflight、出力判定 | OCCTオブジェクト保持 |
| PyO3 binding | JSON境界でcoreを呼ぶ | CAD処理 |
| CadQuery backend | IR→形状、最終solid・干渉検査、STL/STEP出力 | 未対応ルールの合格判定 |

JSONは初期のFFI・保存境界であり、Python scriptの文字列を評価しない。今後の性能測定で問題になるまで、独自の複雑なFFIオブジェクト共有を導入しない。

部品catalogはIRを組み立てるための寸法データであり、検証には関与しない。catalogが返す`Keepout`と`Feature`は手で書いたものと区別されず、同じRust coreの検証を通る。値はすべて公式資料が寸法線として与える数値に限り、記載のない項目は`None`として利用側に指定を求める。詳細は [部品catalog](catalog.md) を参照する。

## IR v2

- 長さはmm。座標は右手系で全Part共通、+Zを上方とする。回転・配置変換・材料はまだ扱わない。
- `Model`は`schema_version=2`、`parts`、`keepouts`、`policy`を持つ。
- `Part`は単独の製造部品。`Feature`のIDはPart内で一意、Part IDとKeepout IDは各名前空間内で一意とする。
- `Feature`は`shape`、`role`、`operation`を持つ。roleは`base/wall/mount/rib/generic`。roleだけで強度を保証しない。
- `shape`は`kind`で分岐する。`box`は`min`/`max`、`cylinder`は`axis`、軸に垂直な平面上の`center`、`radius`、軸方向の`span`を持つ。cylinderは軸平行に限る。任意軸は配置変換とあわせて後続項目とする。
- 穴は`operation=cut`のcylinder、bossは`add`のcylinderで表す。面取り・filletはOCCTのedge選択に依存するため導入しない。
- Booleanの意味は「すべてのAddの和から、すべてのCutを引く」。記述順依存の逐次CSGではない。Cutは生成用であり、最小feature寸法ルールの対象外。
- 最小feature寸法はprimitiveの寸法を測る。cylinderは直径と高さの小さい方とする。
- `Keepout`は確保領域と面ごとのclearance、access方向の集合を持つ。`clearance_mm`は`default`と面名 (`minus_x`等) の上書きからなる。支持面へ接触させる面だけを0にでき、他の面の要求は残る。
- `access`は6方向から選ぶ。各方向についてclearance込みの断面を、全部品のAABBの外側2 mmまで掃引した領域を検査する。
- keepoutはboxに限る。面別clearanceと6方向accessはboxの面を前提とするため、cylinderのkeepoutは受理しない。
- IDは小文字ASCII英字で始まり、小文字英数字とunderscoreのみ、64文字以内。path traversalとWindows予約名を拒否する。
- `Policy`は`min_feature_mm`のほか、最終形状の検査に`voxel_mm`、`min_wall_mm`、`min_neck_mm`、`build_direction`、`overhang_angle_deg`、`bridge_max_mm`を持つ。primitiveの寸法と最終形状の肉厚は別の概念であり、要求値も分けて指定する。
- 座標±1,000,000 mm、primitive寸法0.001 mm以上、100部品・100keepout・合計1000feature・JSON 1 MB以内、1部品あたりvoxel 2億cell以内を実装上の上限とする。これらはプリンタ能力の保証値ではない。

### v1からの昇格

`schema_version=1`のJSONは読み込み時にv2へ変換する。v1は軸平行box、一様clearance、単一accessだけを表現できるため、対応は一意に定まる。`bounds`は`shape`の`kind=box`へ、数値の`clearance_mm`は`{"default": n}`へ、`access`の文字列は1要素の配列へ、`null`は空配列へ移す。出力は常にv2で、v1では書き出さない。

## 検査と出力

検査は`pass / fail / not_evaluated`の3状態とし、rule ID・target ID・理由を保持する。1件でもfailがある場合、またはrequiredルールが全件passでない場合、出力を拒否する。preflightだけでは標準policyの出力条件を満たさない。

標準policyは`feature_thickness/valid_solid/single_solid/keepout_clearance/access_clearance/part_interference/final_wall_thickness/neck_section/closed_cavity/support_free`を必須とする。`mesh_manifold/mesh_volume`はexport時に評価され、failがあれば出力を拒否する。適用対象がないkeepout・access・部品間干渉検査は「宣言された対象なし」と明示する。未宣言の基板・工具が存在しないことは保証しない。

`strength/thermal`は常にnot_evaluatedである。requiredに指定した場合は出力を拒否する。標準policyでの出力許可は「実装済み検査を満たした」の意味であり、印刷・構造安全の認証ではない。

交差体積は1e-7 mm³を許容差とし、面接触は許容する。印刷公差や嵌合clearanceと、この数値誤差の許容差を混同しない。極小の干渉、STL meshのmanifold性、slicer上の層島・bridge・supportは別の検査が必要である。

`valid_solid`の空判定には接触判定の許容差を流用せず、1e-12 mm³を下限とする。最小box寸法0.001 mmの立方体は1e-9 mm³であり、接触許容差を空判定に使うと正当な最小形状を無効と扱うためである。

## 最終形状の検査

`final_wall_thickness`、`neck_section`、`closed_cavity`、`support_free`はIRから直接rasterizeしたvoxel上で判定する。OCCTのface/edge topologyに依存せず、boxとcylinderの内外判定だけで最終形状を得る。Boolean後のface対応を推測しないという方針と整合する。

格子の間隔は`policy.voxel_mm` (既定0.2 mm)、cell数の上限は1 partあたり2億である。上限を超える入力はvalidateで拒否する。cellはvoxel中心で内外を判定するため、形状は最大で格子の半分だけ外側へ膨らむ。要求値に格子1つ分を足して判定し、量子化の誤差を失敗側へ倒す。格子より小さい形状は評価できず、`neck_section`がfailするため出力は拒否される。

- `final_wall_thickness`: 各cellを通る軸方向の連続長のうち最小のものを厚さとする。扱うprimitiveは軸平行に限るため、壁は必ずいずれかの軸に沿って厚さを持つ。球のopeningで測ると角や稜線が必ず除去され、十分に厚い立体まで薄肉と判定されるため採らない。面の縁や稜線は必ず薄くなるので、一辺`min_wall_mm`の立方体に満たない領域は形状の縁として数えない。
- `neck_section`: 半径`min_neck_mm / 2`でerosionし、6-連結の連結成分が1つに保たれるかを見る。分かれる場合は、その断面で繋がる細い接続部がある。距離場はFelzenszwalb-Huttenlocherの下位包絡線法で求める厳密なEuclidean距離であり、chamfer近似のような方向依存の誤差を持たない。
- `closed_cavity`: 空cellを格子の外周から6-連結でflood fillし、到達できない空領域を閉空洞とする。格子より細い隙間を通じて外部に通じる空洞は、閉じていると判定される。
- `support_free`: `policy.build_direction` (既定`plus_z`) の軸で層に分け、各層のcellが直下の層の半径`tan(overhang_angle_deg)` cell以内に材料を持つかを見る。材料が最初に現れる層はbuild plateに接するものとして支持済みとする。直下が支持されているかは問わない。支持の要否は層ごとに独立して評価し、1箇所のoverhangがその上の全体を未支持にすることを避ける。未支持のcellは、同じ層で両端を支持された材料に挟まれ、区間長が`bridge_max_mm`以内であればbridgeとして渡せるものとする。端で終わる区間は片持ちでありbridgeにならない。

`support_free`はノズル径、層厚、冷却、材料といったslicerとプリンタの条件を含まない。判定は幾何のみに基づく保守的な近似であり、実際に支持なしで印刷できることを保証しない。slicerでの評価との突合は`docs/development.md`の手順による。

## 出力meshの検査

`mesh_manifold`と`mesh_volume`はbackendが書き出したSTLを対象とする。設計入力ではなく出力表現の検査であり、STLが存在するexport時にのみ評価できる。`build`の時点ではnot_evaluatedとして残る。標準policyのrequiredには含めないが、failは他の検査と同様に出力を拒否するため、exportは常にこの検査を通過した結果だけを配置する。

`mesh_manifold`は各有向edgeが1回、その逆向きも1回だけ現れること (閉じた向き整合)、退化三角形がないこと、連結成分が1つであることを検査する。`mesh_volume`はmeshの符号付き体積を発散定理で求め、solid体積との相対差を`policy.mesh_volume_tolerance` (既定0.01) と比較する。符号が負の場合は全体の向きが内向きであることを示す。

頂点はf32のbit patternで同一視する (-0.0は+0.0に正規化)。同一座標を異なるfloatで書いたmeshは隣接を検出できず非manifoldと判定される。この判定は安全側に倒れる。binary STLのみを対象とし、ASCII STLは長さ不一致として拒否する。解析失敗は例外にせず、failのcheckとして保持する。

この検査はmeshが閉じた単一の立体であることを確認するものであり、自己交差、印刷可能性、slicer上の挙動を保証しない。

`export(model, directory)`はモデルを再buildして検査する。利用者が変更可能な`Build.report`をexportの証拠として再利用しない。既存出力ディレクトリを上書きせず、検査とファイル生成が完了した後に新規ディレクトリへ配置する。STL/STEPのほか、意味モデルの`model.json`、検査・backend版・ファイルSHA-256を含む`report.json`を保存する。`model_sha256`は保存した`model.json`のbytesそのもののdigestであり、利用者は保存ファイルの再hashで照合できる。

## CadQuery/OCCT依存への対策

永続的な意味IDはIRのPart/Featureに置き、face番号やedge列挙順に置かない。primitiveは軸平行のboxとcylinderに限定し、fragileなface selectorを使用しない。Boolean後のface→feature対応は現段階で保証しない。

backend例外、無効形状、空形状をfailとして保持する。依存を固定し、版更新時は孤立・切断・空形状・干渉・アクセス・STEP再読込の回帰テストを行う。STLのバイト一致ではなく、寸法・体積・接続性と検査結果を比較する。カーネルのhard crashやhangはPython例外処理では隔離できないため、将来worker processとtimeoutを導入する。

## 解析への拡張

応力解析は材料定数、積層方向・異方性、荷重、支持条件、接触、mesh品質・収束性を明示する。熱解析は発熱量、熱伝導率、接触熱抵抗、対流条件、周囲温度を明示する。値が欠ける場合に任意の既定値で合格扱いしない。

Part/Featureから解析境界へstableな参照を渡す。ただしBoolean後に対応が失われた境界を推測で指定しない。印刷用STLをそのまま解析用体積meshとして扱わず、solver連携は別adapterとする。初期実装にはsolver依存を含めない。
