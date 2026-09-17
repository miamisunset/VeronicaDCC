import Foundation
import Testing

@testable import Veronica

/// Gesture→action map contract (issue #41): button+modifiers select the
/// coarse op, deltas scale to FFI units, and bare `F` frames all. Pure
/// `ViewportGestureMap` coverage without AppKit events.
struct ViewportGestureMapTests {
    @Test("drag kind maps button plus modifiers", arguments: [
        (ViewportMouseButton.left, false, false, ViewportDragKind.orbit),
        (ViewportMouseButton.left, true, false, ViewportDragKind.pan),
        (ViewportMouseButton.left, false, true, ViewportDragKind.dolly),
        (ViewportMouseButton.left, true, true, ViewportDragKind.pan),
        (ViewportMouseButton.right, false, false, ViewportDragKind.dolly),
        (ViewportMouseButton.right, true, true, ViewportDragKind.dolly),
        (ViewportMouseButton.middle, false, false, ViewportDragKind.pan),
        (ViewportMouseButton.middle, true, true, ViewportDragKind.pan)
    ])
    func dragKind(
        button: ViewportMouseButton,
        command: Bool,
        option: Bool,
        expected: ViewportDragKind
    ) {
        #expect(ViewportGestureMap.dragKind(button: button, command: command, option: option) == expected)
    }

    @Test func orbitDragPassesPixelsThrough() {
        let cursor = CursorNDC(x: 0.25, y: -0.5)
        #expect(
            ViewportGestureMap.dragAction(kind: .orbit, dx: 12, dy: -7, cursor: cursor)
                == .orbitDelta(dx: 12, dy: -7)
        )
    }

    @Test func panDragPassesPixelsThrough() {
        let cursor = CursorNDC(x: 0, y: 0)
        #expect(
            ViewportGestureMap.dragAction(kind: .pan, dx: -3, dy: 9, cursor: cursor)
                == .panDelta(dx: -3, dy: 9)
        )
    }

    @Test func dollyDragScalesVerticalPixelsToLogFactor() {
        let cursor = CursorNDC(x: 0.5, y: 0.5)
        // Dragging up (positive dy) zooms in (positive factor).
        let action = ViewportGestureMap.dragAction(kind: .dolly, dx: 100, dy: 20, cursor: cursor)
        #expect(action == .dolly(logFactor: 20 * ViewportGestureMap.dragDollyScale, cursor: cursor))
        // Horizontal motion never dollies.
        #expect(
            ViewportGestureMap.dragAction(kind: .dolly, dx: 100, dy: 0, cursor: cursor)
                == .dolly(logFactor: 0, cursor: cursor)
        )
    }

    @Test func wheelScalesDeltaYToLogFactor() {
        let cursor = CursorNDC(x: -0.25, y: 0.75)
        #expect(
            ViewportGestureMap.wheelAction(deltaY: 10, cursor: cursor)
                == .dolly(logFactor: 10 * ViewportGestureMap.wheelDollyScale, cursor: cursor)
        )
        #expect(
            ViewportGestureMap.wheelAction(deltaY: -4, cursor: cursor)
                == .dolly(logFactor: -4 * ViewportGestureMap.wheelDollyScale, cursor: cursor)
        )
    }

    @Test("cursor NDC maps view-space points", arguments: [
        (CGPoint(x: 0, y: 0), CursorNDC(x: -1, y: -1)),
        (CGPoint(x: 400, y: 300), CursorNDC(x: 1, y: 1)),
        (CGPoint(x: 200, y: 150), CursorNDC(x: 0, y: 0)),
        (CGPoint(x: 100, y: 225), CursorNDC(x: -0.5, y: 0.5))
    ])
    func cursorNDC(point: CGPoint, expected: CursorNDC) {
        let size = CGSize(width: 400, height: 300)
        #expect(ViewportGestureMap.cursorNDC(point: point, size: size) == expected)
    }

    @Test func cursorNDCRejectsEmptyViews() {
        #expect(
            ViewportGestureMap.cursorNDC(
                point: CGPoint(x: 5, y: 5),
                size: CGSize(width: 0, height: 300)
            ) == nil
        )
        #expect(
            ViewportGestureMap.cursorNDC(
                point: CGPoint(x: 5, y: 5),
                size: CGSize(width: 400, height: 0)
            ) == nil
        )
    }

    @Test func bareFReturnsFrameAll() {
        #expect(
            ViewportGestureMap.frameAllKey(
                characters: "f",
                command: false,
                control: false,
                option: false,
                shift: false
            ) == .frameAll
        )
    }

    @Test("frame-all key rejects modifiers and other keys", arguments: [
        (nil as String?, false, false, false, false),
        ("g", false, false, false, false),
        ("f", true, false, false, false),
        ("f", false, true, false, false),
        ("f", false, false, true, false),
        ("f", false, false, false, true),
        ("F", false, false, false, false)
    ])
    func frameAllKeyRejects(
        characters: String?,
        command: Bool,
        control: Bool,
        option: Bool,
        shift: Bool
    ) {
        #expect(
            ViewportGestureMap.frameAllKey(
                characters: characters,
                command: command,
                control: control,
                option: option,
                shift: shift
            ) == nil
        )
    }

    @Test("trackpad drag kind pans, orbits with Option", arguments: [
        (false, ViewportDragKind.pan),
        (true, ViewportDragKind.orbit)
    ])
    func trackpadDragKind(option: Bool, expected: ViewportDragKind) {
        #expect(ViewportGestureMap.trackpadDragKind(option: option) == expected)
    }

    @Test func trackpadDragPansByFingerMotionPixels() {
        let cursor = CursorNDC(x: 0, y: 0)
        // Plain two-finger-drag ≡ Command+left-drag pan.
        #expect(
            ViewportGestureMap.trackpadDragAction(dx: 8, dy: -5, option: false, cursor: cursor)
                == .panDelta(dx: 8, dy: -5)
        )
    }

    @Test func trackpadDragOrbitsWithOption() {
        let cursor = CursorNDC(x: 0.1, y: 0.2)
        // Option+two-finger-drag ≡ plain left-drag orbit.
        #expect(
            ViewportGestureMap.trackpadDragAction(dx: 8, dy: -5, option: true, cursor: cursor)
                == .orbitDelta(dx: 8, dy: -5)
        )
    }

    @Test func pinchScalesMagnificationToLogFactor() {
        let cursor = CursorNDC(x: -0.5, y: 0.25)
        // Spreading fingers (positive magnification) zooms in.
        #expect(
            ViewportGestureMap.pinchAction(magnification: 0.1, cursor: cursor)
                == .dolly(logFactor: 0.1 * ViewportGestureMap.pinchDollyScale, cursor: cursor)
        )
        // Pinching (negative magnification) zooms out.
        #expect(
            ViewportGestureMap.pinchAction(magnification: -0.2, cursor: cursor)
                == .dolly(logFactor: -0.2 * ViewportGestureMap.pinchDollyScale, cursor: cursor)
        )
    }

    @Test func pinchScaleIsPositiveAndSeparateFromWheelScale() {
        // Magnification is unitless; wheel deltas are line units — the two
        // scales must not be conflated.
        #expect(ViewportGestureMap.pinchDollyScale > 0)
        #expect(ViewportGestureMap.pinchDollyScale != ViewportGestureMap.wheelDollyScale)
    }
}
