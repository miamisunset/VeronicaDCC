import CoreGraphics
import Foundation
import Testing

@testable import Veronica

/// Aspect-fit present contract: frames scale into the live-layout drawable
/// without distortion, letterboxing the remainder.
struct AspectFitTests {
    @Test func sameAspectFillsExactly() {
        let fit = aspectFitRect(
            frameWidth: 800,
            frameHeight: 600,
            drawableWidth: 800,
            drawableHeight: 600
        )
        #expect(fit == CGRect(x: 0, y: 0, width: 800, height: 600))
    }

    @Test func wideFrameIntoTallDrawableLetterboxes() {
        // 800x400 (2:1) into 400x400: full width, centered vertically.
        let fit = aspectFitRect(
            frameWidth: 800,
            frameHeight: 400,
            drawableWidth: 400,
            drawableHeight: 400
        )
        #expect(fit == CGRect(x: 0, y: 100, width: 400, height: 200))
    }

    @Test func tallFrameIntoWideDrawablePillarboxes() {
        // 400x800 (1:2) into 800x400: full height, centered horizontally.
        let fit = aspectFitRect(
            frameWidth: 400,
            frameHeight: 800,
            drawableWidth: 800,
            drawableHeight: 400
        )
        #expect(fit == CGRect(x: 300, y: 0, width: 200, height: 400))
    }

    @Test func smallFrameUpscalesToFit() {
        // Capped-small-frame case: 512x320 up into 1024x1024 fills width.
        let fit = aspectFitRect(
            frameWidth: 512,
            frameHeight: 320,
            drawableWidth: 1024,
            drawableHeight: 1024
        )
        #expect(fit == CGRect(x: 0, y: 192, width: 1024, height: 640))
    }

    @Test func zeroOrNegativeInputsYieldZero() {
        #expect(aspectFitRect(
            frameWidth: 0,
            frameHeight: 600,
            drawableWidth: 800,
            drawableHeight: 600
        ) == .zero)
        #expect(aspectFitRect(
            frameWidth: 800,
            frameHeight: -600,
            drawableWidth: 800,
            drawableHeight: 600
        ) == .zero)
        #expect(aspectFitRect(
            frameWidth: 800,
            frameHeight: 600,
            drawableWidth: 0,
            drawableHeight: 600
        ) == .zero)
        #expect(aspectFitRect(
            frameWidth: 800,
            frameHeight: 600,
            drawableWidth: 800,
            drawableHeight: -1
        ) == .zero)
    }

    @Test(
        arguments: [
            (frame: (800, 600), drawable: (800, 600)),
            (frame: (800, 400), drawable: (400, 400)),
            (frame: (400, 800), drawable: (800, 400)),
            (frame: (512, 320), drawable: (1024, 1024)),
            (frame: (2048, 1536), drawable: (1920, 1080)),
            (frame: (1920, 1080), drawable: (800, 1200))
        ]
    )
    func outputAspectMatchesFrameAspect(pair: ((Int, Int), (Int, Int))) {
        let fit = aspectFitRect(
            frameWidth: pair.0.0,
            frameHeight: pair.0.1,
            drawableWidth: pair.1.0,
            drawableHeight: pair.1.1
        )
        #expect(fit.width > 0)
        #expect(fit.height > 0)
        // Contained: never overflows the drawable.
        #expect(fit.width <= CGFloat(pair.1.0) + 1)
        #expect(fit.height <= CGFloat(pair.1.1) + 1)
        // Proportional: output aspect matches frame aspect within 1px
        // rounding (cross-multiplied to avoid division).
        let crossError = abs(
            fit.width * CGFloat(pair.0.1) - fit.height * CGFloat(pair.0.0)
        )
        #expect(crossError <= CGFloat(max(pair.0.0, pair.0.1)))
    }

    @Test func drawableSizeFollowsLiveLayoutUncapped() {
        // Live backing pixels, no cap: 4000x3000 @2x stays 8000x6000 here;
        // the 2048 long-edge cap applies to the Rust intent only.
        let drawable = viewportDrawableSize(
            bounds: CGSize(width: 4000, height: 3000),
            scale: 2
        )
        #expect(drawable == CGSize(width: 8000, height: 6000))
    }

    @Test func drawableSizeRejectsEmpty() {
        #expect(viewportDrawableSize(bounds: .zero, scale: 2) == .zero)
        #expect(viewportDrawableSize(bounds: CGSize(width: 512, height: 320), scale: 0) == .zero)
    }
}
