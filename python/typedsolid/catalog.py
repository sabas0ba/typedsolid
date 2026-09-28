"""基板の寸法catalog。値はすべて公式資料が寸法線として与える数値に基づく。

基板座標系はPCB外形の左下角を原点とし、長辺を+x、短辺を+y、PCB下面をz=0、
部品側を+zとする。回転した配置は扱わない。IRのshapeが軸平行に限られるため、
基板も同じ向きで置く前提とする。

資料が与えない値はNoneとし、推定で埋めない。Noneの項目を必要とする操作は、
利用側に値を求めるか例外を送出する。出典は各entryのsourceが持つ。
"""

from collections.abc import Mapping
from dataclasses import dataclass
from types import MappingProxyType

from .model import Box, Clearance, Direction, Feature, Keepout, Vec2, Vec3, boss, hole

__all__ = ["BOARDS", "Board", "BoardConnector", "MountingHole", "Source", "board", "board_ids"]


@dataclass(frozen=True)
class Source:
    """catalogの値の出典。参照した節まで記録し、再確認できる状態にする。"""

    title: str
    url: str
    # 資料自身が持つ版または更新日。資料に版の記載がない場合は取得日を記す。
    revision: str
    section: str


@dataclass(frozen=True)
class MountingHole:
    """PCBを貫通する取付穴。centerは基板座標系のx, y。

    資料が位置だけを寸法化し、径を与えない場合はdiameter_mmをNoneとする。
    """

    center: Vec2
    diameter_mm: float | None


@dataclass(frozen=True)
class BoardConnector:
    """基板の辺にあるコネクタの位置。

    edgeはコネクタのある辺の外向き法線、offset_mmはその辺に沿ったコネクタ中心の
    基板座標 (edgeがy方向ならx、x方向ならy)。資料は高さ方向の中心を寸法化しない
    ため持たない。
    """

    id: str
    edge: Direction
    offset_mm: float


@dataclass(frozen=True)
class Board:
    """1枚の基板。

    overall_height_mmはPCB下面から最も高い部品の頂点までを指す。資料が全高を
    与えない基板ではNoneとなり、envelopeやkeepoutの呼び出し側が値を指定する。
    """

    id: str
    name: str
    length_mm: float
    width_mm: float
    mounting_holes: tuple[MountingHole, ...]
    source: Source
    pcb_thickness_mm: float | None = None
    overall_height_mm: float | None = None
    # 資料が辺上の位置を寸法化したコネクタだけを持つ。
    connectors: tuple[BoardConnector, ...] = ()

    def __post_init__(self) -> None:
        if self.length_mm <= 0.0 or self.width_mm <= 0.0:
            raise ValueError(f"{self.id}: 外形は正の値である必要がある")
        for field_name, value in (("pcb_thickness_mm", self.pcb_thickness_mm),
                                  ("overall_height_mm", self.overall_height_mm)):
            if value is not None and value <= 0.0:
                raise ValueError(f"{self.id}: {field_name}は正の値である必要がある")
        if (self.pcb_thickness_mm is not None and self.overall_height_mm is not None
                and self.overall_height_mm < self.pcb_thickness_mm):
            raise ValueError(f"{self.id}: 全高がPCB板厚を下回っている")
        for index, item in enumerate(self.mounting_holes):
            if item.diameter_mm is not None and item.diameter_mm <= 0.0:
                raise ValueError(f"{self.id}: 取付穴{index}の直径が正でない")
            radius = 0.0 if item.diameter_mm is None else item.diameter_mm / 2.0
            x, y = item.center
            if not (radius <= x <= self.length_mm - radius
                    and radius <= y <= self.width_mm - radius):
                raise ValueError(f"{self.id}: 取付穴{index}が外形からはみ出している")
        seen = set()
        for item in self.connectors:
            if item.id in seen:
                raise ValueError(f"{self.id}: コネクタid {item.id} が重複している")
            seen.add(item.id)
            if item.edge.endswith("_z"):
                raise ValueError(f"{self.id}: コネクタ {item.id} のedgeはx又はy方向である必要がある")
            along = self.length_mm if item.edge.endswith("_y") else self.width_mm
            if not 0.0 <= item.offset_mm <= along:
                raise ValueError(f"{self.id}: コネクタ {item.id} が辺の範囲外にある")

    def connector(self, id: str) -> BoardConnector:
        """idでコネクタを引く。未登録の場合は候補を添えて送出する。"""
        for item in self.connectors:
            if item.id == id:
                return item
        known = ", ".join(item.id for item in self.connectors) or "なし"
        raise KeyError(f"{self.id}: 未登録のコネクタid {id!r}。登録済み: {known}")

    def connector_center(self, id: str, origin: Vec3, z_mm: float) -> Vec2:
        """コネクタ中心のmodel座標を、edgeの軸に垂直な平面の2座標で返す。

        並びはCylinderのcenterと同じで、connector_openingのcenterにそのまま渡せる。
        z_mmはプラグ中心の高さのmodel座標で、資料が与えないため呼び出し側が決める。
        """
        item = self.connector(id)
        x, y, _ = origin
        along = (x if item.edge.endswith("_y") else y) + item.offset_mm
        return (along, z_mm)

    def edge_position(self, edge: Direction, origin: Vec3 = (0.0, 0.0, 0.0)) -> float:
        """基板の辺のmodel座標。開口のplug_spanやwall_spanを決める基準に使う。"""
        x, y, _ = origin
        positions = {
            "minus_x": x, "plus_x": x + self.length_mm,
            "minus_y": y, "plus_y": y + self.width_mm,
        }
        if edge not in positions:
            raise ValueError(f"{self.id}: edgeはx又はy方向である必要がある")
        return positions[edge]

    def height(self, override_mm: float | None = None) -> float:
        """基板が占める高さ。overrideを優先し、無ければ資料の全高を使う。"""
        if override_mm is not None:
            if override_mm <= 0.0:
                raise ValueError(f"{self.id}: 高さは正の値である必要がある")
            return override_mm
        if self.overall_height_mm is None:
            raise ValueError(
                f"{self.id}: 出典が全高を与えていない。height_mmを指定する"
                f" (出典: {self.source.title})"
            )
        return self.overall_height_mm

    def envelope(self, origin: Vec3 = (0.0, 0.0, 0.0), height_mm: float | None = None) -> Box:
        """基板が占める直方体。originは基板座標系の原点に対応するmodel座標。"""
        x, y, z = origin
        return Box((x, y, z), (x + self.length_mm, y + self.width_mm, z + self.height(height_mm)))

    def keepout(
        self,
        id: str,
        origin: Vec3 = (0.0, 0.0, 0.0),
        height_mm: float | None = None,
        clearance: Clearance | None = None,
        access: tuple[Direction, ...] = (),
        attached_to: str | None = None,
    ) -> Keepout:
        """基板の占有領域をkeepoutとして返す。attached_toは基板を固定する部品のid。"""
        return Keepout(
            id,
            self.envelope(origin, height_mm),
            clearance if clearance is not None else Clearance(),
            access,
            attached_to,
        )

    def mount_centers(self, origin: Vec3 = (0.0, 0.0, 0.0)) -> tuple[Vec2, ...]:
        """取付穴中心のmodel座標。z方向は呼び出し側が決める。"""
        x, y, _ = origin
        return tuple((x + item.center[0], y + item.center[1]) for item in self.mounting_holes)

    def bosses(
        self,
        span: Vec2,
        diameter_mm: float,
        origin: Vec3 = (0.0, 0.0, 0.0),
        prefix: str = "pad",
    ) -> tuple[Feature, ...]:
        """各取付穴の位置に立てる支持pad。spanはz方向の範囲。"""
        return tuple(
            boss(f"{prefix}_{index}", "z", center, diameter_mm, span)
            for index, center in enumerate(self.mount_centers(origin))
        )

    def pilot_holes(
        self,
        span: Vec2,
        diameter_mm: float,
        origin: Vec3 = (0.0, 0.0, 0.0),
        prefix: str = "screw",
    ) -> tuple[Feature, ...]:
        """各取付穴の位置に開けるネジ下穴。spanはz方向の範囲。"""
        return tuple(
            hole(f"{prefix}_{index}", "z", center, diameter_mm, span)
            for index, center in enumerate(self.mount_centers(origin))
        )


def _centered_holes(
    length_mm: float,
    width_mm: float,
    length_pitch_mm: float,
    width_pitch_mm: float,
    diameter_mm: float,
) -> tuple[MountingHole, ...]:
    """外形の中心に対して対称な4穴を返す。pitchは各軸の穴間距離。"""
    x_offset = (length_mm - length_pitch_mm) / 2.0
    y_offset = (width_mm - width_pitch_mm) / 2.0
    return tuple(
        MountingHole((x, y), diameter_mm)
        for x in (x_offset, length_mm - x_offset)
        for y in (y_offset, width_mm - y_offset)
    )


def _grid_holes(
    xs: tuple[float, float],
    ys: tuple[float, float],
    diameter_mm: float | None,
) -> tuple[MountingHole, ...]:
    """x座標2つとy座標2つの組み合わせで決まる4穴を返す。"""
    return tuple(MountingHole((x, y), diameter_mm) for x in xs for y in ys)


_PICO_2 = Board(
    id="raspberry_pi_pico_2",
    name="Raspberry Pi Pico 2",
    length_mm=51.0,
    width_mm=21.0,
    # 図が与えるのは穴間ピッチ11.4 (短辺方向) と直径2.1。長辺方向のピッチ47は
    # 端からの2.0と外形51から定まる。PDFのベクタ座標でも同じ値を確認した。
    mounting_holes=_centered_holes(
        51.0, 21.0, length_pitch_mm=47.0, width_pitch_mm=11.4, diameter_mm=2.1
    ),
    pcb_thickness_mm=1.0,
    # 資料は部品高さを与えない。micro-USBとdebug端子の高さは記載がない。
    overall_height_mm=None,
    source=Source(
        title="Raspberry Pi Pico 2 Datasheet",
        url="https://datasheets.raspberrypi.com/pico/pico-2-datasheet.pdf",
        revision="Release 5 (build date 03/07/2026)",
        section="3. Mechanical specification / Figure 3",
    ),
)

_PI_5 = Board(
    id="raspberry_pi_5",
    name="Raspberry Pi 5",
    length_mm=85.0,
    width_mm=56.0,
    # 穴は長辺方向に非対称で、左端から3.5と61.5。短辺方向は3.5と52.5で対称。
    mounting_holes=(
        MountingHole((3.5, 3.5), 2.7),
        MountingHole((3.5, 52.5), 2.7),
        MountingHole((61.5, 3.5), 2.7),
        MountingHole((61.5, 52.5), 2.7),
    ),
    # 図はPCB板厚と部品高さのいずれも寸法線で与えない。
    pcb_thickness_mm=None,
    overall_height_mm=None,
    source=Source(
        title="Raspberry Pi 5 mechanical drawing",
        url="https://datasheets.raspberrypi.com/rpi5/raspberry-pi-5-mechanical-drawing.pdf",
        revision="版の記載なし (2026-09-18取得)",
        section="平面図 (Scale 1:1 @A4)",
    ),
)

_UNO_R4_MINIMA = Board(
    id="arduino_uno_r4_minima",
    name="Arduino UNO R4 Minima",
    length_mm=68.58,
    width_mm=53.34,
    # 4穴はいずれも非対称に配置される。図の15.24/13.97/17.78/7.62と、
    # 2x 2.54 (左側2穴の長辺からの距離、右側2穴の右端からの距離) から定まる。
    mounting_holes=(
        MountingHole((13.97, 2.54), 3.2),
        MountingHole((15.24, 50.8), 3.2),
        MountingHole((66.04, 7.62), 3.2),
        MountingHole((66.04, 35.56), 3.2),
    ),
    pcb_thickness_mm=1.0,
    # 側面図の全高。PCB下面から最も高い部品の頂点まで。
    overall_height_mm=8.5,
    source=Source(
        title="Arduino UNO R4 Minima Datasheet (SKU: ABX00080)",
        url="https://docs.arduino.cc/resources/datasheets/ABX00080-datasheet.pdf",
        revision="Modified 15/09/2026",
        section="11 Mounting Holes And Board Outline",
    ),
)

_PICO = Board(
    id="raspberry_pi_pico",
    name="Raspberry Pi Pico",
    length_mm=51.0,
    width_mm=21.0,
    # 図が与えるのは短辺方向の穴間11.4、長辺の端から穴中心までの2、直径2.1。
    # 長辺方向の穴間47は外形の中心に対する対称から定まり、PDFのベクタ座標で
    # 47.0 (短辺方向11.4を基準) を確認した。
    mounting_holes=_centered_holes(
        51.0, 21.0, length_pitch_mm=47.0, width_pitch_mm=11.4, diameter_mm=2.1
    ),
    # 本文の「51×21 mm 1 mm thick PCB」による。
    pcb_thickness_mm=1.0,
    overall_height_mm=None,
    source=Source(
        title="Raspberry Pi Pico Datasheet",
        url="https://datasheets.raspberrypi.com/pico/pico-datasheet.pdf",
        revision="Release 21 (build date 03/07/2026)",
        section="2. Mechanical specification / Figure 3",
    ),
)

_PICO_W = Board(
    id="raspberry_pi_pico_w",
    name="Raspberry Pi Pico W",
    length_mm=51.0,
    width_mm=21.0,
    # Picoと同じ寸法線 (穴間11.4、端から2、直径2.1)。ベクタ座標で長辺方向の
    # 穴間47.03、短辺方向11.40を確認した。
    mounting_holes=_centered_holes(
        51.0, 21.0, length_pitch_mm=47.0, width_pitch_mm=11.4, diameter_mm=2.1
    ),
    pcb_thickness_mm=1.0,
    overall_height_mm=None,
    source=Source(
        title="Raspberry Pi Pico W Datasheet",
        url="https://datasheets.raspberrypi.com/picow/pico-w-datasheet.pdf",
        revision="Release 7 (build date 03/07/2026)",
        section="2. Mechanical specification / Figure 3",
    ),
)

_PICO_2_W = Board(
    id="raspberry_pi_pico_2_w",
    name="Raspberry Pi Pico 2 W",
    length_mm=51.0,
    width_mm=21.0,
    # 図はPico Wと同一で、ベクタ座標も一致する。
    mounting_holes=_centered_holes(
        51.0, 21.0, length_pitch_mm=47.0, width_pitch_mm=11.4, diameter_mm=2.1
    ),
    pcb_thickness_mm=1.0,
    overall_height_mm=None,
    source=Source(
        title="Raspberry Pi Pico 2 W Datasheet",
        url="https://datasheets.raspberrypi.com/picow/pico-2-w-datasheet.pdf",
        revision="Release 2 (build date 03/07/2026)",
        section="2. Mechanical specification / Figure 3",
    ),
)

_PI_4B = Board(
    id="raspberry_pi_4_model_b",
    name="Raspberry Pi 4 Model B",
    length_mm=85.0,
    width_mm=56.0,
    # Pi 5と同じ配置。左端から3.5と3.5+58、下端から3.5と3.5+49。
    mounting_holes=_grid_holes((3.5, 61.5), (3.5, 52.5), 2.7),
    # 図は部品ごとにZ=の値を記すが、基準面を示さないため全高として採らない。
    pcb_thickness_mm=None,
    overall_height_mm=None,
    # 下辺は左端から3.5+7.7、+14.8、+13.5の連鎖寸法、右辺は下端からの寸法による。
    # USB-Aの2基はyの小さい順に0, 1とする。音声端子は寸法の対応が一意に読めないため持たない。
    connectors=(
        BoardConnector("usb_c_power", "minus_y", 11.2),
        BoardConnector("micro_hdmi_0", "minus_y", 26.0),
        BoardConnector("micro_hdmi_1", "minus_y", 39.5),
        BoardConnector("usb_a_0", "plus_x", 9.0),
        BoardConnector("usb_a_1", "plus_x", 27.0),
        BoardConnector("ethernet", "plus_x", 45.75),
    ),
    source=Source(
        title="Raspberry Pi 4 Model B mechanical drawing",
        url="https://datasheets.raspberrypi.com/rpi4/raspberry-pi-4-mechanical-drawing.pdf",
        revision="RP-008343-DS-1 (2026-09-28取得)",
        section="平面図",
    ),
)

_PI_3BP = Board(
    id="raspberry_pi_3_model_b_plus",
    name="Raspberry Pi 3 Model B+",
    length_mm=85.0,
    width_mm=56.0,
    # 配置はPi 4と同じで、穴径だけが2.75と記される。
    mounting_holes=_grid_holes((3.5, 61.5), (3.5, 52.5), 2.75),
    # Z-Heightの基準面が示されないため全高として採らない。
    pcb_thickness_mm=None,
    overall_height_mm=None,
    # 下辺は左端からの寸法、右辺は下端からの寸法による。USB-Aの2基はyの小さい順に0, 1とする。
    # Ethernetは10.25と11.5の2つの寸法が近接し、どちらが中心かを一意に読めないため持たない。
    connectors=(
        BoardConnector("micro_usb_power", "minus_y", 10.6),
        BoardConnector("hdmi", "minus_y", 32.0),
        BoardConnector("audio", "minus_y", 53.5),
        BoardConnector("usb_a_0", "plus_x", 29.0),
        BoardConnector("usb_a_1", "plus_x", 47.0),
    ),
    source=Source(
        title="Raspberry Pi 3 Model B+ mechanical drawing",
        url="https://datasheets.raspberrypi.com/rpi3/raspberry-pi-3-b-plus-mechanical-drawing.pdf",
        revision="RP-008337-DS-2 (2026-09-28取得)",
        section="平面図",
    ),
)

_ZERO_2_W = Board(
    id="raspberry_pi_zero_2_w",
    name="Raspberry Pi Zero 2 W",
    length_mm=65.0,
    width_mm=30.0,
    # 左右とも端から3.5、下端から3.5と3.5+23。図は穴径を寸法化していない。
    mounting_holes=_grid_holes((3.5, 61.5), (3.5, 26.5), None),
    pcb_thickness_mm=None,
    overall_height_mm=None,
    # 下辺の左端からの寸法による。
    connectors=(
        BoardConnector("mini_hdmi", "minus_y", 12.4),
        BoardConnector("micro_usb_data", "minus_y", 41.4),
        BoardConnector("micro_usb_power", "minus_y", 54.0),
    ),
    source=Source(
        title="Raspberry Pi Zero 2 W mechanical drawing",
        url="https://datasheets.raspberrypi.com/rpizero2/raspberry-pi-zero-2-w-mechanical-drawing.pdf",
        revision="RP-008358-DS-1 (2026-09-28取得)",
        section="平面図",
    ),
)

_UNO_R4_WIFI = Board(
    id="arduino_uno_r4_wifi",
    name="Arduino UNO R4 WiFi",
    length_mm=68.58,
    width_mm=53.34,
    # 資料の図はUNO R4 Minimaの図と同一の画像であり、値もMinimaと同じになる。
    mounting_holes=_UNO_R4_MINIMA.mounting_holes,
    pcb_thickness_mm=1.0,
    overall_height_mm=8.5,
    source=Source(
        title="Arduino UNO R4 WiFi Datasheet (SKU: ABX00087)",
        url="https://docs.arduino.cc/resources/datasheets/ABX00087-datasheet.pdf",
        revision="Modified 22/09/2026",
        section="13 Mounting Holes And Board Outline",
    ),
)

_NANO_EVERY = Board(
    id="arduino_nano_every",
    name="Arduino Nano Every",
    length_mm=43.18,
    width_mm=17.78,
    # 右上の穴が右端と上端から1.27。穴間は長辺方向40.64、短辺方向15.24。
    mounting_holes=_grid_holes((1.27, 41.91), (1.27, 16.51), 1.65),
    pcb_thickness_mm=None,
    overall_height_mm=None,
    source=Source(
        title="Arduino Nano Every Datasheet (SKU: ABX00028)",
        url="https://docs.arduino.cc/resources/datasheets/ABX00028-datasheet.pdf",
        revision="Modified 22/09/2026",
        section="7.1 Board Outline and Mounting Holes",
    ),
)

_NANO_33_IOT = Board(
    id="arduino_nano_33_iot",
    name="Arduino Nano 33 IoT",
    length_mm=43.16,
    width_mm=17.77,
    # 右上の穴が右端と上端から1.26。穴間は長辺方向40.64、短辺方向15.24。
    # 穴径はR0.83による。同じ資料の6.2節の図は4x Ø1.65と記し、一致しない。
    mounting_holes=_grid_holes((1.26, 41.9), (1.27, 16.51), 1.66),
    pcb_thickness_mm=None,
    overall_height_mm=None,
    source=Source(
        title="Arduino Nano 33 IoT Datasheet (SKU: ABX00027)",
        url="https://docs.arduino.cc/resources/datasheets/ABX00027-datasheet.pdf",
        revision="Modified 22/09/2026",
        section="6.1 Board Outline and Mounting Holes",
    ),
)

_MKR_WAN_1310 = Board(
    id="arduino_mkr_wan_1310",
    name="Arduino MKR WAN 1310",
    length_mm=67.7,
    width_mm=25.0,
    # 左上の穴が左端と上端から2.25。穴間は長辺方向57.00、短辺方向20.50。
    # 図は穴径を寸法化していない。
    mounting_holes=_grid_holes((2.25, 59.25), (2.25, 22.75), None),
    pcb_thickness_mm=None,
    overall_height_mm=None,
    source=Source(
        title="Arduino MKR WAN 1310 Datasheet (SKU: ABX00029)",
        url="https://docs.arduino.cc/resources/datasheets/ABX00029-datasheet.pdf",
        revision="Modified 22/09/2026",
        section="4.1 Board Outline / 4.2 Mounting Holes",
    ),
)

BOARDS: Mapping[str, Board] = MappingProxyType(
    {
        entry.id: entry
        for entry in (
            _PICO,
            _PICO_W,
            _PICO_2,
            _PICO_2_W,
            _ZERO_2_W,
            _PI_3BP,
            _PI_4B,
            _PI_5,
            _UNO_R4_MINIMA,
            _UNO_R4_WIFI,
            _NANO_EVERY,
            _NANO_33_IOT,
            _MKR_WAN_1310,
        )
    }
)


def board_ids() -> tuple[str, ...]:
    """登録済み基板のidを返す。"""
    return tuple(BOARDS)


def board(id: str) -> Board:
    """idで基板を引く。未登録の場合は候補を添えて送出する。"""
    try:
        return BOARDS[id]
    except KeyError:
        raise KeyError(f"未登録の基板id {id!r}。登録済み: {', '.join(BOARDS)}") from None
