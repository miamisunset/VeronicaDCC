import XCTest

/// Slice-2 oracle: the viewport stats advance across frames because Rust
/// ticks the turntable, and the entity count holds the demo scene size.
final class ViewportPixelsTests: XCTestCase {
    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    /// Queries any descendant by accessibility identifier (fresh each call).
    private func element(_ app: XCUIApplication, _ identifier: String) -> XCUIElement {
        viewportElement(app, identifier)
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

    /// Issue-#32 oracle: the timing line exists and carries the fps meter
    /// plus the tick/stage splits. No warm-up wait: the line renders from
    /// initial state ("0 fps · ...") before the first tick lands.
    @MainActor
    func testTimingLabelShowsFps() throws {
        let app = XCUIApplication()
        app.launch()

        let timing = element(app, "viewportTimingLabel")
        XCTAssertTrue(timing.waitForExistence(timeout: 10))
        let value = timing.value as? String
        XCTAssertNotNil(value)
        XCTAssertTrue(value?.contains("fps") ?? false)
        XCTAssertTrue(value?.contains("ms tick") ?? false)

        // The frame-extents line rides alongside for the #32 matrix.
        let frame = element(app, "viewportFrameLabel")
        XCTAssertTrue(frame.waitForExistence(timeout: 5))
        XCTAssertTrue((frame.value as? String)?.hasPrefix("frame ") ?? false)
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

        let tickCount = waitForViewportWarmUp(app)
        XCTAssertGreaterThanOrEqual(tickCount, 120, "engine never warmed up")

        let pane = element(app, "viewportPane")
        XCTAssertTrue(pane.waitForExistence(timeout: 5))

        let shot = app.screenshot()
        let windowFrame = app.windows.firstMatch.frame
        let brightFraction = try XCTUnwrap(
            viewportBrightPixelFraction(of: shot, in: windowFrame),
            "could not map the app window into the screenshot"
        )
        // A lit cube face adds ~10% bright pixels to the window; a
        // clear-only viewport leaves only white overlay text (~0.3%).
        XCTAssertGreaterThan(brightFraction, 0.005, "viewport renders no lit pixels")
    }
}
