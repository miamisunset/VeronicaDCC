# ADR 0001: IOSurface render transport for the Bevy viewport

- Status: accepted (spike slice 1: seam only; slice 2: live pixels)
- Date: 2026-09-15

## Context

The Viewport (left pane) must show real Bevy-rendered pixels inside the
SwiftUI window, which refreshes natively (144 Hz target monitor) at full pane
size. The Rust workspace currently forbids `bevy_render` / `winit` and runs a
headless `bevy_app::App` with `ScheduleRunnerPlugin` only. The frame loop is
Swift-driven: a view-vended `CADisplayLink` paces one `vrn_tick` per display
refresh, issued off-MainActor.

## Options considered

1. **Bevy-owned `winit` window.** Cheap, but the pixels land in a separate OS
   window, not the split-view Viewport. Fails the layout requirement.
2. **CPU readback** (render to `Image` target, ship bytes over FFI as
   `CGImage`). True integration but a multi-megabyte memcpy plus image
   rebuild per frame; cannot hold native refresh at full pane size.
3. **Swift-owned `CAMetalLayer`, raw pointer to Rust.** Same visual result,
   but inverts ownership: Bevy's renderer wants to own surface lifecycle, and
   a non-retained layer pointer across FFI is a use-after-free hazard on
   window close. Rejected (see decision).
4. **Windowless Bevy render to a shared texture, published via `IOSurface`.**
   Swift owns an `MTKView`; Rust owns the render target; per-frame handoff is
   an `IOSurface` handle (refcounted, designed for cross-process GPU sharing),
   not a memcpy and not a raw UI pointer.

## Decision

Option 4. Each side owns its objects; resize is one FFI call ("new size") and
the published artifact is a handle. This preserves the repo ownership rule
(Rust: scene/render state; Swift: presentation) and FFI hygiene (opaque
handles, `VrnResult` codes).

## Consequences

- The workspace "no `bevy_render`" constraint is lifted for a render-capable
  path (new or extended crate); headless schedule-tick stays for logic-only
  use.
- `veronica.h` must actually export the surface (fix generation) before Xcode
  links `libveronica.a`.
- Fallback, timeboxed: if the headless Metal adapter will not initialize
  windowless on the target machine, fall back to option 2 at reduced rate to
  still land visual proof, and record it here.
