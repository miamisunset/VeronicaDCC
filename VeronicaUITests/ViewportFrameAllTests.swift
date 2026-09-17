import XCTest

/// Issue-#46 oracle, re-based on the wired scene (#51): bare `F` still
/// delivers frame-all end to end, but the launch scene is EMPTY (viewport
/// camera + key light, no geometry), so frame-all is a defined no-op — the
/// refit must move no pixels while the engine keeps ticking.
///
/// Delivery chain: `pane.click()` focuses the nav view (`mouseDown` claims
/// first responder; the stats overlay is click-through so any pane point
/// focuses), the app-wide key tap in `ViewportNavView` catches bare `F`
/// even when SwiftUI moves focus afterward, `typeKey("f")` maps to
/// `.frameAll`, the intent crosses FFI on the serial engine queue. Rust pins
/// the engine half (empty frame-all preserves the camera); unit tests pin
/// the mapping and scope guards (`ViewportNavViewTests`); this test pins
/// the delivery half on the empty path. F-on-content pixel-observability
/// lives in `CubeRecookLoopTests` (create → frame → size edit).
///
/// Oracle math: the scene is STATIC (no auto-spin since #46) and empty, so
/// a before/after screenshot pair isolates the (non-)refit exactly — only
/// overlay text digits (tick/fps) may flicker a few pixels. Threshold 1.3x
/// is the same tripwire as before, now asserting it never fires.
final class ViewportFrameAllTests: XCTestCase {
    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    @MainActor
    func testPressingFOnEmptySceneMovesNoPixels() throws {
        let app = XCUIApplication()
        // Isolation: a persisted cube from another test would give F real
        // geometry to fit, breaking the empty-scene contract.
        app.launchArguments = ["--vrn-reset-graph"]
        app.launch()

        let warmed = waitForViewportWarmUp(app)
        XCTAssertGreaterThanOrEqual(warmed, 120, "engine never warmed up")

        // Click to focus the nav view, then frame. The overlay is
        // click-through, so clicking its (collapsed) element lands on the
        // MTKView; the key tap delivers `F` regardless of focus.
        let pane = viewportElement(app, "viewportPane")
        XCTAssertTrue(pane.waitForExistence(timeout: 5))
        pane.click()
        let tick = viewportElement(app, "viewportTickLabel")
        XCTAssertTrue(tick.waitForExistence(timeout: 5))
        let tickBefore = tick.value as? String

        let windowFrame = app.windows.firstMatch.frame
        let before = try XCTUnwrap(
            viewportBrightPixelFraction(of: app.screenshot(), in: windowFrame),
            "could not map the app window into the screenshot"
        )
        app.typeKey("f", modifierFlags: [])

        // One beat for the serial engine queue to run the intent, then the
        // empty refit must have moved nothing: no geometry exists to fit.
        RunLoop.main.run(until: Date().addingTimeInterval(1.0))
        let after = try XCTUnwrap(
            viewportBrightPixelFraction(of: app.screenshot(), in: windowFrame),
            "could not map the app window into the screenshot"
        )
        XCTAssertLessThan(
            after,
            max(before * 1.3, 0.005),
            "F moved pixels on an empty scene: bright fraction \(before) -> \(after)"
        )

        // And F wedged nothing: the engine is still ticking underneath.
        XCTAssertNotEqual(tick.value as? String, tickBefore)
    }
}
