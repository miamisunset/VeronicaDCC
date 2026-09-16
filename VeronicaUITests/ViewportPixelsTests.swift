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
}
