import XCTest

/// Issue-#46 oracle: bare `F` drives frame-all end to end AND the refit is
/// pixel-observable — the bright-pixel fraction must jump.
///
/// Delivery chain: `pane.click()` focuses the nav view (`mouseDown` claims
/// first responder; the stats overlay is click-through so any pane point
/// focuses), the app-wide key tap in `ViewportNavView` catches bare `F`
/// even when SwiftUI moves focus afterward, `typeKey("f")` maps to
/// `.frameAll`, the intent crosses FFI on the serial engine queue. Rust pins
/// the engine half (`frame_all_moves_published_gpu_pixels`); unit tests pin
/// the mapping and scope guards (`ViewportNavViewTests`); this test pins
/// the delivery half.
///
/// Oracle math: the scene is STATIC (no auto-spin since #46), so a
/// before/after screenshot pair isolates the refit exactly — a lost `F`
/// measures a ratio of exactly 1.0. At the default layout the pane opens
/// narrow (aspect ~0.67, measured `frame 1228x1846`), where fitting the
/// cube runs the camera 4.74 → ~3.75: the pane-lit area grows 1.655x
/// (measured through FFI at exactly that size), which reads ~1.5x
/// window-wide against the dark chrome. Threshold 1.3x keeps wide margin
/// on both sides with zero flakiness.
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

        // Baseline: the warmed-up viewport shows lit pixels. The scene is
        // static, so one frame is the signal — no averaging.
        let windowFrame = app.windows.firstMatch.frame
        let before = try XCTUnwrap(
            viewportBrightPixelFraction(of: app.screenshot(), in: windowFrame),
            "could not map the app window into the screenshot"
        )
        XCTAssertGreaterThan(before, 0.005, "viewport renders no lit pixels before F")

        // Click to focus the nav view, then frame. The overlay is
        // click-through, so clicking its (collapsed) element lands on the
        // MTKView; the key tap delivers `F` regardless of focus.
        let pane = viewportElement(app, "viewportPane")
        XCTAssertTrue(pane.waitForExistence(timeout: 5))
        pane.click()
        app.typeKey("f", modifierFlags: [])

        // One beat for the serial engine queue to run the intent, then the
        // refit must be pixel-observable: fitting from ~4.74 to ~3.75
        // enlarges the cube, so the lit fraction jumps ~1.5x.
        RunLoop.main.run(until: Date().addingTimeInterval(1.0))
        let after = try XCTUnwrap(
            viewportBrightPixelFraction(of: app.screenshot(), in: windowFrame),
            "could not map the app window into the screenshot"
        )
        XCTAssertGreaterThan(after, 0.005, "viewport went dark after F")
        XCTAssertGreaterThan(
            after,
            before * 1.3,
            "F moved no pixels: bright fraction \(before) -> \(after)"
        )
    }
}
