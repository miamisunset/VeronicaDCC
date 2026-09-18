# ADR 0007: Sub-element selection MVP — GPU on-demand pick, face ordinals, primvar highlight

- Status: accepted (grill session 2026-09-17, spec issue #TBD)
- Date: 2026-09-17

## Context

The graph cooks a Cube and the viewport shows its pixels, but the viewport
is write-only from the user's perspective: there is no path from a tap on a
rendered face back to an element identity. Gemini proposed a four-layer
design (realize trigger, GPU picking pass, Group operator, consumer
operator) that assumed a `HalfEdgeMesh` with `SlotMap` keys, an
`ImplicitCube` type, and a `RealizeTopology` node — none of which exist.
Audited reality: geometry is index soup (`EvaluatedMesh` with positions,
indices, and a primvar map), `realize()` is already a pipeline phase (not a
node), exactly two operator kinds exist (`Container`, `Cube`), and the
viewport is a headless offscreen render with no tap channel at all (every
mouse-down starts a navigation drag). The MVP must prove tap → identity →
highlight end-to-end while staying correct at hundreds of thousands of
triangles — the scale the product targets, not the 12 triangles a Cube has
today.

## Decision

Highlight-only MVP on four pillars. **Picking is a GPU ID-buffer pass,
rendered on demand** — only when a tap arrives, never per tick — reusing
the existing offscreen render and staging-buffer readback. Per-click cost
is one extra frame render plus a one-pixel readback, independent of
triangle count; taps are not latency-sensitive, frames are, so the extra
frame of latency is acceptable and hover-highlight is excluded. A
BVH-accelerated CPU raycast was rejected: it buys a permanent second
representation of every mesh plus recook-invalidation bookkeeping for zero
lasting benefit once the GPU path exists.

**Identity is a face ordinal** (pre-#79 wire; superseded by the polygon
epoch below but kept as the design record): `(node_id, triangle_index)` into the
realized mesh, 24-bit RGB-encodable. Highlight is retained across recooks
while a node's triangle count is unchanged and cleared on topology change
— safe today (a Cube recook never changes its 12 triangles) and a
future-proof invalidation rule. Faces only; points, edges, and
shift-accumulate are the next slice.

**Highlight is primvar-driven**: Rust writes a `veronica:selection` mask
and the material mixes emissive on masked faces. The soup's split verts
make this exactly face-granular for free, and it builds the mask → pixels
machinery the future Group operator reuses instead of throwaway overlay
code. **Selection state lives in a Rust resource** (Swift mirrors
read-only): "ephemeral" describes lifetime — never serialized into the
graph snapshot — not location. Pixels and invalidation both live in Rust;
the state driving them does too. The FFI is `vrn_viewport_pick` (NDC in,
identity out-params) plus `vrn_selection_clear`; a background miss clears
the selection and returns `Ok` (standard deselect-on-miss), and a single
pick replaces.

**Half-edge is deferred to the consumer slice, but the seam is built
now.** Its API (key stability, weld tolerance, invalidation) can only be
designed correctly against its first real consumer — extrude or loop
selection — so building it now would schedule the very refactoring it
claims to avoid. Instead the contract is pinned here: face ordinals as
identity, deterministic `realize()` output order, `veronica:selection`
mask conventions, and the store's landing zone behind them. The later swap
is mechanical: backing store changes, while FFI, picks, tests, and
highlight code survive untouched.

## Consequences

- Swift needs a tap-vs-drag discriminator first: today every mouse-down
  starts a navigation drag, so a tap channel is a prerequisite of any
  picking technique, not a cost of this one.
- The GPU pass is the one piece that cannot be unit-tested headless
  without an adapter: it is pinned at the UI-test level (like viewport
  pixels today) with deterministic CPU-side tests around ID encode/decode.
- Glossary gains `Pick`, `Selection`, `Highlight`, and `Face`
  (`CONTEXT.md`); `Group` stays undefined until the persistence slice.
- Out of scope by design: Group operator, Transform/PolyExtrude
  consumers, point/edge scopes, multi-select, hover, half-edge store.

## Polygon epoch (#79, v2→v3)

Identity is now the polygon id: the P1 `tri_to_poly` grouping maps the
resolved triangle first (P2 rekeyed pick, mask, and retention to
polygons), so a tapped quad reports one shared id. `out_face` names a
polygon; `SelectionFaceOutOfRange` keeps its name and guards the polygon
range. Snapshot v3 accepts v2+v3, with a v2 restore clearing the
ephemeral live pick. Swift mirrors the pair as `(node, polygon)`.
