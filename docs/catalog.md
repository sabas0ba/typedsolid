# 部品catalog

筐体を設計するには、収める基板の外形、取付穴、部品の高さが要る。これらを都度実測するか
非公式のモデルから写すと、値の根拠が残らない。catalogは公式資料が寸法線として与える値だけを
持ち、各entryに出典を添える。

## 座標系

基板座標系はPCB外形の左下角を原点とし、長辺を+x、短辺を+y、PCB下面をz=0、部品側を+zとする。
`origin`はこの原点に対応するmodel座標を指す。IRのshapeは軸平行に限られるため、基板も
同じ向きで置く。回転した配置は扱わない。

## 登録済みの基板

| id | 基板 | 外形 [mm] | PCB板厚 [mm] | 取付穴 | 全高 [mm] |
| --- | --- | --- | --- | --- | --- |
| `raspberry_pi_pico_2` | Raspberry Pi Pico 2 | 51 × 21 | 1.0 | φ2.1 × 4 (47 × 11.4ピッチ) | 記載なし |
| `raspberry_pi_5` | Raspberry Pi 5 | 85 × 56 | 記載なし | φ2.7 × 4 (58 × 49ピッチ) | 記載なし |
| `arduino_uno_r4_minima` | Arduino UNO R4 Minima | 68.58 × 53.34 | 1.0 | φ3.2 × 4 (非対称) | 8.5 |

全高はPCB下面から最も高い部品の頂点までを指す。

取付穴の座標は次のとおり。Pico 2は外形の中心に対して対称、Pi 5は短辺方向だけ対称、
UNO R4 Minimaは両方向とも非対称である。

| id | 取付穴中心 (x, y) [mm] |
| --- | --- |
| `raspberry_pi_pico_2` | (2.0, 4.8), (2.0, 16.2), (49.0, 4.8), (49.0, 16.2) |
| `raspberry_pi_5` | (3.5, 3.5), (3.5, 52.5), (61.5, 3.5), (61.5, 52.5) |
| `arduino_uno_r4_minima` | (13.97, 2.54), (15.24, 50.8), (66.04, 7.62), (66.04, 35.56) |

## 出典

| id | 資料 | 版 | 参照箇所 |
| --- | --- | --- | --- |
| `raspberry_pi_pico_2` | [Raspberry Pi Pico 2 Datasheet](https://datasheets.raspberrypi.com/pico/pico-2-datasheet.pdf) | Release 5 (build date 03/07/2026) | 3. Mechanical specification / Figure 3 |
| `raspberry_pi_5` | [Raspberry Pi 5 mechanical drawing](https://datasheets.raspberrypi.com/rpi5/raspberry-pi-5-mechanical-drawing.pdf) | 版の記載なし (2026-09-18取得) | 平面図 (Scale 1:1 @A4) |
| `arduino_uno_r4_minima` | [Arduino UNO R4 Minima Datasheet](https://docs.arduino.cc/resources/datasheets/ABX00080-datasheet.pdf) | Modified 15/09/2026 | 11 Mounting Holes And Board Outline |

Pi 5の図は「寸法は参考値であり製造データに使用しない」と明記している。筐体側のclearanceは
利用者が決める。

## 記載のない値

資料が寸法線として与えない値は`None`とし、推定で埋めない。`height()`と`envelope()`は、
全高が`None`の基板では呼び出し側に値を要求する。

```python
board("raspberry_pi_5").height()          # ValueError: 出典が全高を与えていない
board("raspberry_pi_5").height(20.0)      # 20.0
```

コネクタ開口は現時点でどのentryも持たない。3件の資料はいずれも開口の縦横を寸法線で
与えていないためである。Pi 5の図はコネクタの位置 (左端から11.2 / 25.8 / 39.2など) を
与えるが開口寸法は与えず、Pico 2とUNO R4 Minimaは位置も開口も寸法化していない。
開口寸法をUSBやHDMIの規格書から採る場合は、出典の到達性と規格の取り扱いを別途検討する。

## 使い方

```python
from typedsolid import Box, Clearance, Feature, Model, Part, board
from typedsolid.cadquery import export

shell = (Feature("floor", Box((0, 0, 0), (60, 40, 2)), "base"),)   # 壁は省略

pico = board("raspberry_pi_pico_2")
origin = (4.5, 9.5, 4.0)     # 基板左下角を置くmodel座標。PCB下面がz=4.0

pads = pico.bosses((0.0, 4.0), 5.0, origin)              # 取付穴位置に立てる支持pad
screws = pico.pilot_holes((-1.0, 4.0), 1.6, origin)      # padを貫くネジ下穴
keepout = pico.keepout(
    "pcb", origin,
    height_mm=5.0,                                        # 資料に全高がないため指定する
    clearance=Clearance(default=0.5, minus_z=0.0),        # 下面だけ接触を許す
    access=("plus_z",),
)

model = Model(parts=(Part("tray", shell + pads + screws),), keepouts=(keepout,))
export(model, ".work/tray")
```

`bosses`と`pilot_holes`は`prefix`でidの接頭辞を変えられる。同じ基板を複数枚置く場合は
基板ごとに別の`prefix`を与える。

## entryの追加

1. 公式資料を取得し、外形、PCB板厚、取付穴の径と座標、全高を寸法線から読む。
2. 読み取りが図の目視に依存する場合は、PDFのベクタ座標で照合する。`pdftocairo -svg`で
   変換したパス座標から、既知の寸法を基準にした縮尺で穴の中心と径を求める。Pico 2は
   この方法で、短辺方向の穴間11.4を基準として穴径2.092 mm (図の記載はφ2.1)、
   長辺方向の穴間47.03 mmを得た。図が画像として埋め込まれている資料ではこの照合はできず、
   UNO R4 Minimaが該当する。
3. `python/typedsolid/catalog.py`にentryを追加し、`Source`に資料名、URL、版、参照箇所を記す。
   資料に版の記載がない場合は取得日を記す。
4. `tests/test_catalog.py`の`OUTLINES_MM`と穴座標のtestを追加する。転記誤りはこのtestが
   検出する。
5. 資料が与えない値は`None`のままとし、docsの表にも「記載なし」と書く。
