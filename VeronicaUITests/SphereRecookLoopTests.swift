import CoreGraphics
import XCTest

/// Issue-#74 oracle: the Sphere slice end to end — create through the
/// `Geometry` submenu, see viewport geometry appear, change a resolution and
/// see pixels move; plus the T4 staleness gap's real test, untestable with
/// the fixed-topology Cube (see `changed_count_clears_selection_on_recook`
/// in `selection.rs`): a picked polygon survives a radius recook (same
/// count) and clears on a segments recook (changed count).
///
/// Real engine only (the mock double owns no scene): launches with
/// `--vrn-reset-graph` for isolation. Pixel oracles model on
/// `CubeRecookLoopTests` (bright-pixel ratio) and `ViewportTapTests`
/// (pixel-difference fraction); the highlight oracle on
/// `ViewportSelectionTests` (orange-pixel fraction, absolute thresholds).
final class SphereRecookLoopTests: XCTestCase {
    /// Whole-quad floor for `testQuadPickPaintsWholeQuad`: calibrated
    /// 2026-09-17 (see the oracle comment there). Sits +13% above the
    /// same-resolution single-triangle ceiling (0.0051) and -12% below the
    /// quad floor (0.0066): thinner margins than the 0.005/0.001 pair, but
    /// the quad cluster is tight (±1.6% across runs). If this ever flakes,
    /// re-run the calibration sweep described in the test comment.
    private static let quadPaintFloor = 0.0058

    /// Whole-quad ceiling: two quads would read ~0.013 and a whole band
    /// ~0.033, so 0.012 (+76% above the observed 0.0068 quad max) fails
    /// only on genuine over-paint, which the floor alone would miss.
    private static let quadPaintCeiling = 0.012

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

        // Min resolution first: at default 960 triangles one painted
        // polygon is ~60 window pixels, unmeasurable against chrome.
        // Segments 5 + rings 2 cook 10 huge fan triangles = 10 polygons
        // 1:1, so a picked polygon reads ~1% orange (measured 0.0104
        // during triage) and the 0.005/0.001 thresholds below keep wide
        // margins on both sides.
        try driveMinResolution(app)

        // Pick off the seam: the i=0 vertex meridian faces the camera by
        // construction, so a pane-center tap straddles the duplicated seam
        // and the MSAA blend deselects. NDC x=+0.15 sits deep inside the
        // first segment's polygon (same measured 0.0104).
        let picked = try pickOffSeamPolygon(app, in: windowFrame)
        XCTAssertGreaterThan(picked, 0.005, "tap highlighted no polygon: \(picked)")

        // Grow radius 0.5 -> 1.0 through the generic float field: the
        // recook keeps the polygon count, so the Selection is retained on
        // the same polygon id and the orange persists (grown ~4x).
        let box1 = viewportElement(app, "operatorBox-1")
        XCTAssertTrue(box1.waitForExistence(timeout: 5))
        box1.click()
        let radiusField = viewportElement(app, "parameterField-radius")
        XCTAssertTrue(radiusField.waitForExistence(timeout: 5))
        try setField(app, radiusField, to: "1.0")
        // Leave the field through the node graph, NOT the viewport: a
        // viewport click here would re-pick the grown polygon and repaint
        // the Selection even if the recook had dropped it, making this a
        // false positive for exactly the retention it claims to pin.
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

        // Same min-resolution + off-seam pick as the radius test: one
        // polygon reads ~1% orange, so clearing back to chrome is
        // unmistakable.
        try driveMinResolution(app)
        let picked = try pickOffSeamPolygon(app, in: windowFrame)
        XCTAssertGreaterThan(picked, 0.005, "tap highlighted no polygon: \(picked)")

        // Coarsen segments 5 -> 8 through the generic integer field: the
        // recook changes the polygon total (10 -> 16), so the stored pick
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

    @MainActor
    func testQuadPickPaintsWholeQuad() throws {
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

        // Quad-band resolution: segments 5 + rings 3 cook 20 triangles in
        // 15 polygons (10 fan singles + 5 quad pairs, see SPHERE_QUAD_JSON
        // in `selection.rs`). With one quad band between the two fan rows,
        // the vertical-middle tap (dy 0.5) lands inside the quad band — the
        // same off-seam dx as `pickOffSeamPolygon`.
        try driveQuadBandResolution(app)
        let painted = try pickOffSeamPolygon(app, in: windowFrame)

        // Whole-quad oracle (polygon epoch #79): the P1 `tri_to_poly`
        // grouping maps the resolved triangle to its polygon, so one tap
        // paints both band triangles, not one. Calibrated 2026-09-17
        // (same window geometry as the 0.0104 triage datum, which still
        // reproduces exactly): an equatorial quad at 5x3 reads 0.0066-0.0068
        // window-wide (6 samples across 2 runs), while a single triangle at
        // this resolution — the 5x3 polar fan — reads 0.0051.
        // `quadPaintFloor` sits between the two (+13% above the triangle
        // ceiling, -12% below the quad floor); a one-triangle paint of the
        // equatorial band would read ~0.003 and fail here. The Rust
        // backstop is `painted == 6` in `selection.rs`
        // (`sphere_quad_pick_paints_both_triangles`), which pins the mask
        // exactly; this oracle pins the pixels — floor against under-paint,
        // ceiling against over-paint (a whole band or mesh).
        XCTAssertGreaterThan(
            painted,
            Self.quadPaintFloor,
            "tap painted one triangle, not the quad: \(painted)"
        )
        XCTAssertLessThan(
            painted,
            Self.quadPaintCeiling,
            "tap over-painted past one quad: \(painted)"
        )
    }

    /// Drives the sphere to a resolution (segments + rings) through the
    /// generic integer fields and waits for the recook. The editor echoes
    /// both commits back from the mirror.
    private func driveResolution(_ app: XCUIApplication, segments: String, rings: String) throws {
        let box1 = viewportElement(app, "operatorBox-1")
        XCTAssertTrue(box1.waitForExistence(timeout: 5))
        box1.click()
        let segmentsField = viewportElement(app, "parameterField-segments")
        XCTAssertTrue(segmentsField.waitForExistence(timeout: 5))
        try setField(app, segmentsField, to: segments)
        let ringsField = viewportElement(app, "parameterField-rings")
        XCTAssertTrue(ringsField.waitForExistence(timeout: 5))
        try setField(app, ringsField, to: rings)
        // Commit through the node graph (see the radius test): a viewport
        // click here could pick a polygon and paint the Selection before
        // the baseline pick below.
        box1.click()
        RunLoop.main.run(until: Date().addingTimeInterval(2.0))
        XCTAssertEqual(segmentsField.value as? String, segments)
        XCTAssertEqual(ringsField.value as? String, rings)
    }

    /// Drives the sphere to quad-band resolution (segments 5, rings 3 =
    /// 20 triangles in 15 polygons: 10 fans + 5 quad pairs) so the
    /// equatorial tap below lands inside the quad band.
    private func driveQuadBandResolution(_ app: XCUIApplication) throws {
        try driveResolution(app, segments: "5", rings: "3")
    }

    /// Drives the sphere to minimum resolution (segments 5, rings 2 = 10
    /// fan triangles = 10 polygons 1:1) so one picked polygon is
    /// window-measurable.
    private func driveMinResolution(_ app: XCUIApplication) throws {
        try driveResolution(app, segments: "5", rings: "2")
    }

    /// Taps one polygon off the camera-facing seam and returns the orange
    /// fraction after the pick settles. See the radius test for why the
    /// tap sits at NDC x=+0.15 instead of the pane center.
    private func pickOffSeamPolygon(_ app: XCUIApplication, in windowFrame: CGRect) throws -> Double {
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
    private func createSphere(_ app: XCUIApplication) {
        let pane = viewportElement(app, "nodeGraphPane")
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
