# ADR 0002: Operator graph slice 1 — containers, intents, JSON snapshots

- Status: accepted (build spec for `feat/operator-graph-slice-1`)
- Date: 2026-09-15

## Context

The right pane is a title-only scaffold. Slice 1 must prove the full
vertical for the procedural graph: right-click → container at cursor →
drag → dive/breadcrumb → nested container → rename → delete, surviving
restart. Vocabulary is settled (`Operator`, `Container`, `Network` in
`CONTEXT.md`); undo is designed-now-built-later; positions live in Rust;
rendering is event-driven SwiftUI `Canvas`, boxes only (no ports exist yet).

## Decision

Pure-organizer containers; single Scene context; registry-driven add menu
(Swift-static, one entry); fine-grained intent FFI with whole-snapshot
mirror as versioned JSON (`"version": 1`); autosave to Application Support
on every commit; `UndoHistory<GraphSnapshot>` in Rust now with pre-image
push on every mutating intent, undo/redo intents deferred; cascade delete;
single selection; background-drag pan now, zoom later.

## Consequences

- `GraphTopology` (nodes + edges only) is superseded by the v1 snapshot
  below: positions, names, and container membership are first-class, because
  persistence and undo both restore from it. One format serves both.
- `libveronica.a` must link into Xcode in this slice — the acceptance list
  is end-to-end or it proves nothing. Manual `cargo build` + link the
  prebuilt `.a` (no script phase: `ENABLE_USER_SCRIPT_SANDBOXING` makes
  network/filesystem access from custom phases unreliable); script-phase
  automation is a later slice. The header is cbindgen-generated (0.28+,
  which parses edition-2024 `#[unsafe(no_mangle)]`) into
  `rust/crates/veronica-ffi/include/veronica.h` at build time — never
  hand-edited, never committed. Context crosses as opaque
  `VrnContextHandle` (a private-`Mutex` newtype, since `Mutex<VrnContext>`
  has no C spelling); string inputs are `const char *`.
- The scrub-vs-structure boundary (below) is a mandated pattern for all
  future continuous gestures, not slice-1 API.

## Interface contract (both tracks code against this)

### Snapshot JSON v1 (`graph-v1.json` golden fixture)

```json
{
  "version": 1,
  "operators": [
    {"id": 7, "kind": "container", "name": "Hero", "parent": null,
     "position": {"x": 120.0, "y": 80.0}}
  ],
  "edges": []
}
```

`parent: null` is root; non-null is the containing container's id. `edges`
is empty in slice 1 (no ports, no wires) but present for forward
compatibility. Positions decode as `Double`/`f64` — never `Float`/`f32`
(float round-trip is pinned by test, not by hope).

### Rust model (`veronica-graph`)

`Operator { id: NodeId, kind: OperatorKind::Container, name: String,
parent: Option<NodeId>, position: Position { x: f64, y: f64 } }`.
`GraphSnapshot { version: u32 (always 1), operators: Vec<OperatorSnapshot>,
edges: Vec<(NodeId, NodeId)> }` with `rename_all = "camelCase"`,
`default` on additive fields. New ids issued by Rust starting at 1;
`NodeId(0)` is never issued (it is the FFI "root parent" sentinel).
`create` assigns the default name `"Container"`. Strict kind validation at
the FFI boundary: only `"container"` accepted, anything else is
`InvalidArgument` — operator #2 is a pure extension. `delete` cascades the
subtree. `rename` rejects empty/blank names. `move` requires a known id;
`create` with non-null parent requires the parent to exist.

`VrnContext` gains `graph_history: UndoHistory<GraphSnapshot>` (cap 64,
same precedent as `mesh_history`); every mutating intent pushes the
pre-image before mutating. No undo/redo externs yet — internal push only.

### FFI externs (`veronica-ffi`, `VrnResult` codes, null-safe)

- `vrn_graph_create_operator(ctx, kind: *c_char, parent: u64, x: f64,
  y: f64, out_id: *mut u64) -> VrnResult`
- `vrn_graph_move_operator(ctx, id: u64, x: f64, y: f64) -> VrnResult`
- `vrn_graph_rename_operator(ctx, id: u64, name: *c_char) -> VrnResult`
- `vrn_graph_delete_operator(ctx, id: u64) -> VrnResult`
- `vrn_graph_snapshot(ctx, out_json: *mut *mut c_char) -> VrnResult`
  (Rust allocates via `CString::into_raw`)
- `vrn_graph_restore(ctx, json: *c_char) -> VrnResult` (replaces whole DAG;
  launch-load path; rejects `version != 1`)
- `vrn_string_free(s: *mut c_char)` (sole deallocator for FFI strings)

### Swift surface

`OperatorTypeDef { kind, displayName }` registry (one entry). `EngineClient`
gains the six intents plus a single `requestSnapshot() -> GraphSnapshot`
that owns the allocate/free boundary — the raw pointer never escapes.
`NodeGraphFeature.State`: mirrored operators, `path: [UInt64]`, single
`selected: UInt64?`, Swift-local pan offset; dive/breadcrumb/pan/selection
never touch FFI. Persistence: autosave snapshot JSON to
`Application Support/<bundle>/graph-v1.json` on every committed mutation;
load on launch via `restore`; corrupt file starts empty (no crash).
Canvas gestures: click select, double-click dive, background-drag pan,
box-drag preview-then-commit, right-click/Ctrl-click menu, Enter rename
(inline overlay), Delete deletes.

### Scrub-vs-structure boundary (mandated future pattern)

Structural edits (create/move-commit/delete/rename/undo/file-open) move
whole JSON snapshots. Continuous gestures (future: gizmo drags, slider
scrubs) use direct scalar setters with **gesture-begin push, scrub writes,
release snapshot** — the pre-image push must precede the first scalar
write, or undo restores mid-gesture mush. Slice 1 implements only the
structural half.

### Tests (gates already mandate; these are the slice-specific pins)

- Golden fixture `rust/crates/veronica-graph/tests/fixtures/graph-v1.json`:
  Rust integration test deserializes it; Swift test loads the same file
  (repo-relative via `#filePath`) and decodes it — drift fails both gates.
- `f64` round-trip property test (Rust; snapshot→restore→snapshot
  bit-identical positions) and Swift decode→encode→decode equality.
- Looped snapshot/free test (Rust) for the allocator boundary.
- Slice acceptance as a UI test (create/move/dive/nested/back/rename/
  delete/persist-relaunch); reducer unit tests for path/selection/stale
  rules via `TestStore`.
