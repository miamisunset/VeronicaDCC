import AppKit
import XCTest

/// Queries any descendant by accessibility identifier (fresh each call).
func viewportElement(_ app: XCUIApplication, _ identifier: String) -> XCUIElement {
    app.descendants(matching: .any)[identifier]
}

/// Waits until the engine warms up (`tick >= minimumTick`), pumping the main
/// run loop so the display-link pacing ticks keeps running, and returns the
/// reached count (0 on timeout; callers assert the floor they need).
///
/// Cold ticks publish clear-only frames while the Bevy pipeline spins up, so
/// pixel oracles must wait past warm-up before screenshotting.
@MainActor
func waitForViewportWarmUp(
    _ app: XCUIApplication,
    minimumTick: Int = 120,
    timeout: TimeInterval = 30
) -> Int {
    let tick = viewportElement(app, "viewportTickLabel")
    XCTAssertTrue(tick.waitForExistence(timeout: 10))
    var tickCount = 0
    let deadline = Date().addingTimeInterval(timeout)
    while Date() < deadline {
        if let raw = tick.value as? String,
            let last = raw.split(separator: " ").last,
            let parsed = Int(last.replacingOccurrences(of: ",", with: "")),
            parsed >= minimumTick {
            tickCount = parsed
            break
        }
        // Pump, don't sleep: the display link pacing ticks is scheduled
        // on the main run loop, so blocking this actor would starve the
        // very counter being waited on.
        RunLoop.main.run(until: Date().addingTimeInterval(0.5))
    }
    return tickCount
}

/// Fraction of `windowFrame` (points, top-left origin) whose relative
/// luminance exceeds 0.5, sampled from `shot` (device pixels, top-left
/// origin, spans the main display).
///
/// Window-wide (not pane-sampled) by choice: the metric is
/// layout-independent while still separating lit geometry (~10%) from a
/// clear-only viewport plus overlay text (~0.3%) by over an order of
/// magnitude. (The MTKView itself never surfaces in the AX tree — proven
/// at #46 — but a full-pane `Color.clear` proxy now carries the pane
/// identifier for position-sensitive gestures; see `ViewportView`.) For
/// before/after ratios the static side chrome dilutes but cannot erase the
/// change, since only pane pixels move.
func viewportBrightPixelFraction(of shot: XCUIScreenshot, in windowFrame: CGRect) -> Double? {
    guard let crop = croppedWindowPixels(of: shot, in: windowFrame) else {
        return nil
    }
    // Stride-sample: every 8th pixel is plenty for a fraction.
    var bright = 0
    var count = 0
    let stride = 8
    for index in 0..<(crop.width * crop.height) where index % stride == 0 {
        let base = index * 4
        let luminance = 0.2126 * Double(crop.pixels[base]) + 0.7152 * Double(crop.pixels[base + 1])
            + 0.0722 * Double(crop.pixels[base + 2])
        if luminance > 127.5 {
            bright += 1
        }
        count += 1
    }
    guard count > 0 else { return nil }
    return Double(bright) / Double(count)
}

/// Fraction of `windowFrame` pixels reading as selection-warm (the
/// primvar highlight tint, issue #64), sampled like `viewportBrightPixelFraction`.
///
/// Warm means red-dominant over blue (`R-B > 40`, `G-B > 20`): the tint is
/// a light peach whose blue channel reaches ~190, so an absolute blue cap
/// would reject the very pixels it must catch, while lit gray geometry
/// reads near-equal channels and the dark chrome reads near-zero. Window
/// chrome contributes only the traffic lights (~1e-4); a picked face
/// covers ~7% window-wide, so the oracle thresholds sit orders of
/// magnitude clear on both sides. Window-wide by the same AX-tree
/// necessity as the bright oracle.
func viewportOrangePixelFraction(of shot: XCUIScreenshot, in windowFrame: CGRect) -> Double? {
    guard let crop = croppedWindowPixels(of: shot, in: windowFrame) else {
        return nil
    }
    // Stride-sample: every 8th pixel is plenty for a fraction.
    var orange = 0
    var count = 0
    let stride = 8
    for index in 0..<(crop.width * crop.height) where index % stride == 0 {
        // `Int` before any arithmetic: `UInt8 + 40` traps on bright pixels.
        let base = index * 4
        let red = Int(crop.pixels[base])
        let green = Int(crop.pixels[base + 1])
        let blue = Int(crop.pixels[base + 2])
        if red > 150, red - blue > 40, green - blue > 20 {
            orange += 1
        }
        count += 1
    }
    guard count > 0 else { return nil }
    return Double(orange) / Double(count)
}

/// Fraction of `windowFrame` pixels whose RGB differs between two
/// screenshots, or `nil` when either shot cannot be mapped.
///
/// The companion oracle to `viewportBrightPixelFraction` for motion (issue
/// #59): bright counts are rotation-stable (orbiting a framed cube swaps
/// which faces are lit without changing how many pixels are), but moved
/// edges and flipped shading change the pixels themselves. Screenshots are
/// lossless PNG, so a static scene diffs to exactly the overlay-text
/// flicker (~1e-5); any real camera move lands orders of magnitude above.
func viewportPixelDifferenceFraction(
    before: XCUIScreenshot,
    after: XCUIScreenshot,
    in windowFrame: CGRect
) -> Double? {
    guard let a = croppedWindowPixels(of: before, in: windowFrame),
        let b = croppedWindowPixels(of: after, in: windowFrame),
        a.width == b.width, a.height == b.height
    else {
        return nil
    }
    var changed = 0
    let total = a.width * a.height
    for index in 0..<total {
        let base = index * 4
        // Sum of absolute RGB channel differences (0–765). Threshold 48
        // ignores encode/color noise without blunting real motion: a moved
        // edge or relit face swings channels by the hundreds.
        let delta = abs(Int(a.pixels[base]) - Int(b.pixels[base]))
            + abs(Int(a.pixels[base + 1]) - Int(b.pixels[base + 1]))
            + abs(Int(a.pixels[base + 2]) - Int(b.pixels[base + 2]))
        if delta > 48 {
            changed += 1
        }
    }
    guard total > 0 else { return nil }
    return Double(changed) / Double(total)
}

/// Normalized RGBA pixels of `windowFrame` cropped from `shot`, shared by
/// the bright-fraction and pixel-difference oracles so both map screenshots
/// identically: `windowFrame` in points (top-left origin), `shot` in device
/// pixels (top-left origin, spans the main display).
private struct WindowCrop {
    /// Row-major RGBA bytes (`bytesPerRow == width * 4`, no stride gaps).
    var pixels: [UInt8]
    /// Crop width in device pixels.
    var width: Int
    /// Crop height in device pixels.
    var height: Int
}

/// Crop `shot` to `windowFrame` (see `viewportBrightPixelFraction` for the
/// mapping), or `nil` when the shot cannot be mapped.
private func croppedWindowPixels(
    of shot: XCUIScreenshot,
    in windowFrame: CGRect
) -> WindowCrop? {
    guard windowFrame.width > 0, windowFrame.height > 0,
        let screenSize = NSScreen.main?.frame.size, screenSize.width > 0,
        let image = NSImage(data: shot.pngRepresentation),
        let source = image.cgImage(forProposedRect: nil, context: nil, hints: nil)
    else {
        return nil
    }
    let scale = CGFloat(source.width) / screenSize.width
    let pixelRect = CGRect(
        x: Int(windowFrame.minX * scale),
        y: Int(windowFrame.minY * scale),
        width: Int(windowFrame.width * scale),
        height: Int(windowFrame.height * scale)
    ).intersection(CGRect(x: 0, y: 0, width: source.width, height: source.height))
    guard !pixelRect.isNull, pixelRect.width >= 4, pixelRect.height >= 4,
        let cropped = source.cropping(to: pixelRect)
    else {
        return nil
    }
    let width = cropped.width
    let height = cropped.height
    guard let context = CGContext(
        data: nil,
        width: width,
        height: height,
        bitsPerComponent: 8,
        bytesPerRow: width * 4,
        space: CGColorSpaceCreateDeviceRGB(),
        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
    ) else {
        return nil
    }
    context.draw(cropped, in: CGRect(x: 0, y: 0, width: width, height: height))
    guard let data = context.data?.assumingMemoryBound(to: UInt8.self) else {
        return nil
    }
    let pixels = Array(UnsafeBufferPointer(start: data, count: width * height * 4))
    return WindowCrop(pixels: pixels, width: width, height: height)
}
