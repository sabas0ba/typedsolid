# TypedSolid 設計

## 目的

意味情報と設計・製造ルールを持つプログラマブルなソリッドモデリング基盤を構築する。主用途は3Dプリント向けの筐体・支持具である。形状の生成だけでなく、部品の独立性、強度上の弱点、基板やコネクタの空間、組立・保守時のアクセスを検証対象にする。

新規B-Repカーネルは開発しない。Rustが意味付き中間表現（IR）とルール判定を管理し、Python APIから使用する。初期backendはCadQuery/OCCTとし、将来のbackend変更が公開モデル全体に波及しない境界を設ける。

## 要求と評価段階

| 要求 | 対応方針 | 初期実装 |
|---|---|---|
| 孤立パーツを避ける | 最終Boolean結果のsolid数を検査 | 対応 |
| 1 STL＝1部品 | 検査済みPartごとにSTL/STEPを出力 | 対応 |
| 細く折れやすい箇所を減らす | primitive寸法、最終肉厚、接続部断面、荷重・積層方向を段階的に評価 | primitive最小寸法のみ |
| 基板等のスペース確保 | clearanceで拡張したkeepoutと最終形状の交差体積を評価 | 軸平行boxに対応 |
| アクセスの確保 | 部品・工具の移動領域と形状の干渉検査 | 全部品に対する+Z直線経路のみ |
| サポート不要化 | 印刷方向、overhang、bridge、閉空洞、層ごとの島を評価 | 未対応 |
| 応力・熱解析 | 材料、境界条件、解析mesh、solverの条件を明示して連携 | 未対応 |

「中空を減らす」は内部を一律に埋める規則にはしない。基板空間・軽量化・放熱と競合するため、閉じた空洞や支持のない天井を制約し、必要な開放空間を保持する。infill率はslicer設定であり、CADの空洞と区別する。

## 境界と責務

| 層 | 責務 | 持たない責務 |
|---|---|---|
| Python API | typed dataclassによるモデル組立、serialization | 検証の独自再実装 |
| Rust core | schema、単位、ID、role、入力検証、preflight、出力判定 | OCCTオブジェクト保持 |
| PyO3 binding | JSON境界でcoreを呼ぶ | CAD処理 |
| CadQuery backend | IR→形状、最終solid・干渉検査、STL/STEP出力 | 未対応ルールの合格判定 |

JSONは初期のFFI・保存境界であり、Python scriptの文字列を評価しない。今後の性能測定で問題になるまで、独自の複雑なFFIオブジェクト共有を導入しない。

## IR v1

- 長さはmm。座標は右手系で全Part共通、+Zを上方とする。回転・配置変換・材料はまだ扱わない。
- `Model`は`schema_version=1`、`parts`、`keepouts`、`policy`を持つ。
- `Part`は単独の製造部品。`Feature`のIDはPart内で一意、Part IDとKeepout IDは各名前空間内で一意とする。
- `Feature`は軸平行box、`role`、`operation`を持つ。roleは`base/wall/mount/rib/generic`。roleだけで強度を保証しない。
- Booleanの意味は「すべてのAddの和から、すべてのCutを引く」。記述順依存の逐次CSGではない。Cutは生成用であり、最小feature寸法ルールの対象外。
- `Keepout`は確保領域と一様clearanceを持つ。`access=plus_z`はclearance込み断面を全部品の最高点より2 mm上まで掃引した領域を検査する。
- IDは小文字ASCII英字で始まり、小文字英数字とunderscoreのみ、64文字以内。path traversalとWindows予約名を拒否する。
- 座標±1,000,000 mm、box寸法0.001 mm以上、100部品・100keepout・合計1000feature・JSON 1 MB以内を実装上の上限とする。これらはプリンタ能力の保証値ではない。

## 検査と出力

検査は`pass / fail / not_evaluated`の3状態とし、rule ID・target ID・理由を保持する。1件でもfailがある場合、またはrequiredルールが全件passでない場合、出力を拒否する。preflightだけでは標準policyの出力条件を満たさない。

標準policyは`feature_thickness/valid_solid/single_solid/keepout_clearance/access_clearance/part_interference`を必須とする。適用対象がないkeepout・access・部品間干渉検査は「宣言された対象なし」と明示する。未宣言の基板・工具が存在しないことは保証しない。

`final_wall_thickness/support_free/strength/thermal`は常にnot_evaluatedである。requiredに指定した場合は出力を拒否する。標準policyでの出力許可は「実装済み検査を満たした」の意味であり、印刷・構造安全の認証ではない。

交差体積は1e-7 mm³を許容差とし、面接触は許容する。印刷公差や嵌合clearanceと、この数値誤差の許容差を混同しない。極小の干渉、STL meshのmanifold性、slicer上の層島・bridge・supportは別の検査が必要である。

`valid_solid`の空判定には接触判定の許容差を流用せず、1e-12 mm³を下限とする。最小box寸法0.001 mmの立方体は1e-9 mm³であり、接触許容差を空判定に使うと正当な最小形状を無効と扱うためである。

`export(model, directory)`はモデルを再buildして検査する。利用者が変更可能な`Build.report`をexportの証拠として再利用しない。既存出力ディレクトリを上書きせず、検査とファイル生成が完了した後に新規ディレクトリへ配置する。STL/STEPのほか、意味モデルの`model.json`、検査・backend版・ファイルSHA-256を含む`report.json`を保存する。`model_sha256`は保存した`model.json`のbytesそのもののdigestであり、利用者は保存ファイルの再hashで照合できる。

## CadQuery/OCCT依存への対策

永続的な意味IDはIRのPart/Featureに置き、face番号やedge列挙順に置かない。初期実装はboxとBooleanに限定し、fragileなface selectorを使用しない。Boolean後のface→feature対応は現段階で保証しない。

backend例外、無効形状、空形状をfailとして保持する。依存を固定し、版更新時は孤立・切断・空形状・干渉・アクセス・STEP再読込の回帰テストを行う。STLのバイト一致ではなく、寸法・体積・接続性と検査結果を比較する。カーネルのhard crashやhangはPython例外処理では隔離できないため、将来worker processとtimeoutを導入する。

## 解析への拡張

応力解析は材料定数、積層方向・異方性、荷重、支持条件、接触、mesh品質・収束性を明示する。熱解析は発熱量、熱伝導率、接触熱抵抗、対流条件、周囲温度を明示する。値が欠ける場合に任意の既定値で合格扱いしない。

Part/Featureから解析境界へstableな参照を渡す。ただしBoolean後に対応が失われた境界を推測で指定しない。印刷用STLをそのまま解析用体積meshとして扱わず、solver連携は別adapterとする。初期実装にはsolver依存を含めない。
