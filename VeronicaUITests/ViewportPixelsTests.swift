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
}
