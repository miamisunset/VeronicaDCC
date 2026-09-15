# Veronica — Ubiquitous Language

Glossary only. No implementation details, no specs, no decisions (those live
in `docs/adr/`). Terms are added the moment they are agreed, never batched.

## Scene

The procedural 3D content owned by Rust. Single source of truth. Swift never
constructs scene objects; it sends intents and mirrors read-only.

## Viewport

The left pane of the main window. Hosts the Bevy-driven view of the Scene.
"Pixels provably change" means successive frames observed in the Viewport
differ because Scene state advanced, not because Swift re-rendered.

## NodeGraphPane

The right pane of the main window. Scaffold for the future procedural node
graph editor. Currently an empty placeholder; it will mirror the Rust DAG
read-only once the graph FFI surface exists.

## Tick

One advancement of the Scene (`SceneWorld::update` / `vrn_tick`). The frame
loop is Swift-driven: each display refresh requests exactly one Tick, then
publishes the resulting frame handle to the UI thread.
