# 既存ケースとの比較

M1の合格条件のうち、既存の筐体へ最終形状のruleを適用して比較した結果を記す。対象のファイルはリポジトリに含めない。取得元、commit、sha256を記すので、利用者が取得して同じ評価を再現できる。評価の方式と制約は [設計](design.md#外部形状への適用) を参照する。

## 対象

| 対象 | 取得元 (repository / path) | commit | ライセンス |
| --- | --- | --- | --- |
| Tang Nano 9K 部品holder | [phoenix367/tang_nano_9K_ov7670](https://github.com/phoenix367/tang_nano_9K_ov7670) / `physical/parts_holder.stl` | `0d7e26b9bbfda6bbbd7ebc545d7240f6fd8c20bd` | MIT |
| MiSTeryNano筐体 (Tang Nano 20K、USB-C Breakout版) | [prcoder-1/MiSTeryNano-Case](https://github.com/prcoder-1/MiSTeryNano-Case) / `USB-C Breakout-Case.stl` | `a62928fba7ee366d20267b0604388340116c6a91` | 記載なし (再配布不可) |
| MiSTeryNano蓋 (同上) | 同上 / `USB-C Breakout-Cover.stl` | 同上 | 記載なし (再配布不可) |
| Framework Expansion Card筐体 | [FrameworkComputer/ExpansionCards](https://github.com/FrameworkComputer/ExpansionCards) / `Mechanical/Printable/3D/ExpansionCard_SelfTapping.stl` | `2904a95d60d4ba771c5c3c56d6fdba7078243fe8` | CC BY 4.0 |

| ファイル | sha256 |
| --- | --- |
| `parts_holder.stl` | `a4cb636c4c4278be513417ccde249ea38b7ae6a341ea707c92c9b70bab869f24` |
| `USB-C Breakout-Case.stl` | `a4848d4330e7d254beea963b7acb67046b6dea70e21662342d5bd12fd15bc1b2` |
| `USB-C Breakout-Cover.stl` | `68fdee343f78fd4051ac406a1f1270cefb4f2ab431a91c1f7e5748406537d3f7` |
| `ExpansionCard_SelfTapping.stl` | `1c1f709830333e8fe5b1a889fc184a88fd4a51452d4caaccc8f4b450f7105d8d` |

Tang Nano 9K用の閉じた筐体で、ライセンスと取得経路が明確なものは見つからなかった。Tang Nano 20K用のMiSTeryNano筐体で代える。

## 手順

ファイルは`.work/`など、git管理外の場所へ取得する。

```bash
mkdir -p .work/external
curl -fsSL -o .work/external/parts_holder.stl \
  https://raw.githubusercontent.com/phoenix367/tang_nano_9K_ov7670/0d7e26b9bbfda6bbbd7ebc545d7240f6fd8c20bd/physical/parts_holder.stl
sha256sum .work/external/parts_holder.stl       # 上表の値と照合する
.venv/bin/python -m typedsolid.external .work/external/parts_holder.stl
```

条件は標準policyの値である。格子0.2 mm、`min_wall_mm` 1.2、`min_neck_mm` 1.2、積層方向はファイルの向きのまま`plus_z`、overhang 45°、bridge 5 mmとした。印刷時の向きは各作者の意図と異なる場合がある。

## 結果

| 対象 | final_wall_thickness | neck_section | closed_cavity | support_free |
| --- | --- | --- | --- | --- |
| Tang Nano 9K 部品holder | pass | pass | pass | pass |
| MiSTeryNano筐体 | 評価不能 | 評価不能 | 評価不能 | 評価不能 |
| MiSTeryNano蓋 | pass | pass | pass | fail (0.128 mm³) |
| Framework Expansion Card筐体 | fail (7領域、最大132.2 mm³) | pass | pass | fail (2.816 mm³、bridge 0.576 mm³) |

- **Tang Nano 9K 部品holder**: 体積126,124 mm³ (格子上)。4 ruleとも通る。
- **MiSTeryNano筐体**: 向き付きで対にならない辺が8本あり、閉じていないか向きが不整合なmeshとして評価を拒否した。T字接続か実際の隙間かは区別していない。
- **MiSTeryNano蓋**: 0.128 mm³ (0.2 mm格子で16 cell) の未支持を検出した。0.1 mm格子では0.076 mm³となる。局所的な小さい張り出しである。`support_free`はslicer条件を含まない保守的な近似であり、実際の印刷で支持が要るかは別途slicerで確かめる。
- **Framework Expansion Card筐体**: 1.2 mm未満の領域が7つある。`min_wall_mm`を1.0とすると9領域 (最大34.4 mm³)、0.8としても12領域 (最大19.6 mm³) が残る。値を下げると縁とみなす領域の下限 (`min_wall_mm`³) も下がるため、領域数は減るとは限らない。標準policyの1.2 mmより薄い造作を持つ設計である。

同じリポジトリの`ExpansionCard_SelfTapping.stp`は、印刷する筐体のほかに、厚さ0.8 mmの板と小さな部品2つの計4 solidを含む組立体である。印刷用のSTLと対象が異なるため比較に用いない。

## 解釈

- 既知の欠陥fixture (`tests/test_enclosure_fixtures.py`) を自前のIRからSTLとSTEPへ出力し、この経路でもIRの経路と同じruleが落ちることを`tests/test_external.py`で確かめている。
- 第三者の筐体では、判定が作者の設計前提 (肉厚、印刷の向き、slicer設定) に強く依存する。failは欠陥の断定ではなく、標準policyとの差を示す。`min_wall_mm`や`build_direction`を作者の前提に合わせて与えると、差の内訳を確かめられる。
- 閉じていないmeshは評価しない。検査の前提 (内外が定まること) が成り立たないためである。
