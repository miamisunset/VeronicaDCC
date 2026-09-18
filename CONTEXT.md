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

## Topology

The triangle count and connectivity of realized geometry. Parameter
edits that preserve Topology retain Selection; edits that change it
clear Selection.

## Resolution

The segment and ring counts controlling a Sphere's triangle budget.
The first Topology-varying parameters in the graph.

## Viewport Pixels

The rendered frames displayed in the Viewport. Produced Rust-side from live
Scene state and published per Tick over `IOSurface`; Swift presents them
without interpreting scene content. Frames render on the GPU; the CPU
rasterizer remains as a deterministic test oracle. _Avoid_:
rendering (also names the data handoff from `EvaluatedMesh` to engine
mesh — a different step).

## Recook Loop

The path from a Parameter change through cook → realize → scene to updated
Viewport Pixels. The loop is closed when editing a Cube's size visibly
changes the Viewport.

## Viewport Camera

The perspective camera through which the Viewport views the Scene. Owned by
Rust like all Scene state; Swift sends navigation intents and mirrors
read-only, never writing camera transforms.

## Pivot

The interest point navigation orbits around. The scene bounds center on load
and after framing; panning moves the view without moving it.

## Frame All

Fitting the whole Scene bounds into the Viewport in one snap. Triggered by
the `F` key; no animation, no undo.

## Primvar

Named data carried by geometry, from standard channels (position, normal,
uv) to custom ones (tension, wetness). Bound to render attributes where
geometry leaves the graph. _Avoid_: attribute (the Houdini SOP name for
the same idea).

## Face

One polygon of realized geometry (a quad-band pair or single fan
triangle, per the P1 `tri_to_poly` grouping). Identity is the polygon id
returned by Pick and held by Selection. _Avoid_: triangle ordinal
(pre-#79 wire; now an implementation detail of realize/pick internals),
polygon only where N-gons are implied (the soup still has none — a
polygon is a pair or a fan single).

## Pick

The operation mapping one viewport tap to one polygon identity
(`Face`), computed Rust-side via the `tri_to_poly` grouping, plus that
resulting identity. A tap on empty space is a Pick that hits nothing.
_Avoid_: select (the verb is reserved for Selection state changes).

## Selection

The ephemeral tool state holding current Picks. Never serialized into the
graph snapshot; cleared on background miss and on topology change.
_Avoid_: group (a persisted graph Operator, not built yet).

## Highlight

The viewport rendering of the Selection. Driven primvar-first: a mask on
the mesh, mixed emissive-side. _Avoid_: outline, overlay (both name
throwaway techniques, not the mask).
