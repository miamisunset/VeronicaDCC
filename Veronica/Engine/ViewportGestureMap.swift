import Foundation

/// Cursor position in normalized device coordinates (x/y in [-1, 1], y-up).
///
/// Carried on `.dolly` so Rust can zoom toward the pointer. Pure value type
/// shared by the gesture map, the nav view, and the feature action.
nonisolated struct CursorNDC: Equatable, Sendable {
    /// Horizontal position: -1 at the left edge, 1 at the right.
    var x: Double
    /// Vertical position: -1 at the bottom edge, 1 at the top.
    var y: Double
}

/// Mouse button that started a viewport drag.
nonisolated enum ViewportMouseButton: Int, Equatable, Sendable {
    /// Left button (button 0).
    case left = 0
    /// Right button (button 1).
    case right = 1
    /// Middle or any other button (button 2+).
    case middle = 2
}

/// Coarse camera op a viewport drag drives.
nonisolated enum ViewportDragKind: Equatable, Sendable {
    /// Turntable orbit around the pivot.
    case orbit
    /// Rigid pan of camera and pivot.
    case pan
    /// Dolly toward the cursor.
    case dolly
}

/// Pure (button, modifier, delta) → `ViewportFeature.Action` mapping for
/// viewport mouse navigation (issue #41) plus trackpad gestures (#42).
///
/// AppKit-free by design: the nav view translates `NSEvent` into these
/// inputs, so the whole map is unit-testable without events. The nav view
/// stays thin (translate → map → `store.send`).
///
/// Trackpad semantics (#42): two-finger-drag pans, Option+two-finger-drag
/// orbits, pinch dollies. Momentum scroll (fingers lifted) is ignored — the
/// camera stops dead instead of drifting. A two-finger-drag step is defined
/// to equal the matching mouse drag (plain ≡ Command+left-drag pan,
/// Option ≡ plain left-drag orbit), so finger motion and cursor motion
/// agree.
nonisolated enum ViewportGestureMap {
    /// Drag pixels → dolly log-factor scale. Dragging up (positive view-space
    /// dy) zooms in, matching the FFI contract (positive factor zooms in).
    static let dragDollyScale = 0.005
    /// Non-precise wheel delta → dolly log-factor scale. Scrolling up
    /// (positive `scrollingDeltaY`) zooms in.
    static let wheelDollyScale = 0.002
    /// Pinch magnification → dolly log-factor scale. Magnification is already
    /// a fractional zoom and `ln(1 + m) ≈ m`, so 1:1 keeps pinch feel matched
    /// to the wheel (positive magnification zooms in). Deliberately separate
    /// from `wheelDollyScale`: magnification is unitless, wheel deltas are
    /// line units.
    static let pinchDollyScale = 1.0
    /// Press-release slop distinguishing a tap from a drag (issue #59):
    /// AppKit delivers no drag events for a clean click, but a press that
    /// wanders a pixel or two before release is still a tap — only motion
    /// beyond this radius (view-space pixels) commits to navigation.
    static let tapThresholdPixels = 4.0

    /// Coarse op for a drag starting with `button` and modifiers.
    ///
    /// Left drags orbit, modified to pan (Command) or dolly (Option);
    /// right drags dolly; middle (or any other) drags pan.
    static func dragKind(button: ViewportMouseButton, command: Bool, option: Bool) -> ViewportDragKind {
        switch button {
        case .middle:
            return .pan
        case .right:
            return .dolly
        case .left:
            if command {
                return .pan
            }
            if option {
                return .dolly
            }
            return .orbit
        }
    }

    /// Action for one drag step of `kind` with view-space pixel deltas.
    static func dragAction(
        kind: ViewportDragKind,
        dx: Double,
        dy: Double,
        cursor: CursorNDC
    ) -> ViewportFeature.Action {
        switch kind {
        case .orbit:
            return .orbitDelta(dx: dx, dy: dy)
        case .pan:
            return .panDelta(dx: dx, dy: dy)
        case .dolly:
            return .dolly(logFactor: dy * dragDollyScale, cursor: cursor)
        }
    }

    /// Action for one non-precise wheel tick. Precise (trackpad) scrolling
    /// never reaches here — it belongs to #42 (`trackpadDragAction`).
    static func wheelAction(deltaY: Double, cursor: CursorNDC) -> ViewportFeature.Action {
        .dolly(logFactor: deltaY * wheelDollyScale, cursor: cursor)
    }

    /// Coarse op for a trackpad two-finger-drag step: pans, modified to
    /// orbit by Option.
    static func trackpadDragKind(option: Bool) -> ViewportDragKind {
        option ? .orbit : .pan
    }

    /// Action for one trackpad two-finger-drag step with scroll-sense pixel
    /// deltas (`scrollingDeltaX/Y`). `inverted` is the event's
    /// `isDirectionInvertedFromDevice` (true under natural scrolling, whose
    /// scroll sense is inverted vs finger motion); the deltas are normalized
    /// to finger-motion pixels so the gesture equals the matching mouse drag.
    /// Flows through `dragAction`.
    static func trackpadDragAction(
        dx: Double,
        dy: Double,
        option: Bool,
        inverted: Bool,
        cursor: CursorNDC
    ) -> ViewportFeature.Action {
        let sign = inverted ? -1.0 : 1.0
        return dragAction(kind: trackpadDragKind(option: option), dx: sign * dx, dy: sign * dy, cursor: cursor)
    }

    /// Action for one pinch step: exponential dolly pivoted on the cursor,
    /// matching wheel feel (spreading fingers zooms in, pinching zooms out).
    static func pinchAction(magnification: Double, cursor: CursorNDC) -> ViewportFeature.Action {
        .dolly(logFactor: magnification * pinchDollyScale, cursor: cursor)
    }

    /// Tap intent for a press at `pressPoint` released at `releasePoint`
    /// (view-space points, y-up) in a view of `size`: `.tapAt` carrying the
    /// release NDC when the press never left tap slop, else `nil` (a drag —
    /// navigation owns it). `nil` for empty views (no meaningful NDC).
    ///
    /// Pure for testing; the nav view owns the press/drag bookkeeping and
    /// calls this only when no drag step has fired.
    static func tapAction(pressPoint: CGPoint, releasePoint: CGPoint, size: CGSize) -> ViewportFeature.Action? {
        guard size.width > 0, size.height > 0 else { return nil }
        let distance = hypot(releasePoint.x - pressPoint.x, releasePoint.y - pressPoint.y)
        guard distance <= tapThresholdPixels else { return nil }
        guard let cursor = cursorNDC(point: releasePoint, size: size) else { return nil }
        return .tapAt(cursor: cursor)
    }

    /// Cursor NDC for `point` (view-space, y-up) in a view of `size`, or
    /// `nil` for empty views (callers fall back to center).
    static func cursorNDC(point: CGPoint, size: CGSize) -> CursorNDC? {
        guard size.width > 0, size.height > 0 else { return nil }
        return CursorNDC(
            x: 2 * point.x / size.width - 1,
            y: 2 * point.y / size.height - 1
        )
    }

    /// `.frameAll` when `characters` is bare `f` (no modifiers), else `nil`.
    ///
    /// `characters` must already be lowercased (so Caps Lock still frames);
    /// Shift counts as a modifier and suppresses the shortcut.
    static func frameAllKey(
        characters: String?,
        command: Bool,
        control: Bool,
        option: Bool,
        shift: Bool
    ) -> ViewportFeature.Action? {
        guard characters == "f", !command, !control, !option, !shift else { return nil }
        return .frameAll
    }
}
