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
| `raspberry_pi_pico` | Raspberry Pi Pico | 51 × 21 | 1.0 | φ2.1 × 4 (47 × 11.4ピッチ) | 記載なし |
| `raspberry_pi_pico_w` | Raspberry Pi Pico W | 51 × 21 | 1.0 | φ2.1 × 4 (47 × 11.4ピッチ) | 記載なし |
| `raspberry_pi_pico_2` | Raspberry Pi Pico 2 | 51 × 21 | 1.0 | φ2.1 × 4 (47 × 11.4ピッチ) | 記載なし |
| `raspberry_pi_pico_2_w` | Raspberry Pi Pico 2 W | 51 × 21 | 1.0 | φ2.1 × 4 (47 × 11.4ピッチ) | 記載なし |
| `raspberry_pi_zero_2_w` | Raspberry Pi Zero 2 W | 65 × 30 | 記載なし | 径記載なし × 4 (58 × 23ピッチ) | 記載なし |
| `raspberry_pi_3_model_b_plus` | Raspberry Pi 3 Model B+ | 85 × 56 | 記載なし | φ2.75 × 4 (58 × 49ピッチ) | 記載なし |
| `raspberry_pi_4_model_b` | Raspberry Pi 4 Model B | 85 × 56 | 記載なし | φ2.7 × 4 (58 × 49ピッチ) | 記載なし |
| `raspberry_pi_5` | Raspberry Pi 5 | 85 × 56 | 記載なし | φ2.7 × 4 (58 × 49ピッチ) | 記載なし |
| `arduino_uno_r4_minima` | Arduino UNO R4 Minima | 68.58 × 53.34 | 1.0 | φ3.2 × 4 (非対称) | 8.5 |
| `arduino_uno_r4_wifi` | Arduino UNO R4 WiFi | 68.58 × 53.34 | 1.0 | φ3.2 × 4 (非対称) | 8.5 |
| `arduino_nano_every` | Arduino Nano Every | 43.18 × 17.78 | 記載なし | φ1.65 × 4 (40.64 × 15.24ピッチ) | 記載なし |
| `arduino_nano_33_iot` | Arduino Nano 33 IoT | 43.16 × 17.77 | 記載なし | φ1.66 × 4 (40.64 × 15.24ピッチ) | 記載なし |
| `arduino_mkr_wan_1310` | Arduino MKR WAN 1310 | 67.7 × 25 | 記載なし | 径記載なし × 4 (57 × 20.5ピッチ) | 記載なし |

全高はPCB下面から最も高い部品の頂点までを指す。

取付穴の座標は次のとおり。Pico系は外形の中心に対して対称、Raspberry Pi 3B+/4/5は短辺方向だけ対称、UNO R4は両方向とも非対称である。Nano Every、Nano 33 IoT、MKR WAN 1310は、1つの穴の端からの距離と穴間ピッチで位置が決まる。

| id | 取付穴中心 (x, y) [mm] |
| --- | --- |
| `raspberry_pi_pico`, `raspberry_pi_pico_w`, `raspberry_pi_pico_2`, `raspberry_pi_pico_2_w` | (2.0, 4.8), (2.0, 16.2), (49.0, 4.8), (49.0, 16.2) |
| `raspberry_pi_zero_2_w` | (3.5, 3.5), (3.5, 26.5), (61.5, 3.5), (61.5, 26.5) |
| `raspberry_pi_3_model_b_plus`, `raspberry_pi_4_model_b`, `raspberry_pi_5` | (3.5, 3.5), (3.5, 52.5), (61.5, 3.5), (61.5, 52.5) |
| `arduino_uno_r4_minima`, `arduino_uno_r4_wifi` | (13.97, 2.54), (15.24, 50.8), (66.04, 7.62), (66.04, 35.56) |
| `arduino_nano_every` | (1.27, 1.27), (1.27, 16.51), (41.91, 1.27), (41.91, 16.51) |
| `arduino_nano_33_iot` | (1.26, 1.27), (1.26, 16.51), (41.9, 1.27), (41.9, 16.51) |
| `arduino_mkr_wan_1310` | (2.25, 2.25), (2.25, 22.75), (59.25, 2.25), (59.25, 22.75) |

## 出典

| id | 資料 | 版 | 参照箇所 |
| --- | --- | --- | --- |
| `raspberry_pi_pico` | [Raspberry Pi Pico Datasheet](https://datasheets.raspberrypi.com/pico/pico-datasheet.pdf) | Release 21 (build date 03/07/2026) | 2. Mechanical specification / Figure 3 |
| `raspberry_pi_pico_w` | [Raspberry Pi Pico W Datasheet](https://datasheets.raspberrypi.com/picow/pico-w-datasheet.pdf) | Release 7 (build date 03/07/2026) | 2. Mechanical specification / Figure 3 |
| `raspberry_pi_pico_2` | [Raspberry Pi Pico 2 Datasheet](https://datasheets.raspberrypi.com/pico/pico-2-datasheet.pdf) | Release 5 (build date 03/07/2026) | 3. Mechanical specification / Figure 3 |
| `raspberry_pi_pico_2_w` | [Raspberry Pi Pico 2 W Datasheet](https://datasheets.raspberrypi.com/picow/pico-2-w-datasheet.pdf) | Release 2 (build date 03/07/2026) | 2. Mechanical specification / Figure 3 |
| `raspberry_pi_zero_2_w` | [Raspberry Pi Zero 2 W mechanical drawing](https://datasheets.raspberrypi.com/rpizero2/raspberry-pi-zero-2-w-mechanical-drawing.pdf) | RP-008358-DS-1 (2026-09-28取得) | 平面図 |
| `raspberry_pi_3_model_b_plus` | [Raspberry Pi 3 Model B+ mechanical drawing](https://datasheets.raspberrypi.com/rpi3/raspberry-pi-3-b-plus-mechanical-drawing.pdf) | RP-008337-DS-2 (2026-09-28取得) | 平面図 |
| `raspberry_pi_4_model_b` | [Raspberry Pi 4 Model B mechanical drawing](https://datasheets.raspberrypi.com/rpi4/raspberry-pi-4-mechanical-drawing.pdf) | RP-008343-DS-1 (2026-09-28取得) | 平面図 |
| `raspberry_pi_5` | [Raspberry Pi 5 mechanical drawing](https://datasheets.raspberrypi.com/rpi5/raspberry-pi-5-mechanical-drawing.pdf) | 版の記載なし (2026-09-18取得) | 平面図 (Scale 1:1 @A4) |
| `arduino_uno_r4_minima` | [Arduino UNO R4 Minima Datasheet](https://docs.arduino.cc/resources/datasheets/ABX00080-datasheet.pdf) | Modified 15/09/2026 | 11 Mounting Holes And Board Outline |
| `arduino_uno_r4_wifi` | [Arduino UNO R4 WiFi Datasheet](https://docs.arduino.cc/resources/datasheets/ABX00087-datasheet.pdf) | Modified 22/09/2026 | 13 Mounting Holes And Board Outline |
| `arduino_nano_every` | [Arduino Nano Every Datasheet](https://docs.arduino.cc/resources/datasheets/ABX00028-datasheet.pdf) | Modified 22/09/2026 | 7.1 Board Outline and Mounting Holes |
| `arduino_nano_33_iot` | [Arduino Nano 33 IoT Datasheet](https://docs.arduino.cc/resources/datasheets/ABX00027-datasheet.pdf) | Modified 22/09/2026 | 6.1 Board Outline and Mounting Holes |
| `arduino_mkr_wan_1310` | [Arduino MKR WAN 1310 Datasheet](https://docs.arduino.cc/resources/datasheets/ABX00029-datasheet.pdf) | Modified 22/09/2026 | 4.1 Board Outline / 4.2 Mounting Holes |

Raspberry Piのmechanical drawingは版を記載しない。取得時のURLが転送される文書番号 (RP-008343-DS-1など) と取得日を版として記す。Pi 5の図は「寸法は参考値であり製造データに使用しない」と明記している。筐体側のclearanceは利用者が決める。

資料ごとの注意点は次のとおり。

- **UNO R4 WiFi**: 外形図はUNO R4 Minimaの資料と同一の画像である。全高8.5もこの図による。
- **Nano 33 IoT**: 6.1節の図は穴を「R0.83mm」、6.2節の図は「4x Ø1.65mm」と記し、両者が一致しない。取付穴の節である6.1節の値を採る。
- **Raspberry Pi 3B+/4**: 図は部品ごとに「Z=」「Z-Height=」の値を記すが、基準面を示さないため全高として採らない。

## 登録していない基板

次の基板は資料を確認したが、登録条件を満たさない。

| 基板 | 理由 |
| --- | --- |
| Arduino UNO R3 | 外形図が外形寸法と左下の穴のx座標を寸法化していない |
| Arduino Nano | 穴間ピッチだけを寸法化し、外形端から穴までの距離がない。図は画像で、ベクタ座標による照合ができない |
| Arduino Nano R4 | 左下の1穴の位置だけを寸法化している。本文 (43.18 mm × 17.78 mm) と仕様表 (18 mm × 45 mm) の外形も一致しない |
| Arduino Nano 33 BLE Rev2 | Arduino Nanoと同じく、外形端から穴までの距離がない |
| Arduino MKR WiFi 1010 | 右上の1穴の位置だけを寸法化している。図の画素上、左側の穴は端からの距離が右側と異なり、対称とみなせない |
| Arduino Nano ESP32 | 寸法図の文字がすべて輪郭化されたSVGで、開発シェルの道具では読み取れない |
| Espressif DevKit、Sipeed Tang Nano | 寸法図の配布元 (dl.espressif.com、dl.sipeed.com) に、開発環境のegress制限で到達できない |
| Seeed XIAO | 公式の寸法資料がDXFの形状データだけで、寸法線がない |

## 記載のない値

資料が寸法線として与えない値は`None`とし、推定で埋めない。`height()`と`envelope()`は、
全高が`None`の基板では呼び出し側に値を要求する。

```python
board("raspberry_pi_5").height()          # ValueError: 出典が全高を与えていない
board("raspberry_pi_5").height(20.0)      # 20.0
```

取付穴の径が`None`の基板 (Zero 2 W、MKR WAN 1310) でも、`bosses`と`pilot_holes`は使える。どちらも径を呼び出し側が与え、catalogからは中心座標だけを使うためである。

## コネクタ位置

資料がコネクタ中心の辺上の位置を寸法線で与え、寸法とコネクタの対応が一意に読める場合だけ、`Board.connectors`に登録する。`edge`はコネクタのある辺の外向き法線、`offset_mm`はその辺に沿った中心の基板座標 (`edge`がy方向ならx、x方向ならy) である。

| id | コネクタ | edge | offset_mm |
| --- | --- | --- | --- |
| `raspberry_pi_4_model_b` | `usb_c_power`, `micro_hdmi_0`, `micro_hdmi_1` | `minus_y` | 11.2, 26.0, 39.5 |
| `raspberry_pi_4_model_b` | `usb_a_0`, `usb_a_1`, `ethernet` | `plus_x` | 9.0, 27.0, 45.75 |
| `raspberry_pi_3_model_b_plus` | `micro_usb_power`, `hdmi`, `audio` | `minus_y` | 10.6, 32.0, 53.5 |
| `raspberry_pi_3_model_b_plus` | `usb_a_0`, `usb_a_1` | `plus_x` | 29.0, 47.0 |
| `raspberry_pi_zero_2_w` | `mini_hdmi`, `micro_usb_data`, `micro_usb_power` | `minus_y` | 12.4, 41.4, 54.0 |

登録していないコネクタと理由は次のとおり。

- **Pi 4の音声端子**: 下辺の7.5と11.5の寸法がどの部品を指すか一意に読めない。
- **Pi 3B+のEthernet**: 右辺の10.25と11.5の2つの寸法が近接しており、どちらが中心かを読めない。
- **Pi 5**: 図を再確認していないため、今回は登録していない。
- **Arduinoの基板**: Nano 33 IoTとMKR WAN 1310の資料にコネクタ位置の図があるが、今回は読み取っていない。

資料は高さ方向の中心を寸法化しないため、`connector_center`は高さを呼び出し側に求める。同じ種類のコネクタが並ぶ場合は、座標の小さい順に`_0`、`_1`と番号を付ける。

開口の寸法 (プラグ外形) はcatalogに持たない。USB・HDMI等の規格書は、利用許諾の範囲や入手条件の点で公開catalogへの転記に使えないためである。開口は`connector_opening`に、利用者が出典とともにプラグ寸法を与えて作る。詳細は [設計](design.md#コネクタ開口の検査) を参照する。

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
   Arduinoの資料が該当する。この場合は、全穴の位置が寸法線だけから定まる基板に限って登録する。

   円の抽出には`scripts/drawing-circles.py`を使う。縮尺は、2.54 mmピッチのピン列など図中の既知の寸法から求める。

   ```bash
   nix develop -c pdftocairo -svg -f 7 -l 7 pico-w-datasheet.pdf .work/pico-w.svg
   python scripts/drawing-circles.py .work/pico-w.svg --mm-per-unit 0.20594 \
       --min-diameter 1.5 --max-diameter 3
   ```
3. `python/typedsolid/catalog.py`にentryを追加し、`Source`に資料名、URL、版、参照箇所を記す。
   資料に版の記載がない場合は取得日を記す。
4. `tests/test_catalog.py`の`OUTLINES_MM`と穴座標のtestを追加する。転記誤りはこのtestが
   検出する。
5. 資料が与えない値は`None`のままとし、docsの表にも「記載なし」と書く。
