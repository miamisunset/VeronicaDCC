import CoreGraphics
import XCTest

/// Slice-1 acceptance flow: create at click, dive, nested create, back,
/// keyboard move, inline rename, relaunch persistence, delete,
/// persist-empty.
///
/// Two entry points share one flow: the mock double (hermetic default) and
/// the real `libveronica.a` surface (end-to-end proof; requires a fresh
/// `cargo build -p veronica-ffi` before gates since Xcode does not track
/// the prebuilt archive).
///
/// Two environment notes: synthetic press-drags never reach SwiftUI gesture
/// recognizers in this harness (even native split-view dividers ignore
/// them), so moves go through arrow-key nudges — the same drag-commit path
/// real drags use. Single-click selection is likewise driven by double-click
/// here; the reducer selection contract is pinned by unit tests.
final class NodeGraphFlowTests: XCTestCase {
    /// Canvas point for the root container.
    private let rootOrigin = CGVector(dx: 120, dy: 100)
    /// Canvas point for the nested container.
    private let nestedOrigin = CGVector(dx: 140, dy: 110)

    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    /// Launch arguments for one app run: reset isolation plus the mock
    /// engine unless `mocked` is false (real `libveronica.a` surface).
    private func launchArguments(mocked: Bool, reset: Bool) -> [String] {
        var arguments: [String] = []
        if reset {
            arguments.append("--vrn-reset-graph")
        }
        if mocked {
            arguments.append("--vrn-mock-engine")
        }
        return arguments
    }

    @MainActor
    func testOperatorGraphSliceFlow() throws {
        // Hermetic default: the in-memory engine double.
        try runSliceFlow(mocked: true)
    }

    @MainActor
    func testOperatorGraphSliceFlowRealEngine() throws {
        // End-to-end proof against the real FFI surface.
        try runSliceFlow(mocked: false)
    }

    /// Slice-1 parameter editor: create, select, edit the name in the third
    /// pane, commit, and see the box follow (mock engine).
    ///
    /// Single-click selection is driven by double-click here (see the class
    /// docs); the click also dives, so the rename is verified back at root.
    @MainActor
    func testParameterEditorRenamesSelection() throws {
        let app = XCUIApplication()
        app.launchArguments = launchArguments(mocked: true, reset: true)
        app.launch()

        XCTAssertTrue(element(app, "nodeGraphPane").waitForExistence(timeout: 10))
        // No selection yet: the editor shows its empty state.
        XCTAssertTrue(element(app, "parameterEmptyState").waitForExistence(timeout: 5))

        try createContainer(app, at: rootOrigin)
        let box1 = element(app, "operatorBox-1")
        XCTAssertTrue(box1.waitForExistence(timeout: 5))

        // Double-click selects (and dives); the editor mirrors the selection.
        box1.doubleClick()
        let field = element(app, "parameterNameField")
        XCTAssertTrue(field.waitForExistence(timeout: 5))
        XCTAssertEqual(field.value as? String, "Container")

        // Editing the field and committing renames through setParameter.
        field.click()
        app.typeKey("a", modifierFlags: .command)
        app.typeText("Hero")
        app.typeKey(.return, modifierFlags: [])
        let committed = expectation(
            for: NSPredicate(format: "value == %@", "Hero"),
            evaluatedWith: field,
            handler: nil
        )
        wait(for: [committed], timeout: 5)

        // Back at root the box shows the new name.
        element(app, "breadcrumbRoot").click()
        XCTAssertTrue(box1.waitForExistence(timeout: 5))
        XCTAssertEqual(box1.label, "Hero")
    }

    /// Queries any descendant by accessibility identifier (fresh each call).
    private func element(_ app: XCUIApplication, _ identifier: String) -> XCUIElement {
        app.descendants(matching: .any)[identifier]
    }

    /// Spins until `condition` holds: toolbar-driven relayouts apply on the
    /// next runloop turn, so a bare click-then-assert races the render.
    /// Elements re-resolve on every access, so the condition always sees
    /// fresh frames.
    private func waitForLayout(
        _ app: XCUIApplication,
        _ condition: @escaping () -> Bool
    ) {
        let settled = expectation(
            for: NSPredicate(block: { _, _ in condition() }),
            evaluatedWith: app,
            handler: nil
        )
        wait(for: [settled], timeout: 5)
    }

    /// Frame of a pane element, re-resolved fresh on every call. Static so
    /// layout-poll closures capture no test-case state.
    private static func paneFrame(_ app: XCUIApplication, _ identifier: String) -> CGRect {
        app.descendants(matching: .any)[identifier].frame
    }

    /// Clicks a Panes menu item: the menu-bar path to the same explicit
    /// layout actions the toolbar buttons send.
    private func panesMenuItem(_ app: XCUIApplication, _ title: String) -> XCUIElement {
        app.menuBars.menuBarItems["Panes"].click()
        let item = app.menuItems[title]
        XCTAssertTrue(item.waitForExistence(timeout: 5))
        return item
    }

    /// Slice-2 rearrangement: swap Graph/Parameters order via the toolbar,
    /// stack them via the Panes menu, then restore the slice-1 default
    /// (mock engine).
    ///
    /// Order is read off pane frames (`nodeGraphPane` is the canvas leaf,
    /// `parameterEditorPane` the editor header — both move with their pane).
    @MainActor
    func testPaneRearrangementSwapsOrderAndStacks() throws {
        let app = XCUIApplication()
        app.launchArguments = launchArguments(mocked: true, reset: true)
        app.launch()

        XCTAssertTrue(element(app, "nodeGraphPane").waitForExistence(timeout: 10))
        XCTAssertTrue(element(app, "parameterEditorPane").waitForExistence(timeout: 5))

        // Slice-1 default: row with the graph left of the editor.
        XCTAssertLessThan(Self.paneFrame(app, "nodeGraphPane").minX, Self.paneFrame(app, "parameterEditorPane").minX)

        // Toolbar swap: the editor moves left of the graph; the empty state
        // survives (nothing is selected). Swap back restores the default.
        element(app, "swapPanesButton").click()
        waitForLayout(app) {
            Self.paneFrame(app, "parameterEditorPane").minX < Self.paneFrame(app, "nodeGraphPane").minX
        }
        XCTAssertTrue(element(app, "parameterEmptyState").exists)
        element(app, "swapPanesButton").click()
        waitForLayout(app) {
            Self.paneFrame(app, "nodeGraphPane").minX < Self.paneFrame(app, "parameterEditorPane").minX
        }

        // Panes menu stacks: a column with the graph above the editor, then
        // back to the side-by-side default.
        panesMenuItem(app, "Stack Graph and Parameters Vertically").click()
        waitForLayout(app) {
            Self.paneFrame(app, "nodeGraphPane").maxY < Self.paneFrame(app, "parameterEditorPane").minY
        }
        XCTAssertTrue(element(app, "parameterEmptyState").exists)
        panesMenuItem(app, "Place Graph and Parameters Side by Side").click()
        waitForLayout(app) {
            Self.paneFrame(app, "nodeGraphPane").minX < Self.paneFrame(app, "parameterEditorPane").minX
        }
        XCTAssertTrue(element(app, "parameterEmptyState").exists)

        // The toolbar orientation button drives the same switch: round-trip
        // it so both controls' wiring is pinned, ending on the default.
        element(app, "paneOrientationButton").click()
        waitForLayout(app) {
            Self.paneFrame(app, "nodeGraphPane").maxY < Self.paneFrame(app, "parameterEditorPane").minY
        }
        element(app, "paneOrientationButton").click()
        waitForLayout(app) {
            Self.paneFrame(app, "nodeGraphPane").minX < Self.paneFrame(app, "parameterEditorPane").minX
        }
        XCTAssertTrue(element(app, "parameterEmptyState").exists)
    }

    /// Canvas-absolute coordinate for a canvas-space point.
    ///
    /// The canvas carries the pane identifier (identifiers on SwiftUI
    /// containers override their descendants, so the pane id lives on the
    /// leaf canvas element instead of the outer stack).
    private func canvasPoint(
        _ app: XCUIApplication,
        _ point: CGVector
    ) -> XCUICoordinate {
        element(app, "nodeGraphPane")
            .coordinate(withNormalizedOffset: CGVector(dx: 0, dy: 0))
            .withOffset(point)
    }

    /// Right-clicks `point` and picks the registry add item.
    private func createContainer(_ app: XCUIApplication, at point: CGVector) throws {
        let target = canvasPoint(app, point)
        // The click moves the mouse first so hover-tracked creation lands
        // exactly on the point before the menu opens.
        target.click()
        target.rightClick()
        let addItem = app.menuItems["Add Container"]
        XCTAssertTrue(addItem.waitForExistence(timeout: 5))
        addItem.click()
    }

    /// The slice-1 acceptance flow, engine-agnostic.
    private func runSliceFlow(mocked: Bool) throws {
        let app = XCUIApplication()
        app.launchArguments = launchArguments(mocked: mocked, reset: true)
        app.launch()

        XCTAssertTrue(element(app, "nodeGraphPane").waitForExistence(timeout: 10))
        XCTAssertTrue(element(app, "nodeGraphEmptyHint").waitForExistence(timeout: 5))
        // The engine identity is visible: mock and real runs are never confused.
        XCTAssertEqual(element(app, "mockEngineBadge").exists, mocked)

        // Create at click: the box lands exactly on the hovered point.
        try createContainer(app, at: rootOrigin)
        let box1 = element(app, "operatorBox-1")
        XCTAssertTrue(box1.waitForExistence(timeout: 5))
        XCTAssertEqual(box1.label, "Container")
        XCTAssertEqual(box1.value as? String, "at 120, 100")

        // Double-click selects and dives into the container's network.
        box1.doubleClick()
        let segment = element(app, "breadcrumbSegment-0")
        XCTAssertTrue(segment.waitForExistence(timeout: 5))
        XCTAssertEqual(segment.label, "Container")
        XCTAssertTrue(element(app, "nodeGraphEmptyHint").waitForExistence(timeout: 5))
        XCTAssertFalse(box1.exists)

        // Nested create lands inside the container, not at root.
        try createContainer(app, at: nestedOrigin)
        let box2 = element(app, "operatorBox-2")
        XCTAssertTrue(box2.waitForExistence(timeout: 5))
        XCTAssertEqual(box2.label, "Container")

        // Breadcrumb jumps back to root: box 1 returns, box 2 hides.
        element(app, "breadcrumbRoot").click()
        XCTAssertTrue(box1.waitForExistence(timeout: 5))
        XCTAssertFalse(element(app, "operatorBox-2").exists)

        // Arrow keys nudge the selection through the drag-commit path.
        app.typeKey(.rightArrow, modifierFlags: [])
        app.typeKey(.downArrow, modifierFlags: [])
        let moved = expectation(
            for: NSPredicate(format: "value == %@", "at 130, 110"),
            evaluatedWith: box1,
            handler: nil
        )
        wait(for: [moved], timeout: 5)

        // Enter opens the inline rename overlay; typing commits on Enter.
        app.typeKey(.return, modifierFlags: [])
        let field = element(app, "operatorRenameField")
        XCTAssertTrue(field.waitForExistence(timeout: 5))
        field.click()
        app.typeKey("a", modifierFlags: .command)
        app.typeText("Hero")
        app.typeKey(.return, modifierFlags: [])
        let renamed = expectation(
            for: NSPredicate(format: "label == %@", "Hero"),
            evaluatedWith: box1,
            handler: nil
        )
        wait(for: [renamed], timeout: 5)

        // Relaunch without reset: autosave restores name and position.
        app.terminate()
        let relaunch = XCUIApplication()
        relaunch.launchArguments = launchArguments(mocked: mocked, reset: false)
        relaunch.launch()
        let reloaded = element(relaunch, "operatorBox-1")
        XCTAssertTrue(reloaded.waitForExistence(timeout: 10))
        XCTAssertEqual(reloaded.label, "Hero")
        XCTAssertEqual(reloaded.value as? String, "at 130, 110")

        // Diving shows the nested container survived the relaunch too.
        reloaded.doubleClick()
        XCTAssertTrue(element(relaunch, "breadcrumbSegment-0").waitForExistence(timeout: 5))
        XCTAssertTrue(element(relaunch, "operatorBox-2").waitForExistence(timeout: 5))

        // Delete key removes the selection, cascading the subtree. Still
        // dived in: box 1 is already hidden, so the nested box disappearing
        // plus the empty hint is the in-session proof (box 1's absence here
        // would pass vacuously).
        relaunch.typeKey(.delete, modifierFlags: [])
        let nestedGone = expectation(
            for: NSPredicate(format: "exists == false"),
            evaluatedWith: element(relaunch, "operatorBox-2"),
            handler: nil
        )
        wait(for: [nestedGone], timeout: 5)
        XCTAssertTrue(element(relaunch, "nodeGraphEmptyHint").waitForExistence(timeout: 5))

        // A final relaunch proves the empty graph persisted.
        relaunch.terminate()
        let empty = XCUIApplication()
        empty.launchArguments = launchArguments(mocked: mocked, reset: false)
        empty.launch()
        XCTAssertTrue(element(empty, "nodeGraphEmptyHint").waitForExistence(timeout: 10))
        XCTAssertFalse(element(empty, "operatorBox-1").exists)
    }
}
