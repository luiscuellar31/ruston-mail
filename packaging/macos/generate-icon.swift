#!/usr/bin/env swift

import AppKit
import CoreGraphics
import Foundation
import ImageIO
import UniformTypeIdentifiers

// Draws Ruston Mail's 1024-pixel icon. Every shape derives from a few
// measures below, so the icon is mirror-symmetric by construction, and the
// image is drawn and saved in sRGB, so each color comes out as written.

// MARK: - Configuration

let canvasSize = 1024

let outputPath: String = {
    let arguments = CommandLine.arguments
    if arguments.count > 1 {
        return NSString(string: arguments[1]).expandingTildeInPath
    }
    return "assets/macos/ruston-mail-1024.png"
}()

// MARK: - Palette

extension NSColor {
    /// An sRGB color from `0xRRGGBB`.
    convenience init(hex: UInt32, alpha: CGFloat = 1.0) {
        self.init(
            srgbRed: CGFloat((hex >> 16) & 0xFF) / 255.0,
            green: CGFloat((hex >> 8) & 0xFF) / 255.0,
            blue: CGFloat(hex & 0xFF) / 255.0,
            alpha: alpha
        )
    }
}

// Every color the icon uses. The light comes from above: each piece is
// lighter at its top, and the higher a piece sits, the lighter it is.
let backgroundTop = NSColor(hex: 0x3C247A)
let backgroundBottom = NSColor(hex: 0x241848)
/// The flap's shadow, and the envelope's base under its faces.
let deepIndigo = NSColor(hex: 0x341994)
let deepViolet = NSColor(hex: 0x4824BF)
/// The lower fold, a shade darker than the side faces.
let foldViolet = NSColor(hex: 0x3D21A6)
let lightFoldViolet = NSColor(hex: 0x5434D2)
/// The side faces: Proton's violet, lightening upward.
let electricViolet = NSColor(hex: 0x6D4AFF)
let mediumViolet = NSColor(hex: 0x8566FF)
/// The flap.
let lavender = NSColor(hex: 0xC4B7FF)
let lightLavender = NSColor(hex: 0xD4CAFF)
let highlight = NSColor(hex: 0xFFFFFF)
let shadow = NSColor(hex: 0x000000)

// MARK: - Geometry

/// Apple's macOS icon grid: an 824-pixel shape centered on the canvas.
let iconRect = CGRect(x: 100, y: 100, width: 824, height: 824)
let iconCornerRadius: CGFloat = 185.4

let envelopeSize = CGSize(width: 648, height: 454)
let envelopeRect = CGRect(
    x: iconRect.midX - envelopeSize.width / 2,
    y: iconRect.midY - envelopeSize.height / 2,
    width: envelopeSize.width,
    height: envelopeSize.height
)
let envelopeCornerRadius: CGFloat = 72

/// Where the flap's and the lower fold's sloping edges start, measured in
/// from the envelope's corners.
let foldInset = CGSize(width: 8, height: 5)
/// How far below the envelope's middle the flap's tip reaches. The lower
/// fold rises as far above it, so its peak stays under the flap.
let flapDepth: CGFloat = 48

let flapTip = CGPoint(x: envelopeRect.midX, y: envelopeRect.midY - flapDepth)
let flapLeftStart = CGPoint(
    x: envelopeRect.minX + foldInset.width,
    y: envelopeRect.maxY - foldInset.height
)
let flapRightStart = CGPoint(
    x: envelopeRect.maxX - foldInset.width,
    y: flapLeftStart.y
)

let foldPeak = CGPoint(x: envelopeRect.midX, y: envelopeRect.midY + flapDepth)
let foldLeftEnd = CGPoint(
    x: flapLeftStart.x,
    y: envelopeRect.minY + foldInset.height
)
let foldRightEnd = CGPoint(x: flapRightStart.x, y: foldLeftEnd.y)

/// Where the flap's edge from `start` crosses the lower fold's edge: on the
/// envelope's middle line, since the fold mirrors the flap.
func flapCrossing(from start: CGPoint) -> CGPoint {
    let t = (start.y - envelopeRect.midY) / (start.y - flapTip.y)
    return CGPoint(x: start.x + t * (flapTip.x - start.x), y: envelopeRect.midY)
}

/// How tall the flap's shadow is where it reaches the envelope's sides.
let flapShadowHeight: CGFloat = 64

// MARK: - Drawing Helpers

func makeGradient(_ colors: [NSColor]) -> NSGradient {
    guard let gradient = NSGradient(colors: colors, atLocations: nil, colorSpace: .sRGB) else {
        fatalError("Could not create NSGradient")
    }
    return gradient
}

/// Fills `path` with a gradient from `bottom` to `top`.
func fillVertically(_ path: NSBezierPath, from bottom: NSColor, to top: NSColor) {
    makeGradient([bottom, top]).draw(in: path, angle: 90)
}

func polygon(_ points: [CGPoint]) -> NSBezierPath {
    let path = NSBezierPath()
    path.move(to: points[0])
    for point in points.dropFirst() {
        path.line(to: point)
    }
    path.close()
    return path
}

/// A rounded rectangle whose corners ease into their curve, as Apple's icon
/// shapes do, instead of the sudden change of a circular arc. Each corner
/// spans 1.528 radii of its edges; the control points are the usual cubic
/// fit for that curve.
func continuousRoundedRect(_ rect: CGRect, radius: CGFloat) -> NSBezierPath {
    let span: CGFloat = 1.52866483
    let r = min(radius, min(rect.width, rect.height) / (2 * span))
    let path = NSBezierPath()

    // Arrives at `corner` heading along `incoming` and leaves along `outgoing`.
    func corner(_ corner: CGPoint, _ incoming: CGVector, _ outgoing: CGVector) {
        func point(_ back: CGFloat, _ forward: CGFloat) -> CGPoint {
            CGPoint(
                x: corner.x - back * r * incoming.dx + forward * r * outgoing.dx,
                y: corner.y - back * r * incoming.dy + forward * r * outgoing.dy
            )
        }
        path.line(to: point(span, 0))
        path.curve(
            to: point(0.63149399, 0.07491100),
            controlPoint1: point(1.08849299, 0),
            controlPoint2: point(0.86840701, 0)
        )
        path.curve(
            to: point(0.07491100, 0.63149399),
            controlPoint1: point(0.37282401, 0.16905001),
            controlPoint2: point(0.16905001, 0.37282401)
        )
        path.curve(
            to: point(0, span),
            controlPoint1: point(0, 0.86840701),
            controlPoint2: point(0, 1.08849299)
        )
    }

    path.move(to: CGPoint(x: rect.minX + span * r, y: rect.minY))
    corner(CGPoint(x: rect.maxX, y: rect.minY), CGVector(dx: 1, dy: 0), CGVector(dx: 0, dy: 1))
    corner(CGPoint(x: rect.maxX, y: rect.maxY), CGVector(dx: 0, dy: 1), CGVector(dx: -1, dy: 0))
    corner(CGPoint(x: rect.minX, y: rect.maxY), CGVector(dx: -1, dy: 0), CGVector(dx: 0, dy: -1))
    corner(CGPoint(x: rect.minX, y: rect.minY), CGVector(dx: 0, dy: -1), CGVector(dx: 1, dy: 0))
    path.close()
    return path
}

/// Runs `draw` with a drop shadow below it.
func withShadow(color: NSColor, blur: CGFloat, drop: CGFloat, _ draw: () -> Void) {
    NSGraphicsContext.saveGraphicsState()
    let castShadow = NSShadow()
    castShadow.shadowColor = color
    castShadow.shadowBlurRadius = blur
    castShadow.shadowOffset = NSSize(width: 0, height: -drop)
    castShadow.set()
    draw()
    NSGraphicsContext.restoreGraphicsState()
}

/// A light line just inside the edge of `shape`.
func innerRim(_ shape: NSBezierPath, color: NSColor, width: CGFloat) {
    NSGraphicsContext.saveGraphicsState()
    shape.addClip()
    let rim = shape.copy() as! NSBezierPath
    // Half of a stroke lies outside the shape, where the clip drops it.
    rim.lineWidth = width * 2
    color.setStroke()
    rim.stroke()
    NSGraphicsContext.restoreGraphicsState()
}

// MARK: - Canvas

guard
    let sRGB = CGColorSpace(name: CGColorSpace.sRGB),
    let canvas = CGContext(
        data: nil,
        width: canvasSize,
        height: canvasSize,
        bitsPerComponent: 8,
        bytesPerRow: 0,
        space: sRGB,
        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
    )
else {
    fatalError("Could not create the sRGB canvas")
}

canvas.setShouldAntialias(true)
canvas.interpolationQuality = .high

NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = NSGraphicsContext(cgContext: canvas, flipped: false)

// MARK: - Icon Shape

let iconShape = continuousRoundedRect(iconRect, radius: iconCornerRadius)

withShadow(color: shadow.withAlphaComponent(0.34), blur: 46, drop: 24) {
    shadow.setFill()
    iconShape.fill()
}

fillVertically(iconShape, from: backgroundBottom, to: backgroundTop)

// A faint light over the top of the shape.
NSGraphicsContext.saveGraphicsState()
iconShape.addClip()
let topLightHeight: CGFloat = 309
fillVertically(
    NSBezierPath(rect: CGRect(
        x: iconRect.minX,
        y: iconRect.maxY - topLightHeight,
        width: iconRect.width,
        height: topLightHeight
    )),
    from: highlight.withAlphaComponent(0),
    to: highlight.withAlphaComponent(0.045)
)
NSGraphicsContext.restoreGraphicsState()

innerRim(iconShape, color: highlight.withAlphaComponent(0.105), width: 3)

// MARK: - Envelope

let envelopeShape = continuousRoundedRect(envelopeRect, radius: envelopeCornerRadius)

withShadow(color: shadow.withAlphaComponent(0.27), blur: 26, drop: 14) {
    deepViolet.setFill()
    envelopeShape.fill()
}

NSGraphicsContext.saveGraphicsState()
envelopeShape.addClip()

// Side faces: the two halves of the envelope. The flap and the lower fold
// cover the middle, so only each side's triangle shows.
let (leftHalf, rightHalf) = envelopeRect.divided(
    atDistance: envelopeRect.width / 2,
    from: .minXEdge
)
for half in [leftHalf, rightHalf] {
    fillVertically(NSBezierPath(rect: half), from: electricViolet, to: mediumViolet)
}

let lowerFold = polygon([
    CGPoint(x: envelopeRect.minX, y: envelopeRect.minY),
    CGPoint(x: envelopeRect.maxX, y: envelopeRect.minY),
    foldRightEnd,
    foldPeak,
    foldLeftEnd,
])
fillVertically(lowerFold, from: foldViolet, to: lightFoldViolet)

let flap = polygon([
    flapLeftStart,
    flapTip,
    flapRightStart,
    CGPoint(x: envelopeRect.maxX, y: envelopeRect.maxY),
    CGPoint(x: envelopeRect.minX, y: envelopeRect.maxY),
])
// The flap's shadow: a flat wedge under each of its edges, tallest at the
// envelope's side and gone where the flap meets the lower fold.
deepIndigo.withAlphaComponent(0.26).setFill()
for (start, side) in [(flapLeftStart, envelopeRect.minX), (flapRightStart, envelopeRect.maxX)] {
    polygon([
        CGPoint(x: side, y: start.y - flapShadowHeight),
        start,
        flapCrossing(from: start),
    ]).fill()
}
fillVertically(flap, from: lavender, to: lightLavender)

NSGraphicsContext.restoreGraphicsState()

innerRim(envelopeShape, color: highlight.withAlphaComponent(0.085), width: 2)

NSGraphicsContext.restoreGraphicsState()

// MARK: - PNG Export

guard let image = canvas.makeImage() else {
    fatalError("Could not finish the image")
}

let outputURL = URL(fileURLWithPath: outputPath)
do {
    try FileManager.default.createDirectory(
        at: outputURL.deletingLastPathComponent(),
        withIntermediateDirectories: true
    )
} catch {
    fputs("Error creating \(outputURL.deletingLastPathComponent().path): \(error)\n", stderr)
    exit(EXIT_FAILURE)
}

// The PNG carries the sRGB profile it was drawn in.
guard
    let destination = CGImageDestinationCreateWithURL(
        outputURL as CFURL,
        UTType.png.identifier as CFString,
        1,
        nil
    )
else {
    fputs("Error writing PNG: cannot create \(outputURL.path)\n", stderr)
    exit(EXIT_FAILURE)
}
CGImageDestinationAddImage(destination, image, nil)
guard CGImageDestinationFinalize(destination) else {
    fputs("Error writing PNG: \(outputURL.path)\n", stderr)
    exit(EXIT_FAILURE)
}

print("Ruston Mail icon written to \(outputURL.path)")
