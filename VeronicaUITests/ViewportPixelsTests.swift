import AppKit
import XCTest

/// Slice-2 oracle: the viewport stats advance across frames because Rust
/// ticks the turntable, and the entity count holds the demo scene size.
final class ViewportPixelsTests: XCTestCase {
    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    /// Queries any descendant by accessibility identifier (fresh each call).
    private func element(_ app: XCUIApplication, _ identifier: String) -> XCUIElement {
        app.descendants(matching: .any)[identifier]
    }

    @MainActor
    func testTickStatsAdvanceAcrossFrames() throws {
        let app = XCUIApplication()
        app.launch()

        let tick = element(app, "viewportTickLabel")
        XCTAssertTrue(tick.waitForExistence(timeout: 10))
        let first = tick.value as? String
        XCTAssertNotNil(first)
        // The display link paces ticks off-MainActor: the label must change.
        let advanced = expectation(
            for: NSPredicate(format: "value != %@", first ?? ""),
            evaluatedWith: tick,
            handler: nil
        )
        wait(for: [advanced], timeout: 5)

        // The demo scene stays three entities (camera, light, cube).
        let entities = element(app, "viewportEntityLabel")
        XCTAssertTrue(entities.waitForExistence(timeout: 5))
        XCTAssertEqual(entities.value as? String, "entities 3")
    }

    /// Slice-3 oracle (#30): the tick label keeps advancing across pane
    /// rearrangements — Stack/Side-by-Side plus Swap must not freeze the
    /// viewport (the slice-2 drawable fight) or black it.
    @MainActor
    func testTickKeepsAdvancingAcrossPaneRearrangements() throws {
        let app = XCUIApplication()
        app.launch()

        let tick = element(app, "viewportTickLabel")
        XCTAssertTrue(tick.waitForExistence(timeout: 10))
        let orientationButton = element(app, "paneOrientationButton")
        XCTAssertTrue(orientationButton.waitForExistence(timeout: 5))
        let swapButton = element(app, "swapPanesButton")
        XCTAssertTrue(swapButton.waitForExistence(timeout: 5))

        // Stack the panes: the tick must keep moving (no freeze).
        let first = tick.value as? String
        orientationButton.tap()
        wait(
            for: [expectation(for: NSPredicate(format: "value != %@", first ?? ""), evaluatedWith: tick)],
            timeout: 5
        )

        // Swap the panes: still advancing.
        let second = tick.value as? String
        swapButton.tap()
        wait(
            for: [expectation(for: NSPredicate(format: "value != %@", second ?? ""), evaluatedWith: tick)],
            timeout: 5
        )

        // And back to side-by-side: the loop survives the round trip.
        let third = tick.value as? String
        orientationButton.tap()
        wait(
            for: [expectation(for: NSPredicate(format: "value != %@", third ?? ""), evaluatedWith: tick)],
            timeout: 5
        )
    }

    /// Regression oracle for the unbundled-metallib blackout (#30 follow-up):
    /// the window must show lit pixels, not just the letterbox clear. Waits
    /// past GPU warm-up (cold ticks publish clear-only frames while the
    /// Bevy pipeline spins up), screenshots the app, and asserts the
    /// window's bright-pixel fraction clears a threshold no clear-only
    /// frame can reach. Window-wide (not pane-sampled) because the pane
    /// exposes no geometry to accessibility; the threshold still separates
    /// lit geometry (~10%) from clear plus overlay text (0.16% measured)
    /// by over an order of magnitude — and a mis-cropped region fails
    /// closed (dark desktop measures ~0%, below threshold).
    @MainActor
    func testViewportPaneShowsLitPixels() throws {
        let app = XCUIApplication()
        app.launch()

        let tick = element(app, "viewportTickLabel")
        XCTAssertTrue(tick.waitForExistence(timeout: 10))
        var tickCount = 0
        let deadline = Date().addingTimeInterval(30)
        while Date() < deadline {
            if let raw = tick.value as? String,
                let last = raw.split(separator: " ").last,
                let parsed = Int(last.replacingOccurrences(of: ",", with: "")),
                parsed >= 120 {
                tickCount = parsed
                break
            }
            // Pump, don't sleep: the display link pacing ticks is scheduled
            // on the main run loop, so blocking this actor would starve the
            // very counter being waited on.
            RunLoop.main.run(until: Date().addingTimeInterval(0.5))
        }
        XCTAssertGreaterThanOrEqual(tickCount, 120, "engine never warmed up")

        let pane = element(app, "viewportPane")
        XCTAssertTrue(pane.waitForExistence(timeout: 5))

        let shot = app.screenshot()
        let windowFrame = app.windows.firstMatch.frame
        let brightFraction = try XCTUnwrap(
            brightPixelFraction(of: shot, in: windowFrame),
            "could not map the app window into the screenshot"
        )
        // A lit cube face adds ~10% bright pixels to the window; a
        // clear-only viewport leaves only white overlay text (~0.3%).
        XCTAssertGreaterThan(brightFraction, 0.005, "viewport renders no lit pixels")
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
    private func brightPixelFraction(of shot: XCUIScreenshot, in windowFrame: CGRect) -> Double? {
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
}
