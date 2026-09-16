import Testing

@testable import Veronica

/// Timing-meter contract (issue #32): fps smoothing math and the overlay
/// line format. Both are pure, so they pin exactly.
struct ViewportTimingTests {
    @Test func firstSampleSeedsTheAverage() {
        #expect(smoothedFrameRate(previous: nil, deltaSeconds: 1 / 120) == 120)
    }

    @Test func averageConvergesTowardTheInstantRate() {
        let smoothed = smoothedFrameRate(previous: 60, deltaSeconds: 1 / 120)
        #expect(smoothed > 60)
        #expect(smoothed < 120)
    }

    @Test func steadyRateStaysPut() {
        let smoothed = smoothedFrameRate(previous: 60, deltaSeconds: 1 / 60)
        #expect(abs(smoothed - 60) < 0.001)
    }

    @Test func nonPositiveIntervalKeepsPrevious() {
        #expect(smoothedFrameRate(previous: 60, deltaSeconds: 0) == 60)
        #expect(smoothedFrameRate(previous: 60, deltaSeconds: -1) == 60)
        #expect(smoothedFrameRate(previous: nil, deltaSeconds: 0) == 0)
    }

    @Test func millisecondsKeepOneDecimal() {
        #expect(milliseconds(8_300) == 8.3)
        #expect(milliseconds(0) == 0.0)
        #expect(milliseconds(16_666) == 16.7)
    }

    @Test func timingLineFormatsEveryStage() {
        #expect(timingLine(
            fps: 119.6,
            tickUs: 8_300,
            updateUs: 5_100,
            readbackUs: 2_900,
            uploadUs: 300,
            presentUs: 400
        ) == "120 fps · 8.3 ms tick · u 5.1 r 2.9 w 0.3 p 0.4")
    }

    @Test func timingLineStartsAtZero() {        #expect(timingLine(
            fps: 0,
            tickUs: 0,
            updateUs: 0,
            readbackUs: 0,
            uploadUs: 0,
            presentUs: 0
        ) == "0 fps · 0.0 ms tick · u 0.0 r 0.0 w 0.0 p 0.0")
    }

    @Test func frameLineShowsExtents() {
        #expect(frameLine(width: 2048, height: 853) == "frame 2048×853")
        #expect(frameLine(width: 0, height: 0) == "frame 0×0")
    }
}
