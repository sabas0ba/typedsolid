import unittest

from typedsolid import Box, Clearance, MountingHole, Source, board, board_ids
from typedsolid.catalog import BOARDS, Board

# 出典が寸法線として与える外形。catalogへの転記誤りを検出する。
OUTLINES_MM = {
    "raspberry_pi_pico_2": (51.0, 21.0),
    "raspberry_pi_5": (85.0, 56.0),
    "arduino_uno_r4_minima": (68.58, 53.34),
}


def source() -> Source:
    return Source("t", "https://example.invalid/d.pdf", "rev", "sec")


class RegistryTests(unittest.TestCase):
    def test_registered_ids_match_outline_table(self):
        self.assertEqual(set(board_ids()), set(OUTLINES_MM))

    def test_outline_matches_source(self):
        for board_id, outline in OUTLINES_MM.items():
            with self.subTest(board=board_id):
                entry = board(board_id)
                self.assertEqual((entry.length_mm, entry.width_mm), outline)

    def test_every_board_cites_a_source(self):
        for board_id in OUTLINES_MM:
            with self.subTest(board=board_id):
                cited = board(board_id).source
                self.assertTrue(cited.url.startswith("https://"))
                for field in (cited.title, cited.revision, cited.section):
                    self.assertTrue(field)

    def test_unknown_id_lists_the_registered_ones(self):
        with self.assertRaises(KeyError) as caught:
            board("raspberry_pi_4")
        for known in board_ids():
            self.assertIn(known, str(caught.exception))

    def test_registry_is_read_only(self):
        with self.assertRaises(TypeError):
            BOARDS["raspberry_pi_pico_2"] = None


class MountingHoleTests(unittest.TestCase):
    def test_holes_lie_inside_the_outline(self):
        for board_id in OUTLINES_MM:
            entry = board(board_id)
            self.assertEqual(len(entry.mounting_holes), 4)
            for index, item in enumerate(entry.mounting_holes):
                with self.subTest(board=board_id, hole=index):
                    radius = item.diameter_mm / 2.0
                    self.assertLessEqual(radius, item.center[0])
                    self.assertLessEqual(item.center[0], entry.length_mm - radius)
                    self.assertLessEqual(radius, item.center[1])
                    self.assertLessEqual(item.center[1], entry.width_mm - radius)

    def test_pico_2_pitch_matches_the_datasheet_figure(self):
        """図が与えるのは短辺方向11.4。長辺方向47は端からの2.0と外形51から定まる。"""
        entry = board("raspberry_pi_pico_2")
        xs = sorted({item.center[0] for item in entry.mounting_holes})
        ys = sorted({item.center[1] for item in entry.mounting_holes})
        self.assertEqual(xs, [2.0, 49.0])
        self.assertEqual(ys, [4.8, 16.2])
        self.assertAlmostEqual(xs[1] - xs[0], 47.0)
        self.assertAlmostEqual(ys[1] - ys[0], 11.4)
        self.assertEqual({item.diameter_mm for item in entry.mounting_holes}, {2.1})

    def test_pi_5_holes_are_asymmetric_along_the_long_edge(self):
        """長辺方向は左端から3.5と61.5で、右端からは23.5離れる。"""
        entry = board("raspberry_pi_5")
        centers = sorted(item.center for item in entry.mounting_holes)
        self.assertEqual(centers, [(3.5, 3.5), (3.5, 52.5), (61.5, 3.5), (61.5, 52.5)])
        self.assertEqual({item.diameter_mm for item in entry.mounting_holes}, {2.7})

    def test_uno_r4_minima_holes_match_the_uno_form_factor(self):
        entry = board("arduino_uno_r4_minima")
        centers = sorted(item.center for item in entry.mounting_holes)
        self.assertEqual(centers, [(13.97, 2.54), (15.24, 50.8), (66.04, 7.62), (66.04, 35.56)])
        self.assertEqual(entry.pcb_thickness_mm, 1.0)
        self.assertEqual(entry.overall_height_mm, 8.5)


class HeightTests(unittest.TestCase):
    def test_source_without_a_height_requires_an_override(self):
        entry = board("raspberry_pi_5")
        with self.assertRaisesRegex(ValueError, "全高"):
            entry.height()
        self.assertEqual(entry.height(12.0), 12.0)

    def test_override_takes_precedence(self):
        entry = board("arduino_uno_r4_minima")
        self.assertEqual(entry.height(), 8.5)
        self.assertEqual(entry.height(20.0), 20.0)

    def test_non_positive_override(self):
        for value in [0.0, -1.0]:
            with self.subTest(value=value), self.assertRaisesRegex(ValueError, "正の値"):
                board("raspberry_pi_5").height(value)


class GeometryTests(unittest.TestCase):
    def test_envelope_follows_the_origin_and_height(self):
        entry = board("arduino_uno_r4_minima")
        self.assertEqual(entry.envelope(), Box((0.0, 0.0, 0.0), (68.58, 53.34, 8.5)))
        self.assertEqual(
            entry.envelope((10.0, 5.0, 2.0)), Box((10.0, 5.0, 2.0), (78.58, 58.34, 10.5))
        )
        self.assertEqual(
            entry.envelope((0.0, 0.0, 0.0), 3.0), Box((0.0, 0.0, 0.0), (68.58, 53.34, 3.0))
        )

    def test_keepout_carries_clearance_and_access(self):
        keepout = board("raspberry_pi_pico_2").keepout(
            "pcb", (1.0, 2.0, 3.0), height_mm=6.0,
            clearance=Clearance(default=0.5, minus_z=0.0), access=("plus_z",),
        )
        self.assertEqual(keepout.id, "pcb")
        self.assertEqual(keepout.shape, Box((1.0, 2.0, 3.0), (52.0, 23.0, 9.0)))
        self.assertEqual(keepout.clearance_mm.minus_z, 0.0)
        self.assertEqual(keepout.access, ("plus_z",))

    def test_keepout_defaults_to_a_plain_clearance(self):
        keepout = board("arduino_uno_r4_minima").keepout("pcb")
        self.assertEqual(keepout.clearance_mm, Clearance())
        self.assertEqual(keepout.access, ())

    def test_mount_centers_are_offset_by_the_origin(self):
        moved = board("raspberry_pi_pico_2").mount_centers((10.0, 20.0, 30.0))
        self.assertEqual(
            sorted(moved), [(12.0, 24.8), (12.0, 36.2), (59.0, 24.8), (59.0, 36.2)]
        )

    def test_bosses_and_pilot_holes_share_the_hole_positions(self):
        entry = board("raspberry_pi_pico_2")
        origin = (4.5, 9.5, 4.0)
        pads = entry.bosses((0.0, 4.0), 5.0, origin)
        screws = entry.pilot_holes((-1.0, 4.0), 1.6, origin)

        self.assertEqual(len(pads), 4)
        self.assertEqual(len(screws), 4)
        self.assertEqual([pad.shape.center for pad in pads], list(entry.mount_centers(origin)))
        self.assertEqual(
            [pad.shape.center for pad in pads], [screw.shape.center for screw in screws]
        )
        self.assertEqual({pad.operation for pad in pads}, {"add"})
        self.assertEqual({screw.operation for screw in screws}, {"cut"})
        self.assertEqual({pad.shape.radius for pad in pads}, {2.5})
        self.assertEqual({screw.shape.radius for screw in screws}, {0.8})
        self.assertEqual({pad.id for pad in pads}, {f"pad_{index}" for index in range(4)})
        self.assertEqual({screw.id for screw in screws}, {f"screw_{index}" for index in range(4)})

    def test_prefix_keeps_ids_unique_across_two_boards(self):
        entry = board("raspberry_pi_pico_2")
        first = entry.bosses((0.0, 4.0), 5.0, (0.0, 0.0, 0.0), prefix="left_pad")
        second = entry.bosses((0.0, 4.0), 5.0, (60.0, 0.0, 0.0), prefix="right_pad")
        self.assertTrue({item.id for item in first}.isdisjoint({item.id for item in second}))


class ValidationTests(unittest.TestCase):
    def test_hole_outside_the_outline(self):
        with self.assertRaisesRegex(ValueError, "はみ出"):
            Board("x", "X", 10.0, 10.0, (MountingHole((9.9, 5.0), 3.0),), source())

    def test_non_positive_outline(self):
        with self.assertRaisesRegex(ValueError, "外形"):
            Board("x", "X", 0.0, 10.0, (), source())

    def test_height_below_the_pcb_thickness(self):
        with self.assertRaisesRegex(ValueError, "板厚"):
            Board("x", "X", 10.0, 10.0, (), source(), pcb_thickness_mm=2.0, overall_height_mm=1.0)

    def test_non_positive_hole_diameter(self):
        with self.assertRaisesRegex(ValueError, "直径"):
            Board("x", "X", 10.0, 10.0, (MountingHole((5.0, 5.0), 0.0),), source())


if __name__ == "__main__":
    unittest.main()
