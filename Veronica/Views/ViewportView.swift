import ComposableArchitecture
import IOSurface
import Metal
import MetalKit
import SwiftUI

/// Long-edge cap mirrored from Rust `MAX_VIEWPORT_EDGE` (veronica-scene).
/// Intents scale proportionally to fit it, so Swift never sends what Rust
/// would reject and aspect never distorts at the cap.
nonisolated let viewportMaxEdge: UInt32 = 2048
/// Minimum per-axis backing-pixel delta before a size intent crosses FFI.
///
/// `updateNSView` runs every tick; without hysteresis, rounding jitter on
/// fractional scales would re-target the GPU target (a full texture +
/// staging rebuild) every frame. 2px is invisible and kills the churn.
nonisolated let viewportSizeHysteresis: UInt32 = 2

/// Compute the viewport size intent for the current layout, or `nil` when no
/// FFI call is owed.
///
/// Backing pixels are `bounds * scale`, rounded, then scaled proportionally
/// to fit `viewportMaxEdge` on the long edge (per-axis clamping would
/// distort aspect: 8000x6000 must become 2048x1536, not 2048x2048). Empty or
/// hidden views (`bounds <= 0`, `scale <= 0`) send nothing (a zero intent is
/// an FFI error), and sub-hysteresis jitter stays quiet so per-tick updates
/// cost only the compare. Pure for testing.
nonisolated func viewportSizeIntent(
    bounds: CGSize,
    scale: CGFloat,
    lastSent: (width: UInt32, height: UInt32)?
) -> (width: UInt32, height: UInt32)? {
    guard bounds.width > 0, bounds.height > 0, scale > 0 else { return nil }
    var width = (bounds.width * scale).rounded()
    var height = (bounds.height * scale).rounded()
    let longest = max(width, height)
    if longest > CGFloat(viewportMaxEdge) {
        let factor = CGFloat(viewportMaxEdge) / longest
        width = (width * factor).rounded()
        height = (height * factor).rounded()
    }
    // Positive by construction (`bounds`/`scale` guarded above, cap factor
    // in (0, 1]), so the conversions below cannot trap.
    let snapped = (width: UInt32(width), height: UInt32(height))
    if let lastSent {
        let deltaWidth = abs(Int(snapped.width) - Int(lastSent.width))
        let deltaHeight = abs(Int(snapped.height) - Int(lastSent.height))
        if deltaWidth < Int(viewportSizeHysteresis), deltaHeight < Int(viewportSizeHysteresis) {
            return nil
        }
    }
    return snapped
}

/// Hosts the Metal view presenting Rust-published `IOSurface` frames, and
/// owns the display link pacing the loop.
///
/// Ownership: Rust owns the `IOSurface` (valid for the engine-context
/// lifetime; Swift never releases it). Each display refresh fires `onFrame`
/// on the main run loop; the TCA effect in `ViewportFeature` hops
/// off-MainActor to tick Rust and publish stats plus the frame handle.
/// `adopt(frame:)` wraps a new handle in an `MTLTexture` once and re-wraps
/// on address change (viewport re-targets recreate the surface); `draw(in:)`
/// centers the blit over the letterbox clear, so consecutive frames differ
/// because Scene state advanced. `updateNSView` drives `drawableSize` from
/// the published frame with `autoResizeDrawable = false` and sends backing-
/// pixel size intents to Rust only on hysteresis-exceeding change.
/// `dismantleNSView` invalidates the link, breaking the link → coordinator
/// retain cycle.
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
        /// Last backing-pixel size sent to Rust. Compared per tick in
        /// `updateNSView`; the FFI call fires only on hysteresis-exceeding
        /// change, so steady-state ticks cost just the compare.
        var lastSentSize: (width: UInt32, height: UInt32)?

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
            // Deliberately empty: `drawableSize` is driven from the published
            // frame in `updateNSView` with `autoResizeDrawable = false`, so
            // restoring it here would re-trigger this callback and deadlock
            // layout during pane swaps (see slice-2 diagnosis).
        }

        func draw(in view: MTKView) {
            guard let drawable = view.currentDrawable,
                let texture = frameTexture,
                let queue = commandQueue,
                let buffer = queue.makeCommandBuffer()
            else {
                return
            }
            guard drawable.texture.width >= texture.width,
                drawable.texture.height >= texture.height
            else {
                // Resize racing the drawable: the published frame is newer
                // than `drawableSize`. Skip loudly rather than presenting a
                // black view with no trace; the next `updateNSView` adopts
                // the new frame size.
                NSLog(
                    "Viewport: drawable %dx%d < frame %dx%d, skipping blit",
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
            // Aspect-fit: the drawable matches the frame in steady state
            // (offset zero); mid-resize the smaller frame centers over the
            // letterbox clear instead of stretching.
            let originX = (drawable.texture.width - texture.width) / 2
            let originY = (drawable.texture.height - texture.height) / 2
            blit.copy(
                from: texture,
                sourceSlice: 0,
                sourceLevel: 0,
                sourceOrigin: MTLOrigin(x: 0, y: 0, z: 0),
                sourceSize: MTLSize(width: texture.width, height: texture.height, depth: 1),
                to: drawable.texture,
                destinationSlice: 0,
                destinationLevel: 0,
                destinationOrigin: MTLOrigin(x: originX, y: originY, z: 0)
            )
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
        // The drawable is driven from the published frame size (see
        // `updateNSView`). Auto-resize would fight that every layout pass:
        // the view resets `drawableSize` behind our back and the size-guard
        // in `draw(in:)` skips frames forever (the slice-2 resize freeze).
        view.autoResizeDrawable = false
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
        // Backing-pixel intent from the live layout, sent only on
        // hysteresis-exceeding change: `updateNSView` runs every tick, but
        // the FFI call (a GPU target rebuild) fires only on real resizes.
        // `setViewportSize` hops to the engine queue itself, so this stays
        // MainActor-cheap and never blocks the frame.
        let scale = nsView.window?.backingScaleFactor ?? 1
        if let intent = viewportSizeIntent(
            bounds: nsView.bounds.size,
            scale: scale,
            lastSent: context.coordinator.lastSentSize
        ) {
            context.coordinator.lastSentSize = intent
            EngineBridge.setViewportSize(width: intent.width, height: intent.height)
        }
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
