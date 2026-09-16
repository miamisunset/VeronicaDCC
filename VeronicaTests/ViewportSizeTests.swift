import CoreGraphics
import Foundation
import Testing

@testable import Veronica

/// Pure size-intent contract: backing pixels cross FFI only on real resizes.
struct ViewportSizeTests {
    @Test func firstLayoutAlwaysSendsBackingPixels() throws {
        let intent = try #require(viewportSizeIntent(
            bounds: CGSize(width: 512, height: 320),
            scale: 2,
            lastSent: nil
        ))
        #expect(intent.width == 1024)
        #expect(intent.height == 640)
    }

    @Test func subHysteresisJitterStaysQuiet() {
        let lastSent = (width: UInt32(1024), height: UInt32(640))
        #expect(viewportSizeIntent(
            bounds: CGSize(width: 512.4, height: 320.4),
            scale: 2,
            lastSent: lastSent
        ) == nil)
    }

    @Test func hysteresisBoundarySends() throws {
        let lastSent = (width: UInt32(1024), height: UInt32(640))
        let intent = try #require(viewportSizeIntent(
            bounds: CGSize(width: 513, height: 320),
            scale: 2,
            lastSent: lastSent
        ))
        #expect(intent.width == 1026)
    }

    @Test func emptyOrHiddenViewsSendNothing() {
        #expect(viewportSizeIntent(bounds: .zero, scale: 2, lastSent: nil) == nil)
        #expect(viewportSizeIntent(
            bounds: CGSize(width: 512, height: 320),
            scale: 0,
            lastSent: nil
        ) == nil)
    }

    @Test func subPixelLayoutsSendNothing() {
        // A 0.1pt pane mid-swap rounds to zero backing pixels — send nothing
        // rather than the zero extent Rust would reject as InvalidArgument.
        #expect(viewportSizeIntent(
            bounds: CGSize(width: 0.1, height: 320),
            scale: 2,
            lastSent: nil
        ) == nil)
        #expect(viewportDrawableSize(bounds: CGSize(width: 0.1, height: 320), scale: 2) == .zero)
    }

    @Test func intentClampsToLongEdgeCap() throws {
        // Backing 8000x6000 scales proportionally to 2048x1536 — per-axis
        // clamping would distort aspect to 2048x2048.
        let intent = try #require(viewportSizeIntent(
            bounds: CGSize(width: 4000, height: 3000),
            scale: 2,
            lastSent: nil
        ))
        #expect(intent.width == viewportMaxEdge)
        #expect(intent.height == 1536)
    }

    @Test func fractionalScaleRoundsToPixels() throws {
        let intent = try #require(viewportSizeIntent(
            bounds: CGSize(width: 300, height: 200),
            scale: 1.5,
            lastSent: nil
        ))
        #expect(intent.width == 450)
        #expect(intent.height == 300)
    }
}
