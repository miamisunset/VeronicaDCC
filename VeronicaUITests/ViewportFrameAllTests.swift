import XCTest

/// Issue-#41 oracle: bare `F` drives frame-all end to end — the click
/// focuses the nav view, the key maps to `.frameAll`, and the intent crosses
/// FFI on the serial engine queue without wedging it.
///
/// Scope note: the demo rasterizer (`veronica-scene` `render.rs`) projects
/// with a fixed scale from the turntable angle only — camera distance never
/// reaches pixels — so no brightness oracle can observe the refit until the
/// renderer honors the camera. What this test proves instead: delivery runs
/// clean (no crash, focus works) and the engine queue stays live, since the
/// frame-all intent executes on that queue ahead of later ticks — ticks
/// advancing past the press proves it ran and returned. The mapped units
/// themselves are pinned by `ViewportNavEffectTests` and the key/drag/wheel
/// translation by `ViewportNavViewTests`.
final class ViewportFrameAllTests: XCTestCase {
    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    @MainActor
    func testPressingFFramesAllAndEngineStaysLive() throws {
        let app = XCUIApplication()
        app.launch()

        let warmed = waitForViewportWarmUp(app)
        XCTAssertGreaterThanOrEqual(warmed, 120, "engine never warmed up")

        let pane = viewportElement(app, "viewportPane")
        XCTAssertTrue(pane.waitForExistence(timeout: 5))

        // Baseline: the warmed-up viewport shows lit pixels.
        let windowFrame = app.windows.firstMatch.frame
        let before = try XCTUnwrap(
            viewportBrightPixelFraction(of: app.screenshot(), in: windowFrame),
            "could not map the app window into the screenshot"
        )
        XCTAssertGreaterThan(before, 0.005, "viewport renders no lit pixels before F")

        // Click to focus the nav view (claims first responder), then frame.
        pane.click()
        app.typeKey("f", modifierFlags: [])

        // Ticks must advance well past the press: the frame-all intent runs
        // on the serial engine queue, so a hang or wedged FFI call would
        // freeze the counter here.
        let tick = viewportElement(app, "viewportTickLabel")
        var advanced = warmed
        let deadline = Date().addingTimeInterval(15)
        while Date() < deadline {
            if let raw = tick.value as? String,
                let last = raw.split(separator: " ").last,
                let parsed = Int(last.replacingOccurrences(of: ",", with: "")),
                parsed >= warmed + 30 {
                advanced = parsed
                break
            }
            // Pump, don't sleep: the display link pacing ticks is scheduled
            // on the main run loop, so blocking this actor would starve the
            // very counter being waited on.
            RunLoop.main.run(until: Date().addingTimeInterval(0.5))
        }
        XCTAssertGreaterThanOrEqual(advanced, warmed + 30, "engine stalled after F")

        // The viewport keeps presenting lit pixels after the intent.
        let after = try XCTUnwrap(
            viewportBrightPixelFraction(of: app.screenshot(), in: windowFrame),
            "could not map the app window into the screenshot"
        )
        XCTAssertGreaterThan(after, 0.005, "viewport went dark after F")
    }
}
