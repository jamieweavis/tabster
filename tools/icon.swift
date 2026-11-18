// Draws the Tabster app icon (a four-string tab staff on paper with a red playhead) as a 1024 px PNG.
// `just icon` runs this and packs the result into Resources/Bundle/AppIcon.icns and .github/icon.png.
//
// usage: swift tools/icon.swift <out.png>

import AppKit
import CoreText

guard CommandLine.arguments.count == 2 else {
    FileHandle.standardError.write(Data("usage: swift tools/icon.swift <out.png>\n".utf8))
    exit(2)
}

let size = 1024
let srgb = CGColorSpace(name: CGColorSpace.sRGB)!
guard let ctx = CGContext(
    data: nil, width: size, height: size, bitsPerComponent: 8, bytesPerRow: 0, space: srgb,
    bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
) else { fatalError("no bitmap context") }

// Work in top-left-origin coordinates on Apple's 1024 grid: an 824 px plate inset by 100 px.
ctx.translateBy(x: 0, y: CGFloat(size))
ctx.scaleBy(x: 1, y: -1)

func color(_ hex: UInt32, _ alpha: CGFloat = 1) -> CGColor {
    CGColor(
        colorSpace: srgb,
        components: [
            CGFloat((hex >> 16) & 0xFF) / 255, CGFloat((hex >> 8) & 0xFF) / 255, CGFloat(hex & 0xFF) / 255, alpha,
        ])!
}

func gradient(_ stops: [(CGFloat, CGColor)]) -> CGGradient {
    CGGradient(colorsSpace: srgb, colors: stops.map(\.1) as CFArray, locations: stops.map(\.0))!
}

let plate = CGRect(x: 100, y: 100, width: 824, height: 824)
let platePath = CGPath(roundedRect: plate, cornerWidth: 186, cornerHeight: 186, transform: nil)
let paper = gradient([(0, color(0xFEFCF6)), (1, color(0xEAE3D2))])
let ink = color(0x2A2A30)
let red = color(0xE8452C)

func fillPaper() {
    ctx.drawLinearGradient(paper, start: CGPoint(x: 0, y: plate.minY), end: CGPoint(x: 0, y: plate.maxY), options: [])
}

// Plate, with its drop shadow.
ctx.saveGState()
ctx.setShadow(offset: CGSize(width: 0, height: -14), blur: 32, color: color(0x000000, 0.28))
ctx.addPath(platePath)
ctx.setFillColor(color(0xEAE3D2))
ctx.fillPath()
ctx.restoreGState()

ctx.saveGState()
ctx.addPath(platePath)
ctx.clip()
fillPaper()

// The four strings.
let strings: [CGFloat] = [302, 442, 582, 722]
ctx.setStrokeColor(ink)
ctx.setLineWidth(12)
for y in strings {
    ctx.move(to: CGPoint(x: plate.minX, y: y))
    ctx.addLine(to: CGPoint(x: plate.maxX, y: y))
}
ctx.strokePath()

// Fret numbers: (digit, x, string index from the top).
let notes: [(String, CGFloat, Int)] = [("0", 240, 3), ("2", 410, 1), ("0", 610, 2), ("2", 790, 0)]

// Break the string behind each number with a patch of paper.
for (_, x, string) in notes {
    let gap = CGRect(x: x - 62, y: strings[string] - 64, width: 124, height: 128)
    ctx.saveGState()
    ctx.addPath(CGPath(roundedRect: gap, cornerWidth: 32, cornerHeight: 32, transform: nil))
    ctx.clip()
    fillPaper()
    ctx.restoreGState()
}

// A faint red wash trailing the playhead over what has been played.
let playhead: CGFloat = 510
ctx.saveGState()
ctx.clip(to: CGRect(x: playhead - 300, y: plate.minY, width: 300, height: plate.height))
ctx.drawLinearGradient(
    gradient([(0, color(0xE8452C, 0)), (1, color(0xE8452C, 0.16))]),
    start: CGPoint(x: playhead - 300, y: 0), end: CGPoint(x: playhead, y: 0), options: [])
ctx.restoreGState()

// The numbers, as glyph outlines centred on their string.
let rounded = NSFont.systemFont(ofSize: 160, weight: .heavy).fontDescriptor.withDesign(.rounded)!
let font = NSFont(descriptor: rounded, size: 160)! as CTFont
ctx.setFillColor(color(0x111114))
for (digit, x, string) in notes {
    var chars = Array(digit.utf16)
    var glyphs = [CGGlyph](repeating: 0, count: chars.count)
    guard CTFontGetGlyphsForCharacters(font, &chars, &glyphs, chars.count),
        let outline = CTFontCreatePathForGlyph(font, glyphs[0], nil)
    else { fatalError("no glyph for \(digit)") }
    let box = outline.boundingBoxOfPath
    // Glyph outlines are y-up; flip them and centre the digit's box on (x, string).
    var place = CGAffineTransform(translationX: x - box.midX, y: strings[string] + box.midY).scaledBy(x: 1, y: -1)
    ctx.addPath(outline.copy(using: &place)!)
    ctx.fillPath()
}

// The playhead: a full-height line with a pointed marker at the top.
ctx.setStrokeColor(red)
ctx.setLineWidth(14)
ctx.move(to: CGPoint(x: playhead, y: plate.minY))
ctx.addLine(to: CGPoint(x: playhead, y: plate.maxY))
ctx.strokePath()
ctx.setFillColor(red)
ctx.move(to: CGPoint(x: playhead - 36, y: 100))
ctx.addLine(to: CGPoint(x: playhead + 36, y: 100))
ctx.addLine(to: CGPoint(x: playhead + 36, y: 150))
ctx.addLine(to: CGPoint(x: playhead, y: 190))
ctx.addLine(to: CGPoint(x: playhead - 36, y: 150))
ctx.closePath()
ctx.fillPath()
ctx.restoreGState()

// A lit top edge and a shaded bottom edge on the plate.
let rim = CGPath(roundedRect: plate.insetBy(dx: 3, dy: 3), cornerWidth: 183, cornerHeight: 183, transform: nil)
ctx.saveGState()
ctx.addPath(rim.copy(strokingWithWidth: 6, lineCap: .butt, lineJoin: .miter, miterLimit: 10))
ctx.clip()
ctx.drawLinearGradient(
    gradient([(0, color(0xFFFFFF, 0.9)), (0.4, color(0xFFFFFF, 0)), (1, color(0x000000, 0.14))]),
    start: CGPoint(x: 0, y: plate.minY), end: CGPoint(x: 0, y: plate.maxY), options: [])
ctx.restoreGState()

guard let image = ctx.makeImage(),
    let png = NSBitmapImageRep(cgImage: image).representation(using: .png, properties: [:])
else { fatalError("could not encode the icon") }
try png.write(to: URL(fileURLWithPath: CommandLine.arguments[1]))
