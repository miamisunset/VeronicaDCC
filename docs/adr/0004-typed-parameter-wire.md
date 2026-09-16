# ADR 0004: Typed parameter wire (v2) — closed enum, strict version

- Status: accepted (build spec #17, issues #18/#19)
- Date: 2026-09-16

## Context

Operator parameters were a `BTreeMap<String, String>` (ADR-0003): enough
for the name editor, useless for procedural operators. Cube needs
per-axis size and center as real 3-vectors; future operators need numbers
and flags. The typeless string map cannot express these without
hope-parsing at every consumer.

## Decision

Parameter values are a closed `ParamValue` enum — float (`f64`), integer
(`i64`), text, flag, 3-vector (`[f64; 3]`) — externally tagged with
lowercase tags, so the wire is self-describing (`{"vec3": [...]}`).
3-vectors travel as bare `f64` triples and convert to engine math types
only at evaluation and upload boundaries: no math library on the wire.
Consumers match exhaustively instead of sniffing JSON.

The snapshot version moves 1 → 2 with strict rejection of anything else.
There is no v1-to-v2 migrator because no shipped graphs exist to preserve;
a loud `UnsupportedVersion` beats a quiet misread. Absent `parameters`
still decodes to an empty map (additive read), but version itself is
exact.

The string setter keeps its signature and stores text; there are no typed
setters in this spec — they arrive with typed editing. Cooking with
wrong-typed parameters returns a domain error naming the key and the
expected shape (`CookError::InvalidParameter`), never a coercion. Swift
mirrors the enum read-only; non-string values render with type tags and
reject string editing until typed editing exists.

## Consequences

- Wire bytes are pinned both sides (`param_values_have_stable_wire_forms`,
  golden `graph-v2.json`, Swift bit-pattern tests): renaming a tag or
  changing a shape is a wire break needing a version bump.
- `-0.0`, subnormals, and large magnitudes round-trip bit-identical
  through serde text; tests pin bits, not `==`.
- `set_parameter("size", "big")` stores text and cooks loud later — the
  setter cannot know the operator's schema, so validation lives at cook
  time, where the key meets its expected shape.
- Typed setters and typed editing UI are explicit follow-ups; until then
  every non-text value enters through snapshot restore.
