import CoreGraphics
import XCTest

/// Issue-#59 oracle: taps are Pick input, not navigation — tapping the
/// viewport moves no pixels (the engine seam is a no-op until T5/T6), while
/// a real drag still orbits.
///
/// Setup mirrors `CubeRecookLoopTests` (create a cube through the Geometry
/// submenu, frame it) so navigation has lit pixels to move: on an empty
/// scene even a true orbit moves nothing and the two gestures would be
/// indistinguishable. Oracle math: the tap half reuses the established
/// bright-pixel ratio tripwire (1.3x, static scene — only overlay text may
/// flicker) plus a pixel-diff pin near zero; the drag half asserts on
/// pixel-diff calibrated against the same run's tap noise, because bright
/// counts are rotation-stable (a 1.25-rad orbit moves edges and shading
/// while barely changing how many pixels are lit).
final class ViewportTapTests: XCTestCase {
    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    @MainActor
    func testTapMovesNoPixelsWhileDragOrbits() throws {
        let app = XCUIApplication()
        app.launchArguments = ["--vrn-reset-graph"]
        app.launch()

        let warmed = waitForViewportWarmUp(app)
        XCTAssertGreaterThanOrEqual(warmed, 120, "engine never warmed up")

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

        // Frame the new geometry and baseline the 1 m cube.
        let viewport = viewportElement(app, "viewportPane")
        XCTAssertTrue(viewport.waitForExistence(timeout: 5))
        viewport.click()
        app.typeKey("f", modifierFlags: [])
        RunLoop.main.run(until: Date().addingTimeInterval(1.0))
        let windowFrame = app.windows.firstMatch.frame
        let beforeShot = app.screenshot()
        let before = try XCTUnwrap(
            viewportBrightPixelFraction(of: beforeShot, in: windowFrame),
            "could not map the app window into the screenshot"
        )
        XCTAssertGreaterThan(before, 0.005, "cube never appeared in the viewport")

        // A plain click is now a tap: it must navigate nothing — neither the
        // bright count (established oracle) nor any actual pixel.
        viewport.click()
        RunLoop.main.run(until: Date().addingTimeInterval(1.0))
        let afterTapShot = app.screenshot()
        let afterTap = try XCTUnwrap(
            viewportBrightPixelFraction(of: afterTapShot, in: windowFrame),
            "could not map the app window into the screenshot"
        )
        XCTAssertLessThan(
            afterTap,
            max(before * 1.3, 0.005),
            "tap moved pixels: bright fraction \(before) -> \(afterTap)"
        )
        let tapDiff = try XCTUnwrap(
            viewportPixelDifferenceFraction(before: beforeShot, after: afterTapShot, in: windowFrame),
            "could not diff the tap screenshots"
        )
        XCTAssertLessThan(tapDiff, 0.002, "tap moved pixels: diff fraction \(tapDiff)")

        // A real drag still orbits: 250x120 px ≈ 1.25 x 0.6 rad of
        // turntable, which must move far more than overlay flicker.
        // `click(forDuration:thenDragTo:)` (mouse click-and-hold semantics),
        // not `press` (pressure semantics), so AppKit routes a real
        // mouseDown/dragged/up sequence to the nav view.
        let tick = viewportElement(app, "viewportTickLabel")
        XCTAssertTrue(tick.waitForExistence(timeout: 5))
        let tickBeforeDrag = tick.value as? String
        let start = viewport.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5))
        start.click(forDuration: 0.2, thenDragTo: start.withOffset(CGVector(dx: 250, dy: 120)))
        RunLoop.main.run(until: Date().addingTimeInterval(1.5))
        let afterDragShot = app.screenshot()
        XCTAssertNotEqual(
            tick.value as? String,
            tickBeforeDrag,
            "engine froze during the drag: tick stuck at \(tickBeforeDrag ?? "nil")"
        )
        // Bright counts are rotation-stable (a 1.25-rad orbit measured only
        // 7% here), so the drag half asserts on moved pixels, calibrated
        // against the same run's tap noise: the orbit must stand an order of
        // magnitude clear of it, with an absolute floor keeping the
        // assertion meaningful even when the tap diffs to zero.
        let dragDiff = try XCTUnwrap(
            viewportPixelDifferenceFraction(before: afterTapShot, after: afterDragShot, in: windowFrame),
            "could not diff the drag screenshots"
        )
        XCTAssertGreaterThan(
            dragDiff,
            max(0.005, tapDiff * 10),
            "drag moved no pixels: diff fraction \(dragDiff) against tap noise \(tapDiff)"
        )
    }
}
