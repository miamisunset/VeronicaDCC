# Rust-owned viewport camera with coarse-op FFI

The viewport camera (transform, pivot, orbit/pan/dolly math, clamps) lives in Rust as Scene state; Swift sends one coarse camera-op call per input event (`vrn_viewport_orbit/pan/dolly/frame_all` with pixel deltas and cursor NDC) and never writes camera transforms. Gesture-to-op mapping (which button+modifier means orbit) stays in Swift as UI concern; accumulation is implicit — each event mutates the camera and the next tick renders it.

## Considered Options

Swift-owned camera transform pushed to Rust per frame was rejected: it creates dual-writable scene state against the single-source-of-truth rule, and a Swift/Rust transform skew would show as one-frame lag. Raw event streaming (Rust interprets buttons+modifiers) was rejected: it drags AppKit gesture semantics into the engine crate, where they cannot be unit-tested without fabricated events and where trackpad-vs-mouse policy doesn't belong.

## Consequences

Ticket #41/#42 gesture work touches only Swift plus the existing four entry points; camera feel changes (speeds, clamps) touch only Rust. The `camera_translation` getter exists so future Swift readouts observe without writing.
