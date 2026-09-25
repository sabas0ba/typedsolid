"""OCCTの隠線処理で最終shapeをSVGにする。OpenGLは不要。"""

import argparse
from pathlib import Path
import sys
import xml.etree.ElementTree as ET

import cadquery as cq

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from examples.board_tray import board_tray
from examples.electronics_enclosure import electronics_enclosure
from typedsolid.cadquery import build

NS = "http://www.w3.org/2000/svg"
ET.register_namespace("", NS)

# 作例ごとのモデル、描く部品、左図の見出し、下端の説明。
EXAMPLES = {
    "board_tray": (
        board_tray, "board_tray", "Final solid | 60 x 40 x 20 mm",
        "TypedSolid: four cylindrical pads with screw holes; open top; +Z access verified. PCB volume is not exported.",
    ),
    "electronics_enclosure": (
        electronics_enclosure, "electronics_enclosure", "Final solid | 80 x 55 x 24 mm",
        "TypedSolid: bosses with through holes, connector notch and vents; +Z access verified. PCB volume is not exported.",
    ),
}


def render(example, output):
    factory, part_id, heading, description = EXAMPLES[example]
    model = factory()
    result = build(model)
    if not result.export_allowed:
        raise ValueError(result.report)
    document = ET.Element(f"{{{NS}}}svg", width="1400", height="700", viewBox="0 0 1400 700")
    ET.SubElement(document, f"{{{NS}}}rect", width="1400", height="700", fill="#f5f7fa")
    shape = result.shapes[part_id]
    for index in range(2):
        options = {
            "width": 660, "height": 540, "marginLeft": 35, "marginTop": 35,
            "projectionDir": (1, -1, 1.8), "showAxes": False,
            "showHidden": False, "strokeColor": (45, 65, 85), "strokeWidth": 0.35,
        }
        svg = ET.fromstring(cq.exporters.getSVG(shape, options))
        svg.set("x", str(20 + index * 700))
        svg.set("y", "95")
        if index == 1:
            outer = svg.find(f"{{{NS}}}g")
            overlay = ET.SubElement(outer, f"{{{NS}}}g", stroke="#209059", fill="none", **{"stroke-width": "0.4"})
            for keepout in model.keepouts:
                # keepoutはboxに限定されている。
                lo, hi = keepout.shape.min, keepout.shape.max
                bounds = cq.Solid.makeBox(*(hi[i] - lo[i] for i in range(3)), pnt=cq.Vector(*lo))
                region_svg = ET.fromstring(cq.exporters.getSVG(bounds, options))
                for path in region_svg.iter(f"{{{NS}}}path"):
                    ET.SubElement(overlay, f"{{{NS}}}path", d=path.attrib["d"])
        document.append(svg)
        title = ET.SubElement(document, f"{{{NS}}}text", x=str(40 + index * 700), y="55", fill="#293a4c", **{"font-family": "sans-serif", "font-size": "24"})
        title.text = heading if index == 0 else "PCB reserved volume (green overlay)"
    caption = ET.SubElement(document, f"{{{NS}}}text", x="40", y="665", fill="#40546a", **{"font-family": "sans-serif", "font-size": "20"})
    caption.text = description
    output.parent.mkdir(parents=True, exist_ok=True)
    ET.indent(document, space="  ")
    ET.ElementTree(document).write(output, encoding="utf-8", xml_declaration=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--example", choices=sorted(EXAMPLES), default="board_tray")
    parser.add_argument("--output", type=Path, help="既定は.work/<作例名>.svg")
    args = parser.parse_args()
    # 作例ごとに既定の出力先を分け、別の作例のpreviewを上書きしない。
    output = args.output or Path(".work") / f"{args.example.replace('_', '-')}.svg"
    render(args.example, output)
