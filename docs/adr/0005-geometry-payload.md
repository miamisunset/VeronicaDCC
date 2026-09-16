# ADR 0005: Geometry payload — implicit-first, Bevy-free, primvar map

- Status: accepted (build spec #17, issues #19/#20)
- Date: 2026-09-16

## Context

The DAG holds organizers only; nothing makes anything visible. The first
geometry operator must prove procedural flow toward the viewport without
coupling the pure graph core to Bevy: the engine mesh type is `f32`-only
and would pull render types and precision loss into graph code. Cooking
must also avoid paying for topology when downstream nodes only transform
parameters.

## Decision

New `veronica-geometry` crate, Bevy-free: it owns the payload, the cook
entry point over evaluation order, and per-operator parameter parsing.
`veronica-graph` stays data + topology (it gains only the `Cube` operator
kind); `veronica-scene` owns the render handoff; the pipeline works in
`f64` throughout with exactly one documented `f64`→`f32` conversion at
the handoff.

Geometry flows on edges as `GeometryPayload` with two variants. `Implicit`
carries parameters only — the cube's size and center verbatim — allocating
no vertices, so parameter-only downstream nodes never pay for topology.
`Evaluated` is our own `f64` soup (`positions`, `indices`, plus a named
attribute map), produced on demand when a consumer needs topology; it is
deliberately not `bevy_mesh::Mesh`.

Realized channels travel by name (glossary `Primvar`): `normal` and `uv`
are the standard keys, custom channels use namespaced keys (e.g.
`veronica:wetness`) so future producer nodes cannot collide with
standards. Cube proves the carrier by moving its uvs through the map;
custom-attribute producers and shader-side binding are later work.

`cook` walks dependency-first order and its `match` on `OperatorKind` is
exhaustive on purpose: a new kind breaks compilation until its cook path
exists. Containers are skipped by design (they organize, they produce
nothing). Coordinates are meters, Y-up right-handed (glossary `Unit`).

## Consequences

- Realization (`Implicit` → `Evaluated`) and the Bevy handoff are separate
  jobs (#20) against seams defined here; the payload enum and attribute
  map are the deliverable as much as the cube.
- Shape is validated at parse, values are not: non-finite floats pass
  through verbatim; range checks are future validation, not parsing.
- Unknown parameter keys are ignored (forward tolerance); mistyped known
  keys fail loudly with key + expected shape.
- Cube has no inputs, so no input-port plumbing is built; downstream
  wiring belongs to the editing-node work that follows.
