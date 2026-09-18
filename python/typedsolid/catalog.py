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

__all__ = ["BOARDS", "Board", "MountingHole", "Source", "board", "board_ids"]


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
    """PCBを貫通する取付穴。centerは基板座標系のx, y。"""

    center: Vec2
    diameter_mm: float


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
            if item.diameter_mm <= 0.0:
                raise ValueError(f"{self.id}: 取付穴{index}の直径が正でない")
            radius = item.diameter_mm / 2.0
            x, y = item.center
            if not (radius <= x <= self.length_mm - radius
                    and radius <= y <= self.width_mm - radius):
                raise ValueError(f"{self.id}: 取付穴{index}が外形からはみ出している")

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
    ) -> Keepout:
        """基板の占有領域をkeepoutとして返す。"""
        return Keepout(
            id,
            self.envelope(origin, height_mm),
            clearance if clearance is not None else Clearance(),
            access,
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

BOARDS: Mapping[str, Board] = MappingProxyType(
    {entry.id: entry for entry in (_PICO_2, _PI_5, _UNO_R4_MINIMA)}
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
