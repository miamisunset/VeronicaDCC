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
/// The viewport pane deliberately exposes no geometry to accessibility
/// (its `NSViewRepresentable` content is invisible to AX, so the pane
/// element's frame collapses to the stats overlay), hence the
/// window-wide bright-pixel metric instead of pane sampling: it is
/// layout-independent and still separates lit geometry (~10%) from a
/// clear-only viewport plus overlay text (~0.3%) by over an order of
/// magnitude.
func viewportBrightPixelFraction(of shot: XCUIScreenshot, in windowFrame: CGRect) -> Double? {
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
    guard let pixels = context.data?.assumingMemoryBound(to: UInt8.self) else {
        return nil
    }
    // Stride-sample: every 8th pixel is plenty for a fraction.
    var bright = 0
    var count = 0
    let stride = 8
    for index in 0..<(width * height) where index % stride == 0 {
        let base = index * 4
        let luminance = 0.2126 * Double(pixels[base]) + 0.7152 * Double(pixels[base + 1])
            + 0.0722 * Double(pixels[base + 2])
        if luminance > 127.5 {
            bright += 1
        }
        count += 1
    }
    guard count > 0 else { return nil }
    return Double(bright) / Double(count)
}
