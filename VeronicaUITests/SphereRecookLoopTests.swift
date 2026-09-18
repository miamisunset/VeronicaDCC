import CoreGraphics
import XCTest

/// Issue-#74 oracle: the Sphere slice end to end — create through the
/// `Geometry` submenu, see viewport geometry appear, change a resolution and
/// see pixels move; plus the T4 staleness gap's real test, untestable with
/// the fixed-topology Cube (see `selection.rs:889-913`): a picked face
/// survives a radius recook (same count) and clears on a segments recook
/// (changed count).
///
/// Real engine only (the mock double owns no scene): launches with
/// `--vrn-reset-graph` for isolation. Pixel oracles model on
/// `CubeRecookLoopTests` (bright-pixel ratio) and `ViewportTapTests`
/// (pixel-difference fraction); the highlight oracle on
/// `ViewportSelectionTests` (orange-pixel fraction, absolute thresholds).
final class SphereRecookLoopTests: XCTestCase {
    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    @MainActor
    func testCreateSphereEditSegmentsVisiblyChangesViewport() throws {
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

        createSphere(app)

        // Focus via a background miss (pane corner, clear color even after
        // framing): focusing never picks a face, so no highlight tint ever
        // contaminates the bright fractions. Then frame the new geometry.
        let viewport = viewportElement(app, "viewportPane")
        XCTAssertTrue(viewport.waitForExistence(timeout: 5))
        viewport.coordinate(withNormalizedOffset: CGVector(dx: 0.06, dy: 0.06)).click()
        app.typeKey("f", modifierFlags: [])
        RunLoop.main.run(until: Date().addingTimeInterval(1.0))
        let beforeShot = app.screenshot()
        let before = try XCTUnwrap(
            viewportBrightPixelFraction(of: beforeShot, in: windowFrame),
            "could not map the app window into the screenshot"
        )
        XCTAssertGreaterThan(before, empty, "sphere never appeared in the viewport")

        // Select the sphere (single click selects without diving) and drive
        // segments 32 -> 8 through the generic integer field. Unlike the
        // cube's 4x silhouette growth this keeps the silhouette, so the pin
        // is the pixel-difference fraction (moved edges), not a bright
        // ratio: coarsening 32-gon to 8-gon visibly re-cuts the outline.
        let box1 = viewportElement(app, "operatorBox-1")
        XCTAssertTrue(box1.waitForExistence(timeout: 5))
        box1.click()
        let segmentsField = viewportElement(app, "parameterField-segments")
        XCTAssertTrue(segmentsField.waitForExistence(timeout: 5))
        try setField(app, segmentsField, to: "8")
        // Commit through the node graph, NOT the viewport: a viewport click
        // here could pick a face and paint a highlight into the after shot.
        box1.click()
        RunLoop.main.run(until: Date().addingTimeInterval(1.5))
        // The editor echoes the committed integer back from the mirror.
        XCTAssertEqual(segmentsField.value as? String, "8")

        let afterShot = app.screenshot()
        let after = try XCTUnwrap(
            viewportBrightPixelFraction(of: afterShot, in: windowFrame),
            "could not map the app window into the screenshot"
        )
        XCTAssertGreaterThan(after, 0.005, "sphere vanished after the segments edit")
        let moved = try XCTUnwrap(
            viewportPixelDifferenceFraction(before: beforeShot, after: afterShot, in: windowFrame),
            "could not diff the window across the segments edit"
        )
        XCTAssertGreaterThan(
            moved,
            0.005,
            "segments edit moved no pixels: difference fraction \(moved)"
        )
    }

    @MainActor
    func testRadiusChangeRetainsHighlightOnSameFace() throws {
        let app = XCUIApplication()
        app.launchArguments = ["--vrn-reset-graph"]
        app.launch()

        let warmed = waitForViewportWarmUp(app)
        XCTAssertGreaterThanOrEqual(warmed, 120, "engine never warmed up")
        createSphere(app)

        let windowFrame = app.windows.firstMatch.frame
        let viewport = viewportElement(app, "viewportPane")
        XCTAssertTrue(viewport.waitForExistence(timeout: 5))

        // Baseline before any viewport tap: nothing is selected, so no
        // warm pixels beyond the window chrome (~3e-5). The baseline must
        // precede the focus click — even focusing via a face tap would
        // paint the Selection first.
        let plain = try XCTUnwrap(
            viewportOrangePixelFraction(of: app.screenshot(), in: windowFrame),
            "could not map the app window into the screenshot"
        )
        XCTAssertLessThan(plain, 0.001, "warm pixels before any tap: \(plain)")

        // Focus via a background miss so focusing selects nothing, then
        // frame the new geometry.
        viewport.coordinate(withNormalizedOffset: CGVector(dx: 0.06, dy: 0.06)).click()
        app.typeKey("f", modifierFlags: [])
        RunLoop.main.run(until: Date().addingTimeInterval(1.0))

        // Min resolution first: at default 960 triangles one painted face
        // is ~60 window pixels, unmeasurable against chrome. Segments 5 +
        // rings 2 cook 10 huge faces, so a picked face reads ~1% orange
        // (measured 0.0104 during triage) and the 0.005/0.001 thresholds
        // below keep wide margins on both sides.
        try driveMinResolution(app)

        // Pick off the seam: the i=0 vertex meridian faces the camera by
        // construction, so a pane-center tap straddles the duplicated seam
        // and the MSAA blend deselects. NDC x=+0.15 sits deep inside the
        // first segment's face (same measured 0.0104).
        let picked = try pickOffSeamFace(app, in: windowFrame)
        XCTAssertGreaterThan(picked, 0.005, "tap highlighted no face: \(picked)")

        // Grow radius 0.5 -> 1.0 through the generic float field: the
        // recook keeps the triangle count, so the Selection is retained on
        // the same face ordinal and the orange persists (grown ~4x).
        let box1 = viewportElement(app, "operatorBox-1")
        XCTAssertTrue(box1.waitForExistence(timeout: 5))
        box1.click()
        let radiusField = viewportElement(app, "parameterField-radius")
        XCTAssertTrue(radiusField.waitForExistence(timeout: 5))
        try setField(app, radiusField, to: "1.0")
        // Leave the field through the node graph, NOT the viewport: a
        // viewport click here would re-pick the grown face and repaint the
        // Selection even if the recook had dropped it, making this a false
        // positive for exactly the retention it claims to pin.
        box1.click()
        RunLoop.main.run(until: Date().addingTimeInterval(2.0))

        let kept = try XCTUnwrap(
            viewportOrangePixelFraction(of: app.screenshot(), in: windowFrame),
            "could not map the app window into the screenshot"
        )
        XCTAssertGreaterThan(kept, 0.005, "radius change lost the highlight: \(kept)")
    }

    @MainActor
    func testSegmentsChangeClearsHighlight() throws {
        let app = XCUIApplication()
        app.launchArguments = ["--vrn-reset-graph"]
        app.launch()

        let warmed = waitForViewportWarmUp(app)
        XCTAssertGreaterThanOrEqual(warmed, 120, "engine never warmed up")
        createSphere(app)

        let windowFrame = app.windows.firstMatch.frame
        let viewport = viewportElement(app, "viewportPane")
        XCTAssertTrue(viewport.waitForExistence(timeout: 5))
        // Focus via a background miss so focusing selects nothing, then
        // frame the new geometry.
        viewport.coordinate(withNormalizedOffset: CGVector(dx: 0.06, dy: 0.06)).click()
        app.typeKey("f", modifierFlags: [])
        RunLoop.main.run(until: Date().addingTimeInterval(1.0))

        // Same min-resolution + off-seam pick as the radius test: one face
        // reads ~1% orange, so clearing back to chrome is unmistakable.
        try driveMinResolution(app)
        let picked = try pickOffSeamFace(app, in: windowFrame)
        XCTAssertGreaterThan(picked, 0.005, "tap highlighted no face: \(picked)")

        // Coarsen segments 5 -> 8 through the generic integer field: the
        // recook changes the triangle total (10 -> 16), so the stored pick
        // no longer describes this mesh and the Selection clears. Commit
        // through the node graph (see the radius test): a viewport click
        // would re-pick and hide the clearing this test exists to pin.
        let box1 = viewportElement(app, "operatorBox-1")
        XCTAssertTrue(box1.waitForExistence(timeout: 5))
        box1.click()
        let segmentsField = viewportElement(app, "parameterField-segments")
        XCTAssertTrue(segmentsField.waitForExistence(timeout: 5))
        try setField(app, segmentsField, to: "8")
        box1.click()
        RunLoop.main.run(until: Date().addingTimeInterval(2.0))

        let cleared = try XCTUnwrap(
            viewportOrangePixelFraction(of: app.screenshot(), in: windowFrame),
            "could not map the app window into the screenshot"
        )
        XCTAssertLessThan(cleared, 0.001, "segments change kept the highlight: \(cleared)")
    }

    /// Drives the sphere to minimum resolution (segments 5, rings 2 = 10
    /// triangles) through the generic integer fields and waits for the
    /// recook, so one picked face is window-measurable. The editor echoes
    /// both commits back from the mirror.
    private func driveMinResolution(_ app: XCUIApplication) throws {
        let box1 = viewportElement(app, "operatorBox-1")
        XCTAssertTrue(box1.waitForExistence(timeout: 5))
        box1.click()
        let segmentsField = viewportElement(app, "parameterField-segments")
        XCTAssertTrue(segmentsField.waitForExistence(timeout: 5))
        try setField(app, segmentsField, to: "5")
        let ringsField = viewportElement(app, "parameterField-rings")
        XCTAssertTrue(ringsField.waitForExistence(timeout: 5))
        try setField(app, ringsField, to: "2")
        // Commit through the node graph (see the radius test): a viewport
        // click here could pick a face and paint the Selection before the
        // baseline pick below.
        box1.click()
        RunLoop.main.run(until: Date().addingTimeInterval(2.0))
        XCTAssertEqual(segmentsField.value as? String, "5")
        XCTAssertEqual(ringsField.value as? String, "2")
    }

    /// Taps one face off the camera-facing seam and returns the orange
    /// fraction after the pick settles. See the radius test for why the
    /// tap sits at NDC x=+0.15 instead of the pane center.
    private func pickOffSeamFace(_ app: XCUIApplication, in windowFrame: CGRect) throws -> Double {
        let viewport = viewportElement(app, "viewportPane")
        XCTAssertTrue(viewport.waitForExistence(timeout: 5))
        viewport.coordinate(withNormalizedOffset: CGVector(dx: 0.575, dy: 0.5)).click()
        RunLoop.main.run(until: Date().addingTimeInterval(3.0))
        return try XCTUnwrap(
            viewportOrangePixelFraction(of: app.screenshot(), in: windowFrame),
            "could not map the app window into the screenshot"
        )
    }

    /// Creates a sphere through the Geometry submenu at a canvas point and
    /// waits for its operator box (shared `CubeRecookLoopTests` setup, one
    /// token swapped).
    private func createSphere(_ app: XCUIApplication) {        let pane = viewportElement(app, "nodeGraphPane")
        XCTAssertTrue(pane.waitForExistence(timeout: 10))
        let target = pane.coordinate(withNormalizedOffset: CGVector(dx: 0, dy: 0))
            .withOffset(CGVector(dx: 120, dy: 100))
        target.click()
        target.rightClick()
        let geometry = app.menuItems["Geometry"]
        XCTAssertTrue(geometry.waitForExistence(timeout: 5))
        geometry.click()
        let sphereItem = app.menuItems["Sphere"]
        XCTAssertTrue(sphereItem.waitForExistence(timeout: 5))
        sphereItem.click()

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
