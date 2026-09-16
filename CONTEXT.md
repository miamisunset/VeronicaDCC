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
read-only once the graph FFI surface exists. Also called "node editor".

## Canvas

The visual surface inside the NodeGraphPane showing one Network. Operator
boxes are positioned on it; pan and hover are canvas-local.

## Tick

One advancement of the Scene (`SceneWorld::update` / `vrn_tick`). The frame
loop is Swift-driven: each display refresh requests exactly one Tick, then
publishes the resulting frame handle to the UI thread.

## Operator

The behavior-carrying element of the procedural graph (a Houdini "OP": it
evaluates/cooks). Rust owns Operators and the DAG; Swift renders them and
sends intents. `NodeId` is the storage key identifying an Operator. "Node"
means only the visual box on the canvas, if it is used at all.

## Container

An Operator that only organizes: it holds a subnetwork of child Operators,
has no ports, and does not cook. Its children cook in place as if the
Container weren't there. There is exactly one network kind (the Scene
network); hierarchy comes from Containers, not from switching contexts.

## Network

One level of the procedural graph: the Operators directly inside a Container,
or at the root. Diving navigates between Networks; the breadcrumb shows the
path from the root to the Network on screen.

## Parameter

A named, editable value carried by an Operator. Rust owns Parameters; the
parameter editor shows the selected Operator's Parameters. Cross-Network
parameter binding is future work, not part of the term.

## ParameterEditor

The pane showing the selected Operator's Parameters. Selection-driven with
an explicit empty state; a fixed third pane until rearrangement lands.

## Unit

One unit of distance is one meter. Coordinates are Y-up, right-handed.

## Implicit Geometry

Geometry described by parameters instead of stored vertices. A Cube is
implicit until Realization turns it into vertices. _Avoid_: procedural,
parametric (both describe the graph, not the data).

## Realization

The step turning Implicit Geometry into vertices, run only when a
downstream consumer needs topology.

## Viewport Pixels

The rendered frames displayed in the Viewport. Produced Rust-side by the
Bevy renderer and published per Tick over `IOSurface`; Swift presents them
without interpreting scene content. _Avoid_: rendering (also names the data
handoff from `EvaluatedMesh` to engine mesh — a different step).

## Recook Loop

The path from a Parameter change through cook → realize → scene to updated
Viewport Pixels. The loop is closed when editing a Cube's size visibly
changes the Viewport.

## Primvar

Named data carried by geometry, from standard channels (position, normal,
uv) to custom ones (tension, wetness). Bound to render attributes where
geometry leaves the graph. _Avoid_: attribute (the Houdini SOP name for
the same idea).
