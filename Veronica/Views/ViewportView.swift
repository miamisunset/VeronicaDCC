import ComposableArchitecture
import MetalKit
import SwiftUI

/// Hosts the Metal view that the render slice (ADR 0001) will publish
/// `IOSurface` frames into, and owns the display link that paces the loop.
///
/// The `CADisplayLink` is vended by the `MTKView` itself, so it tracks
/// whichever display the viewport is on and suspends off-display. It fires on
/// the main run loop; the TCA effect in `ViewportFeature` is what hops
/// off-MainActor to tick Rust. `dismantleNSView` invalidates the link,
/// breaking the link → coordinator retain cycle.
struct ViewportMetalHost: NSViewRepresentable {
    /// Invoked on the main thread, once per display refresh.
    var onFrame: () -> Void

    final class Coordinator {
        var onFrame: () -> Void
        var link: CADisplayLink?

        init(onFrame: @escaping () -> Void) {
            self.onFrame = onFrame
        }

        @objc func fire() {
            onFrame()
        }
    }

    func makeCoordinator() -> Coordinator {
        Coordinator(onFrame: onFrame)
    }

    func makeNSView(context: Context) -> MTKView {
        let view = MTKView()
        view.device = MTLCreateSystemDefaultDevice()
        // Inert until slice 2: the view never draws (`isPaused` below), so
        // this only documents the intended clear once IOSurface frames land.
        view.clearColor = MTLClearColor(red: 0.05, green: 0.06, blue: 0.09, alpha: 1)
        view.enableSetNeedsDisplay = false
        view.isPaused = true
        let link = view.displayLink(target: context.coordinator, selector: #selector(Coordinator.fire))
        link.add(to: .main, forMode: .common)
        context.coordinator.link = link
        return view
    }

    func updateNSView(_ nsView: MTKView, context: Context) {
        context.coordinator.onFrame = onFrame
    }

    static func dismantleNSView(_ nsView: MTKView, coordinator: Coordinator) {
        coordinator.link?.invalidate()
        coordinator.link = nil
    }
}

/// Left pane: the Bevy-driven viewport.
///
/// Each display refresh sends `.frame`; the reducer ticks Rust off-MainActor
/// and publishes stats back here.
struct ViewportView: View {
    @Bindable var store: StoreOf<ViewportFeature>

    var body: some View {
        ZStack(alignment: .bottomLeading) {
            ViewportMetalHost {
                store.send(.frame)
            }
            VStack(alignment: .leading, spacing: 2) {
                Text("tick \(store.tickCount)")
                    .monospacedDigit()
                Text("entities \(store.entityCount)")
                    .monospacedDigit()
            }
            .font(.caption)
            .foregroundStyle(.white.opacity(0.85))
            .padding(8)
            .accessibilityElement(children: .combine)
            .accessibilityLabel("Viewport stats")
        }
        .accessibilityIdentifier("viewportPane")
    }
}

#Preview {
    ViewportView(
        store: Store(initialState: ViewportFeature.State()) {
            ViewportFeature()
        }
    )
}
