import CoreGraphics
import XCTest

/// Issue-#64 oracle: the user-facing slice — tap a Cube face to highlight
/// it, tap background to clear, resize to retain.
///
/// Setup mirrors `ViewportTapTests` (cube through the Geometry submenu,
/// `F` to frame) so the framed cube sits at the pane center: a plain
/// `viewport.click()` taps the front face. The oracle is the orange-pixel
/// fraction (the primvar highlight tint): a picked face is its only
/// source, so the assertions are absolute, not relative — no orange
/// before the tap, clearly some after, none again after a background
/// miss, still some after a recook.
final class ViewportSelectionTests: XCTestCase {
    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    @MainActor
    func testTapFaceHighlightsAndBackgroundClears() throws {
        let app = XCUIApplication()
        app.launchArguments = ["--vrn-reset-graph"]
        app.launch()

        let warmed = waitForViewportWarmUp(app)
        XCTAssertGreaterThanOrEqual(warmed, 120, "engine never warmed up")
        createCube(app)

        let windowFrame = app.windows.firstMatch.frame
        let viewport = viewportElement(app, "viewportPane")
        XCTAssertTrue(viewport.waitForExistence(timeout: 5))

        // Baseline before any viewport tap: nothing is selected, so no
        // warm pixels beyond the window chrome (~1e-4). The baseline must
        // precede the focus click — even focusing via a face tap would
        // paint the Selection first.
        let plain = try XCTUnwrap(
            viewportOrangePixelFraction(of: app.screenshot(), in: windowFrame),
            "could not map the app window into the screenshot"
        )
        XCTAssertLessThan(plain, 0.002, "warm pixels before any tap: \(plain)")

        // Focus via a background miss (pane corner, clear color even after
        // framing): focusing never selects, so the baseline still holds.
        viewport.coordinate(withNormalizedOffset: CGVector(dx: 0.06, dy: 0.06)).click()
        app.typeKey("f", modifierFlags: [])
        RunLoop.main.run(until: Date().addingTimeInterval(1.0))

        // Tap the framed cube's front face: the pick paints the Selection,
        // the next frames present the tinted face (~7% window-wide).
        viewport.click()
        RunLoop.main.run(until: Date().addingTimeInterval(1.5))
        let picked = try XCTUnwrap(
            viewportOrangePixelFraction(of: app.screenshot(), in: windowFrame),
            "could not map the app window into the screenshot"
        )
        XCTAssertGreaterThan(picked, 0.02, "tap highlighted no face: \(picked)")

        // Tap empty space again: the miss clears the Selection, the warmth
        // goes away with it.
        viewport.coordinate(withNormalizedOffset: CGVector(dx: 0.06, dy: 0.06)).click()
        RunLoop.main.run(until: Date().addingTimeInterval(1.5))
        let cleared = try XCTUnwrap(
            viewportOrangePixelFraction(of: app.screenshot(), in: windowFrame),
            "could not map the app window into the screenshot"
        )
        XCTAssertLessThan(cleared, 0.002, "background tap kept the highlight: \(cleared)")
    }

    @MainActor
    func testResizeKeepsHighlightOnSameFace() throws {
        let app = XCUIApplication()
        app.launchArguments = ["--vrn-reset-graph"]
        app.launch()

        let warmed = waitForViewportWarmUp(app)
        XCTAssertGreaterThanOrEqual(warmed, 120, "engine never warmed up")
        createCube(app)

        let windowFrame = app.windows.firstMatch.frame
        let viewport = viewportElement(app, "viewportPane")
        XCTAssertTrue(viewport.waitForExistence(timeout: 5))
        // Focus via a background miss so focusing selects nothing, then
        // frame the new geometry.
        viewport.coordinate(withNormalizedOffset: CGVector(dx: 0.06, dy: 0.06)).click()
        app.typeKey("f", modifierFlags: [])
        RunLoop.main.run(until: Date().addingTimeInterval(1.0))

        // Pick the front face first.
        viewport.click()
        RunLoop.main.run(until: Date().addingTimeInterval(1.5))
        let picked = try XCTUnwrap(
            viewportOrangePixelFraction(of: app.screenshot(), in: windowFrame),
            "could not map the app window into the screenshot"
        )
        XCTAssertGreaterThan(picked, 0.02, "tap highlighted no face: \(picked)")

        // Grow every size axis 1 -> 2 through the generic triple-field
        // (see `CubeRecookLoopTests`): the recook retains the Selection on
        // the same face ordinal, so the orange persists at the same scale.
        let box1 = viewportElement(app, "operatorBox-1")
        XCTAssertTrue(box1.waitForExistence(timeout: 5))
        box1.click()
        let xField = viewportElement(app, "parameterField-size-0")
        XCTAssertTrue(xField.waitForExistence(timeout: 5))
        try setField(app, xField, to: "2")
        try setField(app, viewportElement(app, "parameterField-size-1"), to: "2")
        try setField(app, viewportElement(app, "parameterField-size-2"), to: "2")
        // Leave the triple group through the node graph, NOT the viewport:
        // a viewport click here would re-pick the grown face and repaint
        // the Selection even if the recook had dropped it, making this a
        // false positive for exactly the retention it claims to pin.
        // Focusing another pane commits the unit all the same.
        box1.click()
        RunLoop.main.run(until: Date().addingTimeInterval(1.5))

        let kept = try XCTUnwrap(
            viewportOrangePixelFraction(of: app.screenshot(), in: windowFrame),
            "could not map the app window into the screenshot"
        )
        XCTAssertGreaterThan(kept, 0.02, "resize lost the highlight: \(kept)")
    }

    /// Creates a cube through the Geometry submenu at a canvas point and
    /// waits for its operator box (shared `CubeRecookLoopTests` setup).
    private func createCube(_ app: XCUIApplication) {
        let pane = viewportElement(app, "nodeGraphPane")
        XCTAssertTrue(pane.waitForExistence(timeout: 10))
        let target = pane.coordinate(withNormalizedOffset: CGVector(dx: 0, dy: 0))
            .withOffset(CGVector(dx: 120, dy: 100))
        target.click()
        target.rightClick()
        let geometry = app.menuItems["Geometry"]
        XCTAssertTrue(geometry.waitForExistence(timeout: 5))
        geometry.click()
        let cubeItem = app.menuItems["Cube"]
        XCTAssertTrue(cubeItem.waitForExistence(timeout: 5))
        cubeItem.click()

        let box1 = viewportElement(app, "operatorBox-1")
        XCTAssertTrue(box1.waitForExistence(timeout: 5))
    }

    /// Replaces a parameter field's contents with `value` and confirms.
    private func setField(_ app: XCUIApplication, _ field: XCUIElement, to value: String) throws {
        field.click()
        app.typeKey("a", modifierFlags: .command)
        app.typeText(value)
        app.typeKey(.return, modifierFlags: [])
    }
}
