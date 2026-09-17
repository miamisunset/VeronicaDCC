import CoreGraphics
import XCTest

/// Issue-#52 oracle: the Recook Loop closed end to end — create a cube
/// through the `Geometry` submenu, see viewport geometry appear, edit `size`
/// in the parameter editor, see the viewport visibly change.
///
/// Real engine only (the mock double owns no scene): launches with
/// `--vrn-reset-graph` for isolation, then models the pixel oracle on
/// `ViewportFrameAllTests` (static scene, before/after bright-pixel ratio
/// over the window). Doubling every size axis grows the projected cube ~4x
/// window-wide against static chrome; threshold 1.3x keeps wide margin on
/// both sides with zero flakiness.
final class CubeRecookLoopTests: XCTestCase {
    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    @MainActor
    func testCreateCubeEditSizeVisiblyChangesViewport() throws {
        let app = XCUIApplication()
        app.launchArguments = ["--vrn-reset-graph"]
        app.launch()

        let warmed = waitForViewportWarmUp(app)
        XCTAssertGreaterThanOrEqual(warmed, 120, "engine never warmed up")

        // Empty scene by decision: clear-only viewport plus overlay text.
        let windowFrame = app.windows.firstMatch.frame
        let empty = try XCTUnwrap(
            viewportBrightPixelFraction(of: app.screenshot(), in: windowFrame),
            "could not map the app window into the screenshot"
        )

        // Create the cube through the Geometry submenu at a canvas point.
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

        // Frame the new geometry, then baseline the 1 m cube.
        let viewport = viewportElement(app, "viewportPane")
        XCTAssertTrue(viewport.waitForExistence(timeout: 5))
        viewport.click()
        app.typeKey("f", modifierFlags: [])
        RunLoop.main.run(until: Date().addingTimeInterval(1.0))
        let before = try XCTUnwrap(
            viewportBrightPixelFraction(of: app.screenshot(), in: windowFrame),
            "could not map the app window into the screenshot"
        )
        XCTAssertGreaterThan(before, empty, "cube never appeared in the viewport")

        // Select the cube (single click selects without diving) and drive
        // every size axis 1 -> 2 through the generic triple-field.
        box1.click()
        let xField = viewportElement(app, "parameterField-size-0")
        XCTAssertTrue(xField.waitForExistence(timeout: 5))
        try setField(app, xField, to: "2")
        try setField(app, viewportElement(app, "parameterField-size-1"), to: "2")
        let zField = viewportElement(app, "parameterField-size-2")
        try setField(app, zField, to: "2")
        // Leaving the triple group commits the unit (see Vec3ParamField).
        viewport.click()
        RunLoop.main.run(until: Date().addingTimeInterval(1.5))
        // The editor echoes the committed triple back from the mirror.
        XCTAssertEqual(xField.value as? String, "2.0")

        let after = try XCTUnwrap(
            viewportBrightPixelFraction(of: app.screenshot(), in: windowFrame),
            "could not map the app window into the screenshot"
        )
        XCTAssertGreaterThan(
            after,
            before * 1.3,
            "size edit moved no pixels: bright fraction \(before) -> \(after)"
        )
    }

    /// Replaces a parameter field's contents with `value` and confirms.
    private func setField(_ app: XCUIApplication, _ field: XCUIElement, to value: String) throws {
        field.click()
        app.typeKey("a", modifierFlags: .command)
        app.typeText(value)
        app.typeKey(.return, modifierFlags: [])
    }
}
