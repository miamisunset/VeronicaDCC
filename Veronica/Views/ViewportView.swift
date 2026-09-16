import ComposableArchitecture
import IOSurface
import Metal
import MetalKit
import MetalPerformanceShaders
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

/// Backing-pixel size of the drawable for the current layout, UNCAPPED.
///
/// The drawable is just an allocation: it follows the live layout
/// (`bounds * scale`, rounded) with no 2048 cap and no hysteresis. The cap
/// and hysteresis live on the Rust frame intent (`viewportSizeIntent`) only.
/// Empty or hidden views yield `.zero` (callers must not assign that).
/// Pure for testing.
nonisolated func viewportDrawableSize(bounds: CGSize, scale: CGFloat) -> CGSize {
    guard bounds.width > 0, bounds.height > 0, scale > 0 else { return .zero }
    return CGSize(
        width: (bounds.width * scale).rounded(),
        height: (bounds.height * scale).rounded()
    )
}

/// Aspect-fit destination rect (contain semantics) of a frame inside a
/// drawable, in drawable pixels.
///
/// The rect fills the drawable on the constraining axis and centers on the
/// other, so the present scales without distortion and letterboxes (or
/// pillarboxes) the remainder. Zero/negative inputs yield `.zero`.
/// Pure for testing.
nonisolated func aspectFitRect(
    frameWidth: Int,
    frameHeight: Int,
    drawableWidth: Int,
    drawableHeight: Int
) -> CGRect {
    guard frameWidth > 0, frameHeight > 0, drawableWidth > 0, drawableHeight > 0 else {
        return .zero
    }
    let fitScale = min(
        CGFloat(drawableWidth) / CGFloat(frameWidth),
        CGFloat(drawableHeight) / CGFloat(frameHeight)
    )
    let width = (CGFloat(frameWidth) * fitScale).rounded()
    let height = (CGFloat(frameHeight) * fitScale).rounded()
    let originX = ((CGFloat(drawableWidth) - width) / 2).rounded()
    let originY = ((CGFloat(drawableHeight) - height) / 2).rounded()
    return CGRect(x: originX, y: originY, width: width, height: height)
}

/// Hosts the Metal view presenting Rust-published `IOSurface` frames, and
/// owns the display link pacing the loop.
///
/// Ownership: Rust owns the `IOSurface` (valid for the engine-context
/// lifetime; Swift never releases it). Each display refresh fires `onFrame`
/// on the main run loop; the TCA effect in `ViewportFeature` hops
/// off-MainActor to tick Rust and publish stats plus the frame handle.
/// `adopt(frame:)` wraps a new handle in an `MTLTexture` once and re-wraps
/// `draw(in:)` centers the frame over the letterbox clear (scaling it only
/// while sizes disagree mid-resize), so consecutive frames differ because
/// Scene state advanced. `updateNSView` drives `drawableSize` from the live
/// size (uncapped allocation) and sends capped, hysteresis-gated size
/// intents to Rust. `dismantleNSView` invalidates the link, breaking the
/// link → coordinator retain cycle.
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
        /// Cached aspect-fit scaler and transient target. The transient is
        /// sized to the last fitted region and recreated only when that
        /// changes (i.e. mid-resize); steady-state frames copy directly.
        private var scaler: MPSImageBilinearScale?
        private var scaledTexture: MTLTexture?
        private var scaledSize: (width: Int, height: Int)?

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

        /// Present the adopted frame into `view`'s drawable, aspect-correct.
        ///
        /// The drawable follows the live layout while the published frame
        /// lags by the FFI round trip, so pane aspect != frame aspect
        /// mid-resize. The fitted region (`aspectFitRect`) is filled by
        /// scaling into a cached transient (MPS bilinear; recreated only
        /// when the fitted size changes) and centered 1:1 over the
        /// letterbox clear — never stretched. Steady-state frames already
        /// match and copy directly. The 1:1 copy preserves row order, so
        /// orientation matches the pre-resize path by construction.
        func draw(in view: MTKView) {
            guard let drawable = view.currentDrawable,
                let texture = frameTexture,
                let device,
                let queue = commandQueue,
                let buffer = queue.makeCommandBuffer()
            else {
                return
            }
            let drawableTexture = drawable.texture
            let fit = aspectFitRect(
                frameWidth: texture.width,
                frameHeight: texture.height,
                drawableWidth: drawableTexture.width,
                drawableHeight: drawableTexture.height
            )
            guard fit != .zero else { return }
            let fitWidth = Int(fit.size.width)
            let fitHeight = Int(fit.size.height)
            let source: MTLTexture
            if fitWidth == texture.width, fitHeight == texture.height {
                source = texture
            } else if let scaled = scaledTexture(
                width: fitWidth,
                height: fitHeight,
                device: device,
                texture: texture,
                buffer: buffer
            ) {
                source = scaled
            } else if texture.width <= drawableTexture.width,
                texture.height <= drawableTexture.height {
                // Transient unavailable: centered copy keeps aspect (with
                // stale borders) instead of presenting nothing.
                NSLog("Viewport: scaled target missing, falling back to centered copy")
                source = texture
            } else {
                NSLog(
                    "Viewport: drawable %dx%d < frame %dx%d, skipping present",
                    drawableTexture.width,
                    drawableTexture.height,
                    texture.width,
                    texture.height
                )
                return
            }
            // Letterbox clear first: regions outside the centered copy must
            // show the clear color, not stale drawable pixels. MTKView's
            // pass descriptor clears to `clearColor` by default.
            if let pass = view.currentRenderPassDescriptor,
                let clear = buffer.makeRenderCommandEncoder(descriptor: pass) {
                clear.endEncoding()
            }
            // The encoder is created only after the size checks: an early
            // return with a live un-ended encoder aborts under Metal
            // validation when the autorelease pool drains.
            guard let blit = buffer.makeBlitCommandEncoder() else { return }
            let originX = (drawableTexture.width - source.width) / 2
            let originY = (drawableTexture.height - source.height) / 2
            blit.copy(
                from: source,
                sourceSlice: 0,
                sourceLevel: 0,
                sourceOrigin: MTLOrigin(x: 0, y: 0, z: 0),
                sourceSize: MTLSize(width: source.width, height: source.height, depth: 1),
                to: drawableTexture,
                destinationSlice: 0,
                destinationLevel: 0,
                destinationOrigin: MTLOrigin(x: originX, y: originY, z: 0)
            )
            blit.endEncoding()
            buffer.present(drawable)
            buffer.commit()
        }

        /// Scale `texture` into the cached transient sized
        /// `width`x`height`, returning it — or `nil` when the transient
        /// cannot be allocated. The cache is recreated only when the
        /// fitted size changes, so steady-state ticks never reach here.
        private func scaledTexture(
            width: Int,
            height: Int,
            device: MTLDevice,
            texture: MTLTexture,
            buffer: MTLCommandBuffer
        ) -> MTLTexture? {
            if scaledTexture == nil || scaledSize?.width != width || scaledSize?.height != height {
                let descriptor = MTLTextureDescriptor.texture2DDescriptor(
                    pixelFormat: .bgra8Unorm,
                    width: width,
                    height: height,
                    mipmapped: false
                )
                descriptor.usage = [.shaderRead, .shaderWrite]
                descriptor.storageMode = .private
                guard let fresh = device.makeTexture(descriptor: descriptor) else {
                    return nil
                }
                scaledTexture = fresh
                scaledSize = (width, height)
            }
            guard let scaled = scaledTexture else { return nil }
            if scaler == nil {
                scaler = MPSImageBilinearScale(device: device)
            }
            scaler?.encode(
                commandBuffer: buffer,
                sourceTexture: texture,
                destinationTexture: scaled
            )
            return scaled
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
            // Deliberately empty: `drawableSize` is driven from the live
            // layout in `updateNSView` with `autoResizeDrawable = false`, so
            // restoring it here would re-trigger this callback and deadlock
            // layout during pane swaps (see slice-2 diagnosis).
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
        // The drawable follows the live layout size (see `updateNSView`).
        // Auto-resize would fight that every layout pass: the view resets
        // `drawableSize` behind our back and the present stretches a stale
        // frame during resize (the slice-2 freeze lineage).
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
        // The drawable is just an allocation: follow the live backing size
        // uncapped (the 2048 cap stays on the Rust frame intent only). No
        // hysteresis — re-setting the current value is cheap, and the `!=`
        // guard skips even that on steady-state ticks.
        let liveDrawable = viewportDrawableSize(bounds: nsView.bounds.size, scale: scale)
        if liveDrawable != .zero, nsView.drawableSize != liveDrawable {
            nsView.drawableSize = liveDrawable
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
