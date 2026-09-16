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
        let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
        if let action = ViewportGestureMap.frameAllKey(
            characters: event.charactersIgnoringModifiers?.lowercased(),
            command: flags.contains(.command),
            control: flags.contains(.control),
            option: flags.contains(.option),
            shift: flags.contains(.shift)
        ) {
            onAction?(action)
        } else {
            super.keyDown(with: event)
        }
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
