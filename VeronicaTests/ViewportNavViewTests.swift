import AppKit
import MetalKit
import Synchronization
import Testing

@testable import Veronica

/// Thin-layer contract for `ViewportNavView` (issue #41): synthesized
/// `NSEvent`s translate to the mapped `ViewportFeature.Action` via
/// `ViewportGestureMap`. No display link or engine needed.
@MainActor
struct ViewportNavViewTests {
    /// Thread-safe action sink: the nav view's `onAction` is escaping and
    /// nonisolated, so the recorder must be `Sendable` with `nonisolated`
    /// members (a `Mutex` alone is move-only and cannot cross the closure).
    private final class ActionRecorder: Sendable {
        private let mutex = Mutex<[ViewportFeature.Action]>([])
        nonisolated func append(_ action: ViewportFeature.Action) {
            mutex.withLock { $0.append(action) }
        }
        nonisolated var actions: [ViewportFeature.Action] {
            mutex.withLock { $0 }
        }
    }

    /// View plus its recorded actions.
    private func makeView() -> (ViewportNavView, ActionRecorder) {
        let recorded = ActionRecorder()
        let nav = ViewportNavView()
        nav.onAction = { action in recorded.append(action) }
        return (nav, recorded)
    }

    @Test func bareFKeyFramesAll() {
        let (nav, recorded) = makeView()
        nav.keyDown(with: keyEvent(characters: "f", modifiers: []))
        let actions = recorded.actions
        #expect(actions == [.frameAll])
    }

    @Test func modifiedFKeyIsIgnored() {
        let (nav, recorded) = makeView()
        nav.keyDown(with: keyEvent(characters: "f", modifiers: .command))
        #expect(recorded.actions.isEmpty)
    }

    @Test func otherKeysFallThroughWithoutAction() {
        let (nav, recorded) = makeView()
        nav.keyDown(with: keyEvent(characters: "g", modifiers: []))
        #expect(recorded.actions.isEmpty)
    }

    /// Host window for scope-guard tests (`contentRect`-only is not a valid
    /// `NSWindow` initializer).
    private func hostWindow() -> NSWindow {
        NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 400, height: 300),
            styleMask: [.titled],
            backing: .buffered,
            defer: false
        )
    }

    @Test func fKeyConsumesInLiveWindowWithoutTextFocus() {
        // Hosted in a window (no text input focused), bare `F` fires.
        let (nav, recorded) = makeView()
        hostWindow().contentView?.addSubview(nav)
        #expect(nav.consumeKeyEvent(keyEvent(characters: "f", modifiers: [])))
        #expect(recorded.actions == [.frameAll])
    }

    @Test func fKeyNotConsumedWhileEditingText() {
        // Typing `f` in a rename field must never reframe the viewport.
        let (nav, recorded) = makeView()
        let window = hostWindow()
        window.contentView?.addSubview(nav)
        let field = NSTextView(frame: NSRect(x: 0, y: 0, width: 100, height: 20))
        window.contentView?.addSubview(field)
        window.makeFirstResponder(field)
        #expect(!nav.consumeKeyEvent(keyEvent(characters: "f", modifiers: [])))
        #expect(recorded.actions.isEmpty)
    }

    @Test func fKeyNotConsumedForForeignWindow() {
        // The app-wide monitor sees every window's keys; only ours consume.
        let (nav, recorded) = makeView()
        hostWindow().contentView?.addSubview(nav)
        let other = hostWindow()
        let foreign = NSEvent.keyEvent(
            with: .keyDown,
            location: NSPoint.zero,
            modifierFlags: [],
            timestamp: 0,
            windowNumber: other.windowNumber,
            context: nil,
            characters: "f",
            charactersIgnoringModifiers: "f",
            isARepeat: false,
            keyCode: 3
        ).unsafelyUnwrapped
        #expect(!nav.consumeKeyEvent(foreign))
        #expect(recorded.actions.isEmpty)
    }

    @Test func leftDragOrbitsByPixelDelta() {
        let (nav, recorded) = makeView()
        nav.mouseDown(with: mouseEvent(type: .leftMouseDown, button: 0, point: NSPoint(x: 100, y: 100)))
        nav.mouseDragged(with: mouseEvent(type: .leftMouseDragged, button: 0, point: NSPoint(x: 112, y: 93)))
        nav.mouseUp(with: mouseEvent(type: .leftMouseUp, button: 0, point: NSPoint(x: 112, y: 93)))
        let actions = recorded.actions
        #expect(actions == [.orbitDelta(dx: 12, dy: -7)])
    }

    @Test func commandLeftDragPans() {
        let (nav, recorded) = makeView()
        var down = mouseEvent(type: .leftMouseDown, button: 0, point: NSPoint(x: 50, y: 50))
        down = withModifiers(down, flags: .command)
        nav.mouseDown(with: down)
        nav.mouseDragged(with: mouseEvent(type: .leftMouseDragged, button: 0, point: NSPoint(x: 60, y: 55)))
        let actions = recorded.actions
        #expect(actions == [.panDelta(dx: 10, dy: 5)])
    }

    @Test func rightDragDolliesTowardCursor() {
        let (nav, recorded) = makeView()
        nav.frame = NSRect(x: 0, y: 0, width: 400, height: 300)
        nav.rightMouseDown(with: mouseEvent(type: .rightMouseDown, button: 1, point: NSPoint(x: 200, y: 150)))
        nav.rightMouseDragged(
            with: mouseEvent(type: .rightMouseDragged, button: 1, point: NSPoint(x: 200, y: 170))
        )
        let actions = recorded.actions
        #expect(actions.count == 1)
        guard case let .dolly(logFactor, cursor) = actions.first else {
            Issue.record("expected a dolly action")
            return
        }
        #expect(logFactor == 20 * ViewportGestureMap.dragDollyScale)
        #expect(cursor == CursorNDC(x: 0, y: 2 * 170 / 300 - 1))
    }

    @Test func mouseWheelDollies() {
        let (nav, recorded) = makeView()
        nav.frame = NSRect(x: 0, y: 0, width: 400, height: 300)
        nav.scrollWheel(with: wheelEvent(deltaY: 3, precise: false))
        let actions = recorded.actions
        #expect(actions.count == 1)
        guard case let .dolly(logFactor, _) = actions.first else {
            Issue.record("expected a dolly action")
            return
        }
        #expect(logFactor == 3 * ViewportGestureMap.wheelDollyScale)
    }

    @Test func preciseTrackpadScrollIsIgnored() {
        let (nav, recorded) = makeView()
        // Precise (trackpad) scrolling belongs to #42, never dollies here.
        nav.scrollWheel(with: wheelEvent(deltaY: 3, precise: true))
        #expect(recorded.actions.isEmpty)
    }

    // MARK: - Synthesized events

    /// Key event carrying `characters` with `modifiers`.
    private func keyEvent(characters: String, modifiers: NSEvent.ModifierFlags) -> NSEvent {
        // The key code is irrelevant: the view reads only
        // `charactersIgnoringModifiers` plus the modifier flags.
        NSEvent.keyEvent(
            with: .keyDown,
            location: NSPoint.zero,
            modifierFlags: modifiers,
            timestamp: 0,
            windowNumber: 0,
            context: nil,
            characters: characters,
            charactersIgnoringModifiers: characters,
            isARepeat: false,
            keyCode: 3
        ).unsafelyUnwrapped
    }

    /// Mouse button event at `point` (view space, y-up).
    private func mouseEvent(type: NSEvent.EventType, button: Int, point: NSPoint) -> NSEvent {
        NSEvent.mouseEvent(
            with: type,
            location: point,
            modifierFlags: [],
            timestamp: 0,
            windowNumber: 0,
            context: nil,
            eventNumber: 0,
            clickCount: 1,
            pressure: 1
        ).unsafelyUnwrapped
    }

    /// Copy of `event` with `flags` set (for modifier+button combinations).
    private func withModifiers(_ event: NSEvent, flags: NSEvent.ModifierFlags) -> NSEvent {
        NSEvent.mouseEvent(
            with: event.type,
            location: event.locationInWindow,
            modifierFlags: flags,
            timestamp: event.timestamp,
            windowNumber: event.windowNumber,
            context: nil,
            eventNumber: event.eventNumber,
            clickCount: event.clickCount,
            pressure: event.pressure
        ).unsafelyUnwrapped
    }

    /// Scroll-wheel event; `precise` selects trackpad-style (pixel-unit,
    /// continuous) deltas over discrete mouse-wheel (line-unit) ones.
    private func wheelEvent(deltaY: Int32, precise: Bool) -> NSEvent {
        let cg = CGEvent(
            scrollWheelEvent2Source: nil,
            units: precise ? .pixel : .line,
            wheelCount: 1,
            wheel1: deltaY,
            wheel2: 0,
            wheel3: 0
        ).unsafelyUnwrapped
        return NSEvent(cgEvent: cg).unsafelyUnwrapped
    }
}
