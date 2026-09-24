#!/usr/bin/env swift

import Cocoa
import AppKit
import CoreGraphics
import Foundation

// MARK: - Configuration

let canvasSize = NSSize(width: 1024, height: 1024)

let arguments = CommandLine.arguments
let outputPath: String = {
    if arguments.count > 1 {
        return NSString(string: arguments[1]).expandingTildeInPath
    }

    return "assets/macos/ruston-mail-1024.png"
}()

// MARK: - Color Helpers

extension NSColor {
    convenience init(hex: UInt32, alpha: CGFloat = 1.0) {
        let red = CGFloat((hex >> 16) & 0xFF) / 255.0
        let green = CGFloat((hex >> 8) & 0xFF) / 255.0
        let blue = CGFloat(hex & 0xFF) / 255.0

        self.init(
            calibratedRed: red,
            green: green,
            blue: blue,
            alpha: alpha
        )
    }
}

// ============================================================
// MARK: - Palette
// ============================================================

let electricViolet = NSColor(hex: 0x6D4AFF)

let lavender = NSColor(hex: 0xC4B7FF)
let lightLavender = NSColor(hex: 0xD4CAFF)

let mediumViolet = NSColor(hex: 0x8566FF)

let deepViolet = NSColor(hex: 0x4824BF)
let deepIndigo = NSColor(hex: 0x341994)

let backgroundTop = NSColor(hex: 0x3C247A)
let backgroundBottom = NSColor(hex: 0x241848)

// ============================================================
// MARK: - Base Geometry
// ============================================================

let envelopeMinX: CGFloat = 188
let envelopeMinY: CGFloat = 287
let envelopeWidth: CGFloat = 648
let envelopeHeight: CGFloat = 454

let envelopeMaxX: CGFloat = envelopeMinX + envelopeWidth
let envelopeMaxY: CGFloat = envelopeMinY + envelopeHeight

let envelopeMidX: CGFloat = envelopeMinX + (envelopeWidth / 2.0)
let envelopeMidY: CGFloat = envelopeMinY + (envelopeHeight / 2.0)

// ============================================================
// MARK: - Symmetric Fold Geometry
// ============================================================
//
// Aquí la idea es que TODO quede matemáticamente simétrico:
//
// - La solapa superior arranca 42 px hacia adentro y 5 px
//   hacia abajo desde la parte superior.
// - El pliegue inferior termina exactamente con el MISMO inset:
//   42 px hacia adentro y 5 px hacia arriba desde la parte inferior.
// - El pico inferior oculto es el espejo vertical del superior
//   respecto al centro del sobre.
//

let foldSideInsetX: CGFloat = 42
let foldEdgeInsetY: CGFloat = 5

// Upper flap start points (perfectly mirrored)
let upperLeftFoldStart = NSPoint(
    x: envelopeMinX + foldSideInsetX,
    y: envelopeMaxY - foldEdgeInsetY
)

let upperRightFoldStart = NSPoint(
    x: envelopeMaxX - foldSideInsetX,
    y: envelopeMaxY - foldEdgeInsetY
)

// Upper tip
let upperTip = NSPoint(
    x: envelopeMidX,
    y: 492
)

// Mirror the upper tip vertically around the center of the envelope.
// This guarantees true vertical symmetry of the fold geometry.
let lowerHiddenPeakY = (2.0 * envelopeMidY) - upperTip.y

let lowerHiddenPeak = NSPoint(
    x: envelopeMidX,
    y: lowerHiddenPeakY
)

// Lower fold endpoints (same inset as the upper fold, but mirrored)
let lowerLeftFoldEnd = NSPoint(
    x: envelopeMinX + foldSideInsetX,
    y: envelopeMinY + foldEdgeInsetY
)

let lowerRightFoldEnd = NSPoint(
    x: envelopeMaxX - foldSideInsetX,
    y: envelopeMinY + foldEdgeInsetY
)

// ============================================================
// MARK: - Drawing Helpers
// ============================================================

func makeGradient(
    colors: [NSColor],
    locations: [CGFloat]
) -> NSGradient {
    precondition(colors.count == locations.count)

    return locations.withUnsafeBufferPointer { buffer in
        guard let gradient = NSGradient(
            colors: colors,
            atLocations: buffer.baseAddress,
            colorSpace: .deviceRGB
        ) else {
            fatalError("Could not create NSGradient")
        }

        return gradient
    }
}

func polygon(_ points: [NSPoint]) -> NSBezierPath {
    let path = NSBezierPath()

    guard let first = points.first else {
        return path
    }

    path.move(to: first)

    for point in points.dropFirst() {
        path.line(to: point)
    }

    path.close()

    return path
}

func setShadow(
    color: NSColor,
    blur: CGFloat,
    offsetX: CGFloat = 0,
    offsetY: CGFloat
) {
    let shadow = NSShadow()

    shadow.shadowColor = color
    shadow.shadowBlurRadius = blur
    shadow.shadowOffset = NSSize(
        width: offsetX,
        height: offsetY
    )

    shadow.set()
}

func fillPath(
    _ path: NSBezierPath,
    color: NSColor,
    shadowColor: NSColor? = nil,
    shadowBlur: CGFloat = 0,
    shadowOffset: NSSize = .zero
) {
    NSGraphicsContext.saveGraphicsState()

    if let shadowColor {
        setShadow(
            color: shadowColor,
            blur: shadowBlur,
            offsetX: shadowOffset.width,
            offsetY: shadowOffset.height
        )
    }

    color.setFill()
    path.fill()

    NSGraphicsContext.restoreGraphicsState()
}

func strokePath(
    _ path: NSBezierPath,
    color: NSColor,
    width: CGFloat
) {
    NSGraphicsContext.saveGraphicsState()

    path.lineWidth = width
    color.setStroke()
    path.stroke()

    NSGraphicsContext.restoreGraphicsState()
}

// MARK: - Bitmap

guard let bitmap = NSBitmapImageRep(
    bitmapDataPlanes: nil,
    pixelsWide: Int(canvasSize.width),
    pixelsHigh: Int(canvasSize.height),
    bitsPerSample: 8,
    samplesPerPixel: 4,
    hasAlpha: true,
    isPlanar: false,
    colorSpaceName: .deviceRGB,
    bytesPerRow: 0,
    bitsPerPixel: 0
) else {
    fatalError("Could not create NSBitmapImageRep")
}

bitmap.size = canvasSize

guard let graphicsContext = NSGraphicsContext(bitmapImageRep: bitmap) else {
    fatalError("Could not create NSGraphicsContext")
}

NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = graphicsContext

guard let cgContext = NSGraphicsContext.current?.cgContext else {
    fatalError("Could not access CGContext")
}

// MARK: - Rendering Quality

cgContext.setShouldAntialias(true)
cgContext.setAllowsAntialiasing(true)
cgContext.interpolationQuality = .high

cgContext.clear(
    CGRect(
        origin: .zero,
        size: CGSize(
            width: canvasSize.width,
            height: canvasSize.height
        )
    )
)

// ============================================================
// MARK: - Main macOS Squircle
// ============================================================

let squircleRect = NSRect(
    x: 100,
    y: 100,
    width: 824,
    height: 824
)

let squircleRadius: CGFloat = 185

let squircle = NSBezierPath(
    roundedRect: squircleRect,
    xRadius: squircleRadius,
    yRadius: squircleRadius
)

// MARK: Outer Shadow

NSGraphicsContext.saveGraphicsState()

setShadow(
    color: NSColor.black.withAlphaComponent(0.34),
    blur: 46,
    offsetX: 0,
    offsetY: -24
)

NSColor.black.setFill()
squircle.fill()

NSGraphicsContext.restoreGraphicsState()

// MARK: Background

let backgroundGradient = makeGradient(
    colors: [
        backgroundBottom,
        backgroundTop
    ],
    locations: [
        0.0,
        1.0
    ]
)

backgroundGradient.draw(
    in: squircle,
    angle: 90
)

// MARK: Subtle Top Illumination

NSGraphicsContext.saveGraphicsState()

squircle.addClip()

let topLightRect = NSRect(
    x: 100,
    y: 615,
    width: 824,
    height: 309
)

let topLightPath = NSBezierPath(rect: topLightRect)

let topLightGradient = makeGradient(
    colors: [
        NSColor.clear,
        NSColor.white.withAlphaComponent(0.045)
    ],
    locations: [
        0.0,
        1.0
    ]
)

topLightGradient.draw(
    in: topLightPath,
    angle: 90
)

NSGraphicsContext.restoreGraphicsState()

// MARK: Squircle Rim Light

let rim = NSBezierPath(
    roundedRect: NSInsetRect(squircleRect, 2, 2),
    xRadius: squircleRadius - 2,
    yRadius: squircleRadius - 2
)

strokePath(
    rim,
    color: NSColor.white.withAlphaComponent(0.105),
    width: 2.5
)

// ============================================================
// MARK: - Envelope
// ============================================================

let envelopeRect = NSRect(
    x: envelopeMinX,
    y: envelopeMinY,
    width: envelopeWidth,
    height: envelopeHeight
)

let envelopeRadius: CGFloat = 72

let envelopeClip = NSBezierPath(
    roundedRect: envelopeRect,
    xRadius: envelopeRadius,
    yRadius: envelopeRadius
)

// MARK: Envelope Shadow

fillPath(
    envelopeClip,
    color: NSColor(hex: 0x6749E5),
    shadowColor: NSColor.black.withAlphaComponent(0.27),
    shadowBlur: 26,
    shadowOffset: NSSize(
        width: 0,
        height: -14
    )
)

// MARK: Envelope Base

let envelopeBaseGradient = makeGradient(
    colors: [
        NSColor(hex: 0x5032C4),
        NSColor(hex: 0x7558F4)
    ],
    locations: [
        0.0,
        1.0
    ]
)

envelopeBaseGradient.draw(
    in: envelopeClip,
    angle: 90
)

// Clip all envelope facets to its rounded silhouette.

NSGraphicsContext.saveGraphicsState()
envelopeClip.addClip()

// ============================================================
// MARK: Left Facet
// ============================================================

let leftFacet = polygon([
    NSPoint(x: envelopeMinX, y: 700),
    NSPoint(x: envelopeMinX, y: envelopeMinY),
    NSPoint(x: 508, y: envelopeMinY),
    NSPoint(x: envelopeMidX, y: 495)
])

let leftGradient = makeGradient(
    colors: [
        NSColor(hex: 0x5A3BE0),
        NSColor(hex: 0x7960FF)
    ],
    locations: [
        0.0,
        1.0
    ]
)

leftGradient.draw(
    in: leftFacet,
    angle: 30
)

// ============================================================
// MARK: Right Facet
// ============================================================

let rightFacet = polygon([
    NSPoint(x: envelopeMaxX, y: 707),
    NSPoint(x: envelopeMaxX, y: envelopeMinY),
    NSPoint(x: 508, y: envelopeMinY),
    NSPoint(x: envelopeMidX, y: 495)
])

let rightGradient = makeGradient(
    colors: [
        NSColor(hex: 0x8067F7),
        NSColor(hex: 0xA08CFF)
    ],
    locations: [
        0.0,
        1.0
    ]
)

rightGradient.draw(
    in: rightFacet,
    angle: 150
)

// ============================================================
// MARK: Bottom Fold
// ============================================================
//
// Este pliegue ahora es perfectamente simétrico:
//
// arriba:
//   (230, 736) -> (512, 492) -> (794, 736)
//
// abajo:
//   (230, 292) -> (512, 536) -> (794, 292)
//
// porque:
// - mismo X
// - mismo inset lateral
// - mismo inset vertical
// - y el pico inferior es espejo del superior
//

let lowerFold = NSBezierPath()

lowerFold.move(
    to: NSPoint(
        x: envelopeMinX,
        y: envelopeMinY
    )
)

lowerFold.line(
    to: NSPoint(
        x: envelopeMaxX,
        y: envelopeMinY
    )
)

lowerFold.line(
    to: lowerRightFoldEnd
)

lowerFold.line(
    to: lowerHiddenPeak
)

lowerFold.line(
    to: lowerLeftFoldEnd
)

lowerFold.close()

let lowerFoldGradient = makeGradient(
    colors: [
        deepIndigo,
        NSColor(hex: 0x5435C6)
    ],
    locations: [
        0.0,
        1.0
    ]
)

lowerFoldGradient.draw(
    in: lowerFold,
    angle: 90
)

// ============================================================
// MARK: Symmetric Shadow Under Upper Flap
// ============================================================
//
// En vez de aplicar shadow al relleno completo de la solapa,
// dibujamos una sombra sobre la línea en "V".
// Eso garantiza que visualmente quede igual en ambos lados.
//

let flapShadowGuide = NSBezierPath()

flapShadowGuide.move(
    to: upperLeftFoldStart
)

flapShadowGuide.line(
    to: upperTip
)

flapShadowGuide.line(
    to: upperRightFoldStart
)

flapShadowGuide.lineJoinStyle = .round
flapShadowGuide.lineCapStyle = .round
flapShadowGuide.lineWidth = 8.0

NSGraphicsContext.saveGraphicsState()

setShadow(
    color: deepIndigo.withAlphaComponent(0.18),
    blur: 10,
    offsetX: 0,
    offsetY: -4
)

NSColor(
    hex: 0x5A39DC,
    alpha: 0.14
).setStroke()

flapShadowGuide.stroke()

NSGraphicsContext.restoreGraphicsState()

// ============================================================
// MARK: Upper Flap
// ============================================================

let upperFlap = NSBezierPath()

upperFlap.move(
    to: upperLeftFoldStart
)

upperFlap.line(
    to: upperTip
)

upperFlap.line(
    to: upperRightFoldStart
)

upperFlap.line(
    to: NSPoint(
        x: envelopeMaxX,
        y: envelopeMaxY
    )
)

upperFlap.line(
    to: NSPoint(
        x: envelopeMinX,
        y: envelopeMaxY
    )
)

upperFlap.close()

let flapGradient = makeGradient(
    colors: [
        lavender,
        lightLavender
    ],
    locations: [
        0.0,
        1.0
    ]
)

// Vertical gradient only, to avoid left/right bias.
flapGradient.draw(
    in: upperFlap,
    angle: 90
)

// End envelope clipping.

NSGraphicsContext.restoreGraphicsState()

// ============================================================
// MARK: Envelope Outer Rim
// ============================================================

strokePath(
    envelopeClip,
    color: NSColor.white.withAlphaComponent(0.085),
    width: 2.0
)

// ============================================================
// MARK: Finish Rendering
// ============================================================

graphicsContext.flushGraphics()

NSGraphicsContext.restoreGraphicsState()

// ============================================================
// MARK: PNG Export
// ============================================================

guard let pngData = bitmap.representation(
    using: .png,
    properties: [:]
) else {
    fatalError("Could not generate PNG data")
}

let outputURL = URL(fileURLWithPath: outputPath)
let outputDirectory = outputURL.deletingLastPathComponent()

do {
    if !outputDirectory.path.isEmpty,
       outputDirectory.path != "." {

        try FileManager.default.createDirectory(
            at: outputDirectory,
            withIntermediateDirectories: true,
            attributes: nil
        )
    }

    try pngData.write(
        to: outputURL,
        options: .atomic
    )

    print("Ruston Mail icon generated successfully:")
    print(outputURL.path)
    print("")
    print("Symmetry check:")
    print("- Upper left start:  \(upperLeftFoldStart)")
    print("- Upper right start: \(upperRightFoldStart)")
    print("- Lower left end:    \(lowerLeftFoldEnd)")
    print("- Lower right end:   \(lowerRightFoldEnd)")
    print("- Upper tip:         \(upperTip)")
    print("- Lower hidden peak: \(lowerHiddenPeak)")

} catch {
    fputs(
        "Error writing PNG: \(error)\n",
        stderr
    )

    exit(EXIT_FAILURE)
}
