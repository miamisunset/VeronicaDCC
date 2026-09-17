import AppKit
import MetalKit

/// `MTKView` subclass translating AppKit gestures into `ViewportFeature`
/// navigation actions (issue #41).
///
/// Thin by design: each handler converts its `NSEvent` into `ViewportGestureMap`
/// inputs (button, modifiers, deltas, cursor NDC) and forwards the mapped
/// action via `onAction`. All gesture→op decisions live in the map, covered
/// by unit tests without AppKit events.
///
/// Map: LMB-drag orbits, Command+LMB and middle-drag pan, Option+LMB and
/// right-drag dolly, non-precise wheel dollies, bare `F` frames all. Precise
/// (trackpad) scroll is ignored here — it belongs to #42.
final class ViewportNavView: MTKView {
    /// Forwards mapped navigation actions (wired to `store.send`).
    var onAction: ((ViewportFeature.Action) -> Void)?
    /// Coarse op the in-flight drag drives, fixed at mouse-down.
    private var dragKind: ViewportDragKind?
    /// Last drag location in view space, for per-event pixel deltas.
    private var lastPoint: NSPoint?

    /// Click-to-focus so bare `F` reaches `keyDown` after one click.
    override var acceptsFirstResponder: Bool { true }
    /// App-wide key tap for bare `F` (issue #46). SwiftUI's focus system
    /// can move first-responder status off this view after the click that
    /// claimed it, so `keyDown` alone never fires in the live app even
    /// though the click lands here. The monitor sees every app key-down
    /// regardless of focus; `consumeKeyEvent` scopes it back to our window
    /// and away from text inputs.
    private var keyMonitor: Any?

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        if window != nil, keyMonitor == nil {
            keyMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
                guard let self, self.consumeKeyEvent(event) else { return event }
                return nil
            }
        } else if window == nil, let monitor = keyMonitor {
            NSEvent.removeMonitor(monitor)
            keyMonitor = nil
        }
    }

    override func mouseDown(with event: NSEvent) {
        // Clicking a bare NSView does not claim focus on its own; without
        // this the `F` shortcut would need a second Tab/click.
        _ = window?.makeFirstResponder(self)
        beginDrag(event, button: .left)
    }

    override func mouseDragged(with event: NSEvent) {
        continueDrag(event)
    }

    override func mouseUp(with event: NSEvent) {
        _ = event
        endDrag()
    }

    override func rightMouseDown(with event: NSEvent) {
        beginDrag(event, button: .right)
    }

    override func rightMouseDragged(with event: NSEvent) {
        continueDrag(event)
    }

    override func rightMouseUp(with event: NSEvent) {
        _ = event
        endDrag()
    }

    override func otherMouseDown(with event: NSEvent) {
        beginDrag(event, button: .middle)
    }

    override func otherMouseDragged(with event: NSEvent) {
        continueDrag(event)
    }

    override func otherMouseUp(with event: NSEvent) {
        _ = event
        endDrag()
    }

    override func scrollWheel(with event: NSEvent) {
        // Precise deltas mean a trackpad: #42 owns precision zoom/pinch, so
        // only discrete mouse wheels dolly here.
        guard !event.hasPreciseScrollingDeltas else { return }
        let deltaY = Double(event.scrollingDeltaY)
        guard deltaY != 0 else { return }
        onAction?(ViewportGestureMap.wheelAction(deltaY: deltaY, cursor: cursorNDC(event)))
    }

    override func keyDown(with event: NSEvent) {
        if !consumeKeyEvent(event) {
            super.keyDown(with: event)
        }
    }

    /// Single decision point for bare-`F`-frames-all, shared by `keyDown`
    /// and the app-wide monitor. Returns whether the event was consumed.
    ///
    /// Scope guards apply only when hosted in a live window (unit tests
    /// drive windowless views): a foreign `event.window` is ignored, and a
    /// text input holding first responder keeps the keystroke — typing `f`
    /// in a rename field must never reframe the viewport.
    func consumeKeyEvent(_ event: NSEvent) -> Bool {
        guard event.type == .keyDown else { return false }
        if let host = window {
            if let target = event.window, target != host {
                return false
            }
            if host.firstResponder is NSText || host.firstResponder is NSTextField {
                return false
            }
        }
        let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
        guard let action = ViewportGestureMap.frameAllKey(
            characters: event.charactersIgnoringModifiers?.lowercased(),
            command: flags.contains(.command),
            control: flags.contains(.control),
            option: flags.contains(.option),
            shift: flags.contains(.shift)
        ) else {
            return false
        }
        onAction?(action)
        return true
    }

    /// Fix the drag's coarse op from button + modifiers; deltas accumulate
    /// from here in `continueDrag`.
    private func beginDrag(_ event: NSEvent, button: ViewportMouseButton) {
        let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
        dragKind = ViewportGestureMap.dragKind(
            button: button,
            command: flags.contains(.command),
            option: flags.contains(.option)
        )
        lastPoint = convert(event.locationInWindow, from: nil)
    }

    /// Map one drag step to its action from the view-space pixel delta.
    private func continueDrag(_ event: NSEvent) {
        guard let kind = dragKind, let last = lastPoint else { return }
        let point = convert(event.locationInWindow, from: nil)
        let dx = Double(point.x - last.x)
        let dy = Double(point.y - last.y)
        lastPoint = point
        guard dx != 0 || dy != 0 else { return }
        onAction?(ViewportGestureMap.dragAction(kind: kind, dx: dx, dy: dy, cursor: cursorNDC(event)))
    }

    /// Release the in-flight drag (mouse-up anywhere ends the gesture).
    private func endDrag() {
        dragKind = nil
        lastPoint = nil
    }

    /// Cursor NDC for `event`'s location, defaulting to center when the view
    /// is empty (a zero-size view cannot host a meaningful cursor).
    private func cursorNDC(_ event: NSEvent) -> CursorNDC {
        let point = convert(event.locationInWindow, from: nil)
        return ViewportGestureMap.cursorNDC(point: point, size: bounds.size)
            ?? CursorNDC(x: 0, y: 0)
    }
}
