import ComposableArchitecture
import IOSurface
import Metal
import MetalKit
import SwiftUI

/// Hosts the Metal view presenting Rust-published `IOSurface` frames, and
/// owns the display link pacing the loop.
///
/// Ownership: Rust owns the `IOSurface` (valid for the engine-context
/// lifetime; Swift never releases it). Each display refresh fires `onFrame`
/// on the main run loop; the TCA effect in `ViewportFeature` hops
/// off-MainActor to tick Rust and publish stats plus the frame handle.
/// `adopt(frame:)` wraps a new handle in an `MTLTexture` once and reuses
/// it; `draw(in:)` blits it into the drawable, so consecutive frames differ
/// because Scene state advanced. `dismantleNSView` invalidates the link,
/// breaking the link → coordinator retain cycle.
struct ViewportMetalHost: NSViewRepresentable {
    /// Invoked on the main thread, once per display refresh.
    var onFrame: () -> Void
    /// Latest published frame handle from `ViewportFeature.State`.
    var frame: VideoFrame?

    final class Coordinator: NSObject, MTKViewDelegate {
        var onFrame: () -> Void
        var link: CADisplayLink?
        private var device: MTLDevice?
        private var commandQueue: MTLCommandQueue?
        private var cachedAddress: UInt64?
        private var frameTexture: MTLTexture?

        init(onFrame: @escaping () -> Void) {
            self.onFrame = onFrame
        }

        @objc func fire() {
            onFrame()
        }

        func attach(device: MTLDevice?, queue: MTLCommandQueue?) {
            self.device = device
            commandQueue = queue
        }

        /// Wrap a newly published handle, skipping re-wraps of the live one.
        ///
        /// The surface is borrowed: the engine context owns it and Swift
        /// takes it unretained, so no retain/release crosses the boundary.
        func adopt(frame: VideoFrame?) {
            guard let frame, frame.surfaceAddress != cachedAddress else { return }
            guard let raw = UnsafeRawPointer(bitPattern: UInt(frame.surfaceAddress)),
                  let device
            else {
                return
            }
            let surface = Unmanaged<IOSurface>.fromOpaque(raw).takeUnretainedValue()
            let descriptor = MTLTextureDescriptor.texture2DDescriptor(
                pixelFormat: .bgra8Unorm,
                width: Int(frame.width),
                height: Int(frame.height),
                mipmapped: false
            )
            descriptor.usage = .shaderRead
            guard let texture = device.makeTexture(
                descriptor: descriptor,
                iosurface: surface,
                plane: 0
            ) else {
                return
            }
            cachedAddress = frame.surfaceAddress
            frameTexture = texture
        }

        func mtkView(_ view: MTKView, drawableSizeWillChange size: CGSize) {
            // Fixed-size frames (slice 2): no resize handling yet. Deliberately
            // empty — restoring `drawableSize` here re-triggers this callback
            // and deadlocks layout during pane swaps (see slice-2 diagnosis).
        }

        func draw(in view: MTKView) {
            guard let drawable = view.currentDrawable,
                let texture = frameTexture,
                let queue = commandQueue,
                let buffer = queue.makeCommandBuffer()
            else {
                return
            }
            guard drawable.texture.width == texture.width,
                drawable.texture.height == texture.height
            else {
                // Size drift (e.g. relayout racing the fixed drawable):
                // skip the frame loudly rather than presenting a black view
                // with no trace. Scaling blits belong to the resize slice.
                NSLog(
                    "Viewport: drawable %dx%d != frame %dx%d, skipping blit",
                    drawable.texture.width,
                    drawable.texture.height,
                    texture.width,
                    texture.height
                )
                return
            }
            // The encoder is created only after the size check: an early
            // return with a live un-ended encoder aborts under Metal
            // validation when the autorelease pool drains.
            guard let blit = buffer.makeBlitCommandEncoder() else { return }
            blit.copy(from: texture, to: drawable.texture)
            blit.endEncoding()
            buffer.present(drawable)
            buffer.commit()
        }
    }

    func makeCoordinator() -> Coordinator {
        Coordinator(onFrame: onFrame)
    }

    func makeNSView(context: Context) -> MTKView {
        let view = MTKView()
        let device = MTLCreateSystemDefaultDevice()
        view.device = device
        // The present path blits into the drawable, and blit writes are
        // illegal on framebuffer-only textures (Metal validation aborts:
        // "destinationTexture must not be a framebufferOnly texture").
        view.framebufferOnly = false
        view.clearColor = MTLClearColor(red: 0.05, green: 0.06, blue: 0.09, alpha: 1)
        view.enableSetNeedsDisplay = false
        view.isPaused = false
        view.delegate = context.coordinator
        context.coordinator.attach(device: device, queue: device?.makeCommandQueue())
        let link = view.displayLink(target: context.coordinator, selector: #selector(Coordinator.fire))
        link.add(to: .main, forMode: .common)
        context.coordinator.link = link
        return view
    }

    func updateNSView(_ nsView: MTKView, context: Context) {
        context.coordinator.onFrame = onFrame
        context.coordinator.adopt(frame: frame)
        if let frame {
            nsView.drawableSize = CGSize(
                width: CGFloat(frame.width),
                height: CGFloat(frame.height)
            )
        }
    }

    static func dismantleNSView(_ nsView: MTKView, coordinator: Coordinator) {
        coordinator.link?.invalidate()
        coordinator.link = nil
    }
}

/// Left pane: the Bevy-driven viewport.
///
/// Each display refresh sends `.frame`; the reducer ticks Rust off-MainActor
/// and publishes stats plus the frame handle back here.
struct ViewportView: View {
    @Bindable var store: StoreOf<ViewportFeature>

    var body: some View {
        ZStack(alignment: .bottomLeading) {
            ViewportMetalHost(
                onFrame: { store.send(.frame) },
                frame: store.frame
            )
            VStack(alignment: .leading, spacing: 2) {
                Text("tick \(store.tickCount)")
                    .monospacedDigit()
                    .accessibilityIdentifier("viewportTickLabel")
                Text("entities \(store.entityCount)")
                    .monospacedDigit()
                    .accessibilityIdentifier("viewportEntityLabel")
            }
            .font(.caption)
            .foregroundStyle(.white.opacity(0.85))
            .padding(8)
            .accessibilityElement(children: .contain)
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
