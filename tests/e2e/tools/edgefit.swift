// edgefit: how closely the Input Grant's red edge fits a display's glass.
//
//   swiftc -O tests/e2e/tools/edgefit.swift -o target/e2e/edgefit
//   target/e2e/edgefit BEZEL.png LEFT TOP PATH.txt WIDTH OVERLAY.png
//
// BEZEL.png is one of Apple's MacBook Pro bezel images (Apple Design
// Resources): its screen opening, where alpha < 255, is the glass. LEFT and
// TOP place the panel's top-left pixel in it. PATH.txt holds the edge's
// outline as SVG path data in panel pixels (`cargo run -p
// agent-screen-model --example screen_edge`). WIDTH is the visible line's
// width in panel pixels. The tool draws the outline as the app does, a
// stroke twice WIDTH clipped to the outline, and reports:
//
// - gap: for each point on the glass's edge, how far in the line's outer
//   side lies (negative: out over the bezel), from both shapes' 50%
//   coverage contours, row by row and column by column;
// - over: the most of any pixel the line covers beyond the glass;
// - width: the line's width across it, measured along the edge's normal.
//
// Gap and overhang are also given for each top corner, the notch and the
// rest of the outline.
//
// OVERLAY.png gets 8x crops of the top-left and top-right corners, the
// notch, and a bottom corner: the bezel black, the glass grey, the line red.

import CoreGraphics
import Foundation
import ImageIO
import UniformTypeIdentifiers

let args = CommandLine.arguments
guard args.count == 7 || args.count == 8, let left = Int(args[2]), let top = Int(args[3]),
  let width = Double(args[5])
else {
  print("usage: edgefit BEZEL.png LEFT TOP PATH.txt WIDTH OVERLAY.png [centred]")
  exit(2)
}

// The bezel's alpha, row by row from the top.
let source = CGImageSourceCreateWithURL(URL(fileURLWithPath: args[1]) as CFURL, nil)!
let image = CGImageSourceCreateImageAtIndex(source, 0, nil)!
let (iw, ih) = (image.width, image.height)
var rgba = [UInt8](repeating: 0, count: iw * ih * 4)
rgba.withUnsafeMutableBytes { p in
  let c = CGContext(
    data: p.baseAddress, width: iw, height: ih, bitsPerComponent: 8, bytesPerRow: iw * 4,
    space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
  c.draw(image, in: CGRect(x: 0, y: 0, width: iw, height: ih))
}

// The outline, in panel pixels, y down.
let words = try! String(contentsOfFile: args[4], encoding: .utf8)
  .split(whereSeparator: { $0 == " " || $0 == "\n" || $0 == "," }).map(String.init)
let outline = CGMutablePath()
var i = 0
func num() -> CGFloat {
  i += 1
  return CGFloat(Double(words[i - 1])!)
}
while i < words.count {
  let op = words[i]
  i += 1
  switch op {
  case "M": outline.move(to: CGPoint(x: num(), y: num()))
  case "L": outline.addLine(to: CGPoint(x: num(), y: num()))
  case "C":
    let (a, b, p) = (CGPoint(x: num(), y: num()), CGPoint(x: num(), y: num()), CGPoint(x: num(), y: num()))
    outline.addCurve(to: p, control1: a, control2: b)
  case "Z": outline.closeSubpath()
  default: fatalError("unknown path op \(op)")
  }
}
let box = outline.boundingBox
let (pw, ph) = (Int(box.maxX.rounded()), Int(box.maxY.rounded()))

// Coverage of the panel's pixels, 0...1, row by row from the top.
func render(_ draw: (CGContext) -> Void) -> [Double] {
  var g = [UInt8](repeating: 0, count: pw * ph)
  g.withUnsafeMutableBytes { p in
    let c = CGContext(
      data: p.baseAddress, width: pw, height: ph, bitsPerComponent: 8, bytesPerRow: pw,
      space: CGColorSpaceCreateDeviceGray(), bitmapInfo: CGImageAlphaInfo.none.rawValue)!
    c.translateBy(x: 0, y: CGFloat(ph))
    c.scaleBy(x: 1, y: -1)
    c.setFillColor(gray: 1, alpha: 1)
    c.setStrokeColor(gray: 1, alpha: 1)
    draw(c)
  }
  return g.map { Double($0) / 255 }
}
// `centred` strokes the path WIDTH wide on its centre line, unclipped, as
// the edge was drawn before it fitted the glass.
let centred = args.count == 8 && args[7] == "centred"
let line = render { c in
  c.addPath(outline)
  if !centred { c.clip() }
  c.addPath(outline)
  c.setLineWidth(CGFloat(centred ? width : 2 * width))
  c.strokePath()
}
// The line with everything inside it: its outer side is this one's edge.
let fill = zip(render { c in
  c.addPath(outline)
  c.fillPath()
}, line).map { max($0, $1) }
var glass = [Double](repeating: 0, count: pw * ph)
for y in 0..<ph {
  for x in 0..<pw {
    let (bx, by) = (x + left, y + top)
    glass[y * pw + x] = 1 - Double(rgba[(by * iw + bx) * 4 + 3]) / 255
  }
}

func at(_ f: [Double], _ x: Int, _ y: Int) -> Double {
  x < 0 || y < 0 || x >= pw || y >= ph ? 0 : f[y * pw + x]
}
// Bilinear, with pixel centres at +0.5.
func sample(_ f: [Double], _ x: Double, _ y: Double) -> Double {
  let (fx, fy) = (x - 0.5, y - 0.5)
  let (x0, y0) = (Int(floor(fx)), Int(floor(fy)))
  let (tx, ty) = (fx - Double(x0), fy - Double(y0))
  let a = at(f, x0, y0) * (1 - tx) + at(f, x0 + 1, y0) * tx
  let b = at(f, x0, y0 + 1) * (1 - tx) + at(f, x0 + 1, y0 + 1) * tx
  return a * (1 - ty) + b * ty
}
// Where coverage crosses 50%, between neighbouring pixel centres, along
// every row and column: the shape's edge, to a fraction of a pixel.
func contour(_ f: [Double]) -> [(Double, Double)] {
  var out: [(Double, Double)] = []
  for y in 0..<ph {
    for x in -1..<pw {
      let (a, b) = (at(f, x, y), at(f, x + 1, y))
      if (a - 0.5) * (b - 0.5) < 0 { out.append((Double(x) + 0.5 + (0.5 - a) / (b - a), Double(y) + 0.5)) }
    }
  }
  for x in 0..<pw {
    for y in -1..<ph {
      let (a, b) = (at(f, x, y), at(f, x, y + 1))
      if (a - 0.5) * (b - 0.5) < 0 { out.append((Double(x) + 0.5, Double(y) + 0.5 + (0.5 - a) / (b - a))) }
    }
  }
  return out
}
// Nearest points by a coarse grid.
struct Grid {
  var cells: [Int: [(Double, Double)]] = [:]
  init(_ points: [(Double, Double)]) {
    for p in points { cells[Grid.key(p.0, p.1), default: []].append(p) }
  }
  static func key(_ x: Double, _ y: Double) -> Int { Int(floor(x / 8)) * 100_000 + Int(floor(y / 8)) }
  func nearest(_ p: (Double, Double)) -> ((Double, Double), Double)? {
    var best: ((Double, Double), Double)?
    let (cx, cy) = (Int(floor(p.0 / 8)), Int(floor(p.1 / 8)))
    for dx in -1...1 {
      for dy in -1...1 {
        for q in cells[(cx + dx) * 100_000 + cy + dy] ?? [] {
          let d = hypot(q.0 - p.0, q.1 - p.1)
          if best == nil || d < best!.1 { best = (q, d) }
        }
      }
    }
    return best
  }
}

let glassEdge = contour(glass)
let outerEdge = contour(fill)
let outer = Grid(outerEdge)
let glassGrid = Grid(glassEdge)
var worstGap = 0.0, worstGapAt = (0.0, 0.0)
var worstOver = 0.0, worstOverAt = (0.0, 0.0)
var unmatched = 0
// The same by part of the outline: the top 100 px at each corner, the top
// edge between them with the notch, and the rest. One part's worst can
// hide another's.
let parts = ["top-left corner", "notch", "top-right corner", "elsewhere"]
func part(_ p: (Double, Double)) -> Int {
  if p.1 > 100 { return 3 }
  if p.0 < 100 { return 0 }
  if p.0 > Double(pw) - 100 { return 2 }
  return 1
}
var partGap = [Double](repeating: 0, count: 4), partOver = [Double](repeating: 0, count: 4)
var partGapAt = [(Double, Double)](repeating: (0, 0), count: 4), partOverAt = partGapAt
for p in glassEdge {
  guard let (q, d) = outer.nearest(p) else {
    unmatched += 1
    continue
  }
  // Positive when the line's outer side is inside the glass.
  let inside = sample(glass, q.0, q.1) > 0.5
  let signed = inside ? d : -d
  if signed > worstGap { (worstGap, worstGapAt) = (signed, p) }
  if -signed > worstOver { (worstOver, worstOverAt) = (-signed, p) }
  let k = part(p)
  if signed > partGap[k] { (partGap[k], partGapAt[k]) = (signed, p) }
  if -signed > partOver[k] { (partOver[k], partOverAt[k]) = (-signed, p) }
}
for p in outerEdge where glassGrid.nearest(p) == nil { unmatched += 1 }

// The most of a pixel the line covers past the glass.
var over = 0.0, overAt = (0, 0)
for y in 0..<ph {
  for x in 0..<pw where line[y * pw + x] - glass[y * pw + x] > over {
    (over, overAt) = (line[y * pw + x] - glass[y * pw + x], (x, y))
  }
}

// The line's width across it: coverage summed inward along the normal of
// the glass's edge, at every contour point.
var minWidth = Double.infinity, maxWidth = 0.0, minWidthAt = (0.0, 0.0), maxWidthAt = (0.0, 0.0)
// Square corners have no normal: leave out the line's ends there.
let square = [(0.0, Double(ph)), (Double(pw), Double(ph))]
for p in outerEdge where square.allSatisfy({ hypot($0.0 - p.0, $0.1 - p.1) > 4 * width }) {
  let gx = sample(fill, p.0 + 1, p.1) - sample(fill, p.0 - 1, p.1)
  let gy = sample(fill, p.0, p.1 + 1) - sample(fill, p.0, p.1 - 1)
  let n = hypot(gx, gy)
  if n < 0.2 { continue }
  let (nx, ny) = (gx / n, gy / n)
  var sum = 0.0
  let step = 0.05
  var t = -2.0
  while t < 3 * width {
    sum += sample(line, p.0 + nx * t, p.1 + ny * t) * step
    t += step
  }
  if sum < minWidth { (minWidth, minWidthAt) = (sum, p) }
  if sum > maxWidth { (maxWidth, maxWidthAt) = (sum, p) }
}

func f(_ v: Double) -> String { String(format: "%.3f", v) }
func pt(_ p: (Double, Double)) -> String { "(\(f(p.0)), \(f(p.1)))" }
print("glass edge points \(glassEdge.count), line edge points \(outerEdge.count), unmatched \(unmatched)")
print("max gap, line inside the glass's edge: \(f(worstGap)) px at \(pt(worstGapAt))")
print("max overhang, line outside the glass: \(f(worstOver)) px at \(pt(worstOverAt))")
print("most of one pixel covered past the glass: \(f(over)) at (\(overAt.0), \(overAt.1))")
for (i, name) in parts.enumerated() {
  print("  \(name): gap \(f(partGap[i])) px at \(pt(partGapAt[i])), overhang \(f(partOver[i])) px at \(pt(partOverAt[i]))")
}
print("line width across it: \(f(minWidth)) at \(pt(minWidthAt)) to \(f(maxWidth)) at \(pt(maxWidthAt)) px")

// The overlay: crops at 8x.
let zoom = 8
let crops: [(Int, Int, Int, Int)] = {
  let notch = outerEdge.filter { $0.1 > 20 && $0.1 < 120 && $0.0 > 200 && $0.0 < Double(pw - 200) }
  let nx = notch.isEmpty ? pw / 2 : Int(notch.map { $0.0 }.min()!)
  let nr = notch.isEmpty ? pw / 2 : Int(notch.map { $0.0 }.max()!)
  return [
    (0, 0, 96, 96), (nx - 40, 0, 96, 96), (nr - 56, 0, 96, 96), (pw - 96, 0, 96, 96), (0, ph - 48, 96, 48),
  ]
}()
let ow = crops.reduce(0) { $0 + $1.2 * zoom + 16 }
let oh = 96 * zoom
var out = [UInt8](repeating: 255, count: ow * oh * 4)
var ox = 0
for (cx, cy, cw, chh) in crops {
  for y in 0..<chh * zoom {
    for x in 0..<cw * zoom {
      let (px, py) = (cx + x / zoom, cy + y / zoom)
      let g = at(glass, px, py)
      let l = at(line, px, py)
      // Bezel black, glass grey, the line red over either.
      var (r, gg, b) = (0.35 * g, 0.35 * g, 0.38 * g)
      (r, gg, b) = (r * (1 - l) + 1.0 * l, gg * (1 - l) + 0.23 * l, b * (1 - l) + 0.19 * l)
      let o = (y * ow + ox + x) * 4
      out[o] = UInt8(r * 255)
      out[o + 1] = UInt8(gg * 255)
      out[o + 2] = UInt8(b * 255)
      out[o + 3] = 255
    }
  }
  // Glass edge contour dots, cyan.
  for p in glassEdge where p.0 >= Double(cx) && p.0 < Double(cx + cw) && p.1 >= Double(cy) && p.1 < Double(cy + chh) {
    let (x, y) = (Int((p.0 - Double(cx)) * Double(zoom)), Int((p.1 - Double(cy)) * Double(zoom)))
    if x < 0 || y < 0 || x >= cw * zoom || y >= chh * zoom { continue }
    let o = (y * ow + ox + x) * 4
    (out[o], out[o + 1], out[o + 2]) = (0, 230, 255)
  }
  ox += cw * zoom + 16
}
let provider = CGDataProvider(data: Data(out) as CFData)!
let picture = CGImage(
  width: ow, height: oh, bitsPerComponent: 8, bitsPerPixel: 32, bytesPerRow: ow * 4,
  space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.noneSkipLast.rawValue),
  provider: provider, decode: nil, shouldInterpolate: false, intent: .defaultIntent)!
let dest = CGImageDestinationCreateWithURL(URL(fileURLWithPath: args[6]) as CFURL, UTType.png.identifier as CFString, 1, nil)!
CGImageDestinationAddImage(dest, picture, nil)
CGImageDestinationFinalize(dest)
print("overlay: \(args[6])")
