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
/// viewport mouse navigation (issue #41).
///
/// AppKit-free by design: the nav view translates `NSEvent` into these
/// inputs, so the whole map is unit-testable without events. The nav view
/// stays thin (translate → map → `store.send`).
nonisolated enum ViewportGestureMap {
    /// Drag pixels → dolly log-factor scale. Dragging up (positive view-space
    /// dy) zooms in, matching the FFI contract (positive factor zooms in).
    static let dragDollyScale = 0.005
    /// Non-precise wheel delta → dolly log-factor scale. Scrolling up
    /// (positive `scrollingDeltaY`) zooms in.
    static let wheelDollyScale = 0.002

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
    /// never reaches here — it belongs to #42 and the nav view ignores it.
    static func wheelAction(deltaY: Double, cursor: CursorNDC) -> ViewportFeature.Action {
        .dolly(logFactor: deltaY * wheelDollyScale, cursor: cursor)
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
