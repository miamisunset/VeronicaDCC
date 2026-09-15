# ADR 0003: Parameter wire contract — string map, virtual `name` key

- Status: accepted (build spec for `feat/parameter-editor-slice-1`, issues
  #10/#11)
- Date: 2026-09-15

## Context

Slice 1 needs editable operator values over the existing intent-plus-
snapshot-mirror surface without a wire version bump. The open questions
were the map shape, how the editor's Name row relates to the existing
rename intent, and key/value normalization.

## Decision

`Operator` gains `parameters: BTreeMap<String, String>` (sorted keys keep
snapshots stable), additive under `#[serde(default)]` so v1 payloads
without it still decode — wire stays version 1. `set_parameter` is a new
intent (`vrn_graph_set_parameter`, same result codes and pre-push guards
as rename). `"name"` is a reserved virtual key that delegates to the
rename path instead of living in the map, so the map starts empty and the
editor's Name row and the canvas rename flow share one validation. Keys
are trimmed, values stored verbatim; blank keys and blank rename values
are rejected before the undo pre-image push, like rename.

## Consequences

- Seeding meant "the editor opens on the name", not a map entry: the wire
  map never contains `"name"`. A future reader seeing an empty map next to
  a named operator should not "fix" this by seeding it — that would fork
  the rename source of truth.
- Typed parameters later extend values, not shape; renaming the virtual
  key or changing trim rules is a wire-level break and needs a version
  bump, not a silent change.
