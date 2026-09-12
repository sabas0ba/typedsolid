"""出力済みbinary STLを描画する。matplotlib/numpyは既存CadQuery lockの依存を使用する。"""

import argparse
import json
from pathlib import Path
import struct

import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
import numpy as np


def load_stl(path: Path) -> np.ndarray:
    data = path.read_bytes()
    if len(data) < 84:
        raise ValueError("binary STL header is incomplete")
    count = struct.unpack_from("<I", data, 80)[0]
    if not count or len(data) != 84 + count * 50:
        raise ValueError("binary STL triangle count does not match file length")
    records = np.frombuffer(data, dtype=np.dtype([
        ("normal", "<f4", (3,)), ("vertices", "<f4", (3, 3)), ("attribute", "<u2"),
    ]), offset=84, count=count)
    triangles = records["vertices"].astype(float)
    if not np.isfinite(triangles).all():
        raise ValueError("STL contains non-finite vertices")
    return triangles


def rasterize(triangles, colors, elevation, azimuth, width, height):
    """正射影とdepth bufferでSTLの可視面を描く。座標・法線は元meshから取得する。"""
    elev, azim = np.radians([elevation, azimuth])
    direction = np.array([np.cos(elev) * np.cos(azim), np.cos(elev) * np.sin(azim), np.sin(elev)])
    horizontal = np.array([-np.sin(azim), np.cos(azim), 0])
    vertical = np.cross(direction, horizontal)
    basis = np.stack((horizontal, vertical, direction), axis=1)
    projected = triangles @ basis
    lo, hi = projected.min(axis=(0, 1)), projected.max(axis=(0, 1))
    scale = min((width - 40) / (hi[0] - lo[0]), (height - 40) / (hi[1] - lo[1]))
    center = (lo + hi) / 2

    def project(points):
        coords = np.asarray(points) @ basis
        coords[..., 0] = (coords[..., 0] - center[0]) * scale + width / 2
        coords[..., 1] = -(coords[..., 1] - center[1]) * scale + height / 2
        return coords

    pixels = project(triangles)
    canvas = np.full((height, width, 3), [244, 247, 250], dtype=np.uint8)
    depth = np.full((height, width), -np.inf)
    for vertices, color in zip(pixels, colors):
        x0, y0 = np.maximum(np.floor(vertices[:, :2].min(axis=0)).astype(int), 0)
        x1, y1 = np.minimum(np.ceil(vertices[:, :2].max(axis=0)).astype(int), [width - 1, height - 1])
        if x1 < x0 or y1 < y0:
            continue
        a, b, c = vertices
        denominator = (b[1] - c[1]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[1] - c[1])
        if abs(denominator) < 1e-10:
            continue
        x, y = np.meshgrid(np.arange(x0, x1 + 1) + 0.5, np.arange(y0, y1 + 1) + 0.5)
        wa = ((b[1] - c[1]) * (x - c[0]) + (c[0] - b[0]) * (y - c[1])) / denominator
        wb = ((c[1] - a[1]) * (x - c[0]) + (a[0] - c[0]) * (y - c[1])) / denominator
        wc = 1 - wa - wb
        z = wa * a[2] + wb * b[2] + wc * c[2]
        region = depth[y0:y1 + 1, x0:x1 + 1]
        visible = (wa >= -1e-8) & (wb >= -1e-8) & (wc >= -1e-8) & (z > region)
        region[visible] = z[visible]
        canvas[y0:y1 + 1, x0:x1 + 1][visible] = (color * 255).astype(np.uint8)
    return canvas, project


def render(stl: Path, output: Path, model_path: Path | None, title: str) -> None:
    triangles = load_stl(stl)
    lower = triangles.min(axis=(0, 1))
    upper = triangles.max(axis=(0, 1))
    size = upper - lower
    if np.any(size <= 0):
        raise ValueError("STL must have non-zero extent on every axis")
    normals = np.cross(triangles[:, 1] - triangles[:, 0], triangles[:, 2] - triangles[:, 0])
    lengths = np.linalg.norm(normals, axis=1)
    if np.any(lengths == 0):
        raise ValueError("STL contains degenerate triangles")
    normals /= lengths[:, None]
    light = np.array([-0.35, -0.45, 0.82])
    light /= np.linalg.norm(light)
    shades = 0.55 + 0.45 * np.maximum(normals @ light, 0)
    height_tint = 0.65 + 0.35 * (triangles[:, :, 2].mean(axis=1) - lower[2]) / size[2]
    colors = (shades * height_tint)[:, None] * np.array([0.28, 0.63, 0.83])
    fig = plt.figure(figsize=(14, 8), facecolor="#f4f7fa")
    grid = fig.add_gridspec(2, 3, width_ratios=(1, 1, 1.1), height_ratios=(1, 0.65))
    ax = fig.add_subplot(grid[:, :2])
    top = fig.add_subplot(grid[0, 2])
    main_image, _ = rasterize(triangles, colors, 65, -58, 1000, 720)
    top_image, top_project = rasterize(triangles, colors, 90, -90, 540, 400)
    ax.imshow(main_image)
    top.imshow(top_image)
    for view in (ax, top):
        view.set_axis_off()
    top.set_title("Top view / PCB reserved volume", fontsize=12, color="#34495e")
    model = json.loads(model_path.read_text()) if model_path else None
    if model:
        for keepout in model["keepouts"]:
            lo, hi = keepout["bounds"]["min"], keepout["bounds"]["max"]
            outline = top_project([
                [lo[0], lo[1], hi[2]], [hi[0], lo[1], hi[2]],
                [hi[0], hi[1], hi[2]], [lo[0], hi[1], hi[2]], [lo[0], lo[1], hi[2]],
            ])
            top.plot(outline[:, 0], outline[:, 1], color="#20a263", linestyle="--", linewidth=1.6)
    detail = fig.add_subplot(grid[1, 2])
    detail.set_axis_off()
    lines = [
        "EXPORTED GEOMETRY",
        f"{size[0]:g} x {size[1]:g} x {size[2]:g} mm",
        f"{len(triangles):,} STL triangles",
        "",
    ]
    if model:
        cylinders = [f for p in model["parts"] for f in p["features"] if "radius_mm" in f["bounds"]]
        bosses = [f for f in cylinders if f["operation"] == "add"]
        holes = [f for f in cylinders if f["operation"] == "cut"]
        lines += [f"{len(bosses)} cylindrical supports", f"{len(holes)} cylindrical cuts"]
        for keepout in model["keepouts"]:
            lo, hi = keepout["bounds"]["min"], keepout["bounds"]["max"]
            dimensions = " x ".join(f"{b-a:g}" for a, b in zip(lo, hi))
            lines += [f"{keepout['id'].upper()}: {dimensions} mm (reserved)"]
    detail.text(0.04, 0.95, "\n".join(lines), va="top", fontsize=12, linespacing=1.65, color="#34495e")
    fig.suptitle(title, x=0.055, y=0.95, ha="left", fontsize=23, color="#223447", weight="bold")
    fig.text(0.055, 0.89, "Open enclosure with mounting holes, connector opening and ventilation slots",
             fontsize=12, color="#5b6e80")
    fig.text(0.055, 0.065, "Rendered from the exported STL. Dashed green outline is reserved space, not exported geometry.",
             fontsize=10, color="#5b6e80")
    fig.subplots_adjust(left=0.015, right=0.97, bottom=0.12, top=0.84, wspace=0.03, hspace=0.08)
    output.parent.mkdir(parents=True, exist_ok=True)
    fig.savefig(output, dpi=120, facecolor=fig.get_facecolor())
    plt.close(fig)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--stl", type=Path, required=True)
    parser.add_argument("--model-json", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--title", default="TypedSolid / Electronics enclosure")
    args = parser.parse_args()
    render(args.stl, args.output, args.model_json, args.title)
