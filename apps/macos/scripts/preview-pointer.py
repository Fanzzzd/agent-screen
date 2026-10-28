#!/usr/bin/env python3
"""Preview the original production cubic constants. No Cargo, GUI or cursor APIs.

The Rust source is the sole path/stroke-parameter input. This is an SVG
geometry preview, not a GPUI screenshot or a raster-equivalence test.
"""
import argparse
import hashlib
import html
import re
from pathlib import Path

NUMBER = r"[-+]?(?:\d+(?:\.\d*)?|\.\d+)"


def scalar(source, name):
    match = re.search(rf"pub const {name}: f64\s*=\s*({NUMBER})\s*;", source)
    if not match:
        raise ValueError(f"missing production scalar {name}")
    return float(match[1])


def vertex(source, name):
    match = re.search(rf"pub const {name}: Vertex\s*=\s*\(\s*({NUMBER})\s*,\s*({NUMBER})\s*\)\s*;", source)
    if not match:
        raise ValueError(f"missing production vertex {name}")
    return float(match[1]), float(match[2])


def load_geometry(path):
    raw = path.read_bytes()
    source = raw.decode("utf-8")
    body = source.split("pub const OUTLINE:", 1)[1].split("];", 1)[0]
    curves = []
    tip = vertex(source, "TIP")
    for block in re.findall(r"Cubic\s*\{([^{}]+)\}", body):
        curve = []
        for key in ("control_a", "control_b", "to"):
            match = re.search(rf"\b{key}:\s*\(\s*({NUMBER})\s*,\s*({NUMBER})\s*\)", block)
            if match:
                curve.append((float(match[1]), float(match[2])))
            elif key == "to" and re.search(r"\bto:\s*TIP\b", block):
                curve.append(tip)
            else:
                raise ValueError(f"unsupported production curve field {key}")
        curves.append(curve)
    if len(curves) != 8 or curves[-1][-1] != tip:
        raise ValueError("expected the complete closed production outline")
    constants = {name: scalar(source, name) for name in (
        "DESIGN_SIZE", "INNER_STROKE", "MIN_INNER_STROKE", "DARK_EDGE", "MIN_DARK_EDGE")}
    return tip, curves, constants, hashlib.sha256(raw).hexdigest()


def main():
    root = Path(__file__).resolve().parents[3]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--geometry", type=Path, default=root / "crates/model/src/pointer_geometry.rs")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    tip, curves, c, digest = load_geometry(args.geometry)
    args.output.mkdir(parents=True, exist_ok=True)
    elements = []

    def text(x, y, value, size=14, color="#646b78"):
        elements.append(f'<text x="{x}" y="{y}" font-size="{size}" fill="{color}">{html.escape(value)}</text>')

    def rect(x, y, w, h, color, radius=10, stroke="none"):
        elements.append(f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{radius}" fill="{color}" stroke="{stroke}"/>')

    def widths(size):
        inner = max(c["INNER_STROKE"] * size / c["DESIGN_SIZE"], c["MIN_INNER_STROKE"])
        return inner, inner + max(c["DARK_EDGE"] * size / c["DESIGN_SIZE"], c["MIN_DARK_EDGE"])

    def path(size):
        scale = size / c["DESIGN_SIZE"]
        p = lambda v: f"{v[0] * scale:.8g},{v[1] * scale:.8g}"
        return "M " + p(tip) + " " + " ".join("C " + " ".join(p(v) for v in curve) for curve in curves) + " Z"

    def glyph(x, y, size, guides=True):
        inner, outer = widths(size)
        d = path(size)
        elements.append(f'<g transform="translate({x:.8g},{y:.8g})">')
        # Match the production core order: dark stroke, fill, light rim.
        # Shadows/glow and GPUI rasterization remain outside this preview.
        elements.append(f'<path d="{d}" fill="none" stroke="#11151b" stroke-opacity="0.82" stroke-width="{outer:.8g}"/>')
        elements.append(f'<path d="{d}" fill="url(#accent)"/>')
        elements.append(f'<path d="{d}" fill="none" stroke="#fff" stroke-opacity="0.95" stroke-width="{inner:.8g}"/>')
        if guides:
            elements.append('<g class="hotspot" stroke="#f14e75" stroke-width="0.75" fill="none"><path d="M -4,0 H 4 M 0,-4 V 4"/><circle r="1.1"/></g>')
        elements.append('</g>')

    def in_canvas(x, y, size):
        # Position the conservative production control hull plus stroke.
        # No alternate icon path is authored for the preview.
        points = [tip] + [v for curve in curves for v in curve]
        k = size / c["DESIGN_SIZE"]
        half = widths(size)[1] / 2
        left = min(v[0] for v in points) * k - half
        top = min(v[1] for v in points) * k - half
        w = (max(v[0] for v in points) - min(v[0] for v in points)) * k + 2 * half
        h = (max(v[1] for v in points) - min(v[1] for v in points)) * k + 2 * half
        glyph(x + (size - w) / 2 - left, y + (size - h) / 2 - top, size)

    elements.append('<svg xmlns="http://www.w3.org/2000/svg" width="1120" height="740" viewBox="0 0 1120 740" font-family="-apple-system,BlinkMacSystemFont,sans-serif">')
    elements.append(f'<metadata>Original production cubic constants. Source SHA256 {digest}</metadata>')
    elements.append('<defs><linearGradient id="accent" x1="0" y1="0" x2="0.45" y2="1"><stop stop-color="#327fda" offset="0.1"/><stop stop-color="#e6dff8" offset="0.95"/></linearGradient></defs>')
    rect(0, 0, 1120, 740, "#f3f4f7", 0)
    text(28, 36, "Rounded Agent Pointer / 原创圆润指针", 24, "#18212e")
    text(28, 62, "Production cubic controls / 读取生产控制点 · Example accent / 示例颜色 · SVG preview, not a GPUI screenshot", 13)
    text(28, 84, "No third-party cursor paths or assets / 未使用第三方指针路径或素材 · Crosshair is preview-only / 十字仅用于预览", 13)
    for column, (background, label, foreground) in enumerate((
        ("#fafbfe", "Light / 浅色内容", "#344054"), ("#1d2027", "Dark / 深色内容", "#c8cfdb"))):
        x = 28 + column * 550
        rect(x, 105, 514, 565, background, 14, "#cbd1dc")
        text(x + 18, 136, label, 17, foreground)
        text(x + 18, 162, "18 / 24 / 32 logical px at 100% / 100%逻辑像素画布", 12, foreground)
        for i, size in enumerate((18, 24, 32)):
            cx = x + 54 + i * 146
            rect(cx - 12, 180, 90, 84, background, 5, "#808898")
            rect(cx, 197, size, size, "none", 0, "#969eaf")
            in_canvas(cx, 197, size)
            text(cx - 2, 250, f"{size} px", 12, foreground)
        text(x + 18, 294, "3× geometry detail / 三倍几何细节", 13, foreground)
        glyph(x + 170, 316, 96)
        text(x + 18, 452, "Image-boundary clipping / 图像边界裁切", 13, foreground)
        clip_id = f"image-{column}"
        elements.append(f'<defs><clipPath id="{clip_id}"><rect x="{x + 34}" y="474" width="438" height="142"/></clipPath></defs>')
        rect(x + 18, 462, 478, 176, "#788294", 6)
        rect(x + 34, 474, 438, 142, background, 0)
        elements.append(f'<g clip-path="url(#{clip_id})">')
        for dx, dy in ((0, 0), (436, 0), (0, 140), (436, 140)):
            glyph(x + 34 + dx, 474 + dy, 24)
        elements.append('</g>')
    text(28, 702, "Hotspot = production TIP (0,0). Size, stroke minima and curve commands come from the Rust source.", 12)
    text(28, 724, f"Source SHA256: {digest}", 11)
    elements.append('</svg>')
    svg = "\n".join(elements)
    (args.output / "preview.svg").write_text(svg)
    page = '''<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Original rounded pointer preview</title><style>body{margin:20px;background:#eef0f4;color:#263141;font:14px -apple-system,BlinkMacSystemFont,sans-serif}label{display:block;margin:0 0 14px}main{overflow:auto}svg{display:block;width:1120px;height:740px}body.hide-hotspot .hotspot{display:none}</style><label><input id="guides" type="checkbox" checked> Hotspot guides / 热点辅助线</label><main>''' + svg + '''</main><script>document.getElementById('guides').addEventListener('change',e=>document.body.classList.toggle('hide-hotspot',!e.target.checked));</script></html>'''
    (args.output / "index.html").write_text(page)
    print(args.output / "index.html")
    print(args.output / "preview.svg")
    print(f"source_sha256={digest}; cubic_segments={len(curves)}; original_geometry=true")


if __name__ == "__main__":
    main()
