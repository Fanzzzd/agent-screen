"""The menu bar icon's states, 18x18 pt template glyphs: two screens and a
pointer, as the app icon. Built from strokes and boolean ops, so a badge
clears the lines under it, and written as one filled path each, as
NSImage renders an SVG mask at 18 px and blurs it.

    pip install skia-pathops
    python3 assets/status/make.py assets/status
"""
import sys, pathops
from pathops import Path, PathOp, LineCap, LineJoin, op

K = 0.5523
out = sys.argv[1]
W = 1.7          # stroke: as heavy as the Wi-Fi and battery glyphs beside it
GAP = 1.4        # clearance around a badge

def rrect(x, y, w, h, r):
    p = Path(); pen = p.getPen()
    pen.moveTo((x + r, y)); pen.lineTo((x + w - r, y))
    pen.curveTo((x + w - r + r*K, y), (x + w, y + r - r*K), (x + w, y + r))
    pen.lineTo((x + w, y + h - r))
    pen.curveTo((x + w, y + h - r + r*K), (x + w - r + r*K, y + h), (x + w - r, y + h))
    pen.lineTo((x + r, y + h))
    pen.curveTo((x + r - r*K, y + h), (x, y + h - r + r*K), (x, y + h - r))
    pen.lineTo((x, y + r))
    pen.curveTo((x, y + r - r*K), (x + r - r*K, y), (x + r, y))
    pen.closePath()
    return p

def poly(points):
    p = Path(); pen = p.getPen()
    pen.moveTo(points[0])
    for q in points[1:]: pen.lineTo(q)
    pen.closePath()
    return p

def stroked(p, width=W, cap=LineCap.ROUND_CAP, join=LineJoin.ROUND_JOIN):
    q = Path(p); q.stroke(width, cap, join, 4); q.convertConicsToQuads(); return q

def union(*ps):
    acc = Path()
    for p in ps: acc = op(acc, p, PathOp.UNION)
    return acc
def minus(a, b): return op(a, b, PathOp.DIFFERENCE)
def grown(p, by): return union(p, stroked(p, 2 * by))

def pointer(x, y, s):
    # The macOS arrow, tip at x, y, pointing up-left; slightly rounded.
    pts = [(0, 0), (0, 10.4), (2.55, 8.05), (4.3, 12.0), (6.05, 11.25), (4.35, 7.4), (7.75, 7.4)]
    p = poly([(x + px * s, y + py * s) for px, py in pts])
    return union(p, stroked(p, 0.6 * s))

def hand(x, y, s):
    parts = [rrect(x + 0.0*s, y + 2.4*s, 1.6*s, 4.6*s, 0.8*s),
             rrect(x + 1.8*s, y + 0.5*s, 1.6*s, 6.2*s, 0.8*s),
             rrect(x + 3.6*s, y + 0.0*s, 1.6*s, 6.6*s, 0.8*s),
             rrect(x + 5.4*s, y + 0.9*s, 1.6*s, 5.8*s, 0.8*s),
             rrect(x + 0.0*s, y + 4.6*s, 7.0*s, 5.6*s, 2.8*s)]
    thumb = stroked(poly([(x + 6.2*s, y + 7.6*s), (x + 7.9*s, y + 5.6*s)]), 1.6*s)
    return union(*parts, thumb)

BACK = rrect(1.25, 2.25, 10.0, 7.5, 1.6)
FRONT = rrect(4.25, 5.0, 11.0, 8.0, 1.9)
front_solid = union(FRONT, stroked(FRONT))          # its outer edge, filled
back = minus(stroked(BACK), grown(front_solid, GAP - W/2 + 0.2))
front_line = stroked(FRONT)

def badge(screens, mark):
    return union(minus(screens, grown(mark, GAP)), mark)

icons = {
    # No agent display open: two screens, a pointer at the front one's corner.
    "idle": badge(union(back, front_line), pointer(10.4, 7.8, 0.72)),
    # Agents at work: idle with the front screen lit.
    "working": badge(union(back, front_solid), pointer(10.4, 7.8, 0.72)),
    # An agent asks for help: a raised hand at the corner.
    "asking": badge(union(back, front_line), hand(10.0, 7.2, 0.82)),
    # Permissions missing: a slash through both screens.
    "setup": badge(union(back, front_line), stroked(poly([(2.0, 1.6), (16.0, 15.6)]), 1.5)),
}

def svg_path(p):
    d = []
    for verb, pts in p:
        f = lambda q: f"{q[0]:.3f} {q[1]:.3f}"
        name = verb.name if hasattr(verb, "name") else str(verb)
        if name == "MOVE": d.append("M" + f(pts[0]))
        elif name == "LINE": d.append("L" + f(pts[0]))
        elif name == "QUAD": d.append("Q" + f(pts[0]) + " " + f(pts[1]))
        elif name == "CUBIC": d.append("C" + " ".join(f(q) for q in pts))
        elif name == "CLOSE": d.append("Z")
        elif name == "CONIC": d.append("Q" + f(pts[0]) + " " + f(pts[1]))
    return "".join(d)

def ink_centroid(p, size=180):
    """The center of the glyph's ink, from a raster of it: where the eye
    puts the middle, unlike the bounding box, which the pointer drags."""
    import subprocess, io
    from PIL import Image
    svg = f'<svg xmlns="http://www.w3.org/2000/svg" width="18" height="18" viewBox="0 0 18 18"><path d="{svg_path(p)}"/></svg>'
    png = subprocess.run(["rsvg-convert", "-w", str(size), "-h", str(size)], input=svg.encode(), capture_output=True, check=True).stdout
    alpha = Image.open(io.BytesIO(png)).getchannel("A")
    total = sx = sy = 0
    for y in range(size):
        for x in range(size):
            a = alpha.getpixel((x, y))
            total += a; sx += a * x; sy += a * y
    k = 18 / size
    return (sx / total + 0.5) * k, (sy / total + 0.5) * k

# One fit for every state, from idle, so switching states moves nothing:
# the glyph's ink centers on the middle, where the eye puts it (two screens
# above, a thin pointer below: the bounding box would sit high), and it is
# as large as that leaves room for inside the 18 pt box, 0.25 pt to spare.
from fontTools.pens.transformPen import TransformPen
def place(p, scale, dx, dy):
    q = Path(); pen = q.getPen()
    p.draw(TransformPen(pen, (scale, 0, 0, scale, dx, dy)))
    return q
cx, cy = ink_centroid(icons["idle"])
x0, y0, x1, y1 = icons["idle"].bounds
reach = max(cx - x0, x1 - cx, cy - y0, y1 - cy)
scale = 8.75 / reach
dx, dy = 9 - cx * scale, 9 - cy * scale
icons = {name: place(p, scale, dx, dy) for name, p in icons.items()}
print(f"ink center {cx:.2f} {cy:.2f}, scale {scale:.3f}")

for name, p in icons.items():
    p.simplify()
    with open(f"{out}/{name}.svg", "w") as fh:
        fh.write(f'<svg xmlns="http://www.w3.org/2000/svg" width="18" height="18" viewBox="0 0 18 18"><path d="{svg_path(p)}"/></svg>\n')
print("wrote", list(icons))
