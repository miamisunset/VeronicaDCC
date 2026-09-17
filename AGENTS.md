# AGENTS.md — Veronica

## Branching (mandated)
- Never commit or push directly to `main`. All work happens on short-lived feature branches (`<type>/<slug>`, e.g. `feat/node-graph-eval`) merged back via pull request.
- `main` is protected on GitHub (no direct pushes, no force-pushes, PR review required). If protection is missing, re-enable it before merging anything.

## Mandated skills (load before touching code)
Load via the `skill` tool; they are pre-allowed in `opencode.json`.
- Any Rust work (write or review): `rust-skills`.
- SwiftUI/TCA structure, reducers, navigation: `swift-architecture-skill`.
- SwiftUI correctness/modern-API review: `swiftui-pro`.
- Unit tests: `swift-testing-pro`.
- Public Swift API naming/docs: `swift-api-design-guidelines-skill`.
Do not write Rust or Swift without loading the matching skill first.

## What this is
Veronica is a macOS DCC app with a Houdini-style procedural workflow (node graph → viewport).
Frontend: Swift + SwiftUI (`Veronica/`). Backend: Rust 2024 edition + Bevy (not yet scaffolded).

## Current repo state
Fresh Xcode template. Only real entrypoints are `Veronica/VeronicaApp.swift` (`@main`) and `Veronica/ContentView.swift`.
No `Cargo.toml`, no Rust bridge, no node-graph/viewport code yet. First backend task must scaffold it — do not assume it exists.

## Build / test (verified)
- Schemes: only `Veronica` is shared. Targets: `Veronica`, `VeronicaTests`, `VeronicaUITests`.
- Build: `xcodebuild -project Veronica.xcodeproj -scheme Veronica -destination 'platform=macOS' build`
- Test all: `xcodebuild -project Veronica.xcodeproj -scheme Veronica -destination 'platform=macOS' test`
- Single unit test (Swift Testing): `xcodebuild ... test -only-testing:VeronicaTests/<StructName>/<testName>`
- Single UI test (XCTest): `xcodebuild ... test -only-testing:VeronicaUITests/<ClassName>/<testName>`
- Default configuration is Release when `-configuration` is omitted; pass `-configuration Debug` for dev builds.
- Rust toolchain present (rustc/cargo 1.98.x) but unused until a crate is added. Once added: `cargo test -p <crate>` for backend-only checks.
- Xcode links `rust/target/release/libveronica.a` in ALL configs (Debug included), so Run-from-Xcode stays smooth: rebuild it via `cargo build --release -p veronica-ffi` from `rust/` after any Rust change, or the app silently runs stale Rust.

## Project quirks (do not guess wrong)
- `Veronica/`, `VeronicaTests/`, `VeronicaUITests/` are `PBXFileSystemSynchronizedRootGroup` — files are auto-added by filesystem. Never hand-edit `project.pbxproj` to add sources.
- App target is Swift 6.4, `SWIFT_DEFAULT_ACTOR_ISOLATION=MainActor`, `SWIFT_STRICT_MEMORY_SAFETY=YES`, `SWIFT_APPROACHABLE_CONCURRENCY=YES`. Mark background work `nonisolated` / detached explicitly; no implicit background mutation.
- Tests differ: `VeronicaTests` uses Swift Testing (`@Test`, `#expect`), `VeronicaUITests` uses XCTest + `XCUIApplication`. Don't mix frameworks.
- App is sandboxed (`ENABLE_APP_SANDBOX=YES`, `ENABLE_USER_SELECTED_FILES=readonly`) with hardened runtime. File access outside the sandbox needs entitlements + open-panel flow — procedural cache/graph files must account for this.
- Deployment target `MACOSX_DEPLOYMENT_TARGET=27.0`, `SDKROOT=macosx`, bundle `com.github.miamisunset.Veronica`. Keep new targets consistent.
- `ENABLE_USER_SCRIPT_SANDBOXING=YES` — custom build phases needing network/fs access will fail unless allowlisted.

## Rust best practices (mandated, researched 2026)
Sources: Azure SDK for Rust guidelines (`azure.github.io/azure-sdk`), `rust-skills`.
- Domain errors with `thiserror` in every crate (`#[derive(Error)]`); it is in `rust/` `[workspace.dependencies]` — add it to any new crate's `[dependencies]`. `anyhow` is banned in library crates; allowed only in future binaries/harnesses at the edge.
- Propagate with `?` + `#[from]` / `#[source]`; match on error kinds, never on message strings; never `map_err` away the source chain.
- Workspace lints are the only lint config (`[workspace.lints]` + `[lints] workspace = true`); `veronica-ffi` keeps a synced manual copy (Cargo forbids inherit+override). Enforced: `cargo clippy --workspace --all-targets -- -D warnings`.
- Every `-> Result` documents `# Errors`; every `unsafe fn` documents `# Safety` plus `// SAFETY:` per block (enforced: `missing_errors_doc`, `undocumented_unsafe_blocks`).
- `#[must_use]` on constructors, builders, validators, and pure fallibles (enforced: `must_use_candidate`).
- Newtypes at I/O boundaries (`NodeId`, `MeshId`); parse, don't validate; accept `&str` / `&[T]`, never `&String` / `&Vec<T>`.
- Edition 2024 FFI hygiene: `unsafe extern`, `#[unsafe(no_mangle)]`, minimal unsafe scope. FFI maps errors to `VrnResult` codes; never propagate panics or Rust error types into Swift.
- Libraries emit `tracing` with structured fields — never install a subscriber, never log secrets; only binaries choose the subscriber.
- Serde wire contract is explicit: `rename_all`, `default` on additive fields, deliberate enum tagging.

## Swift best practices (mandated, researched 2026)
Sources: Swift evolution (SE-0466 default isolation), TCA docs, SwiftLint rule directory.
- Swift 6 language mode everywhere; no `@preconcurrency` as a permanent fix. Verify `SWIFT_TREAT_WARNINGS_AS_ERRORS=YES` at the next build-settings touch.
- `Sendable` on all value-type models; `@unchecked Sendable` only with a `// SAFETY:` invariant comment.
- No `try!` / `as!` / `!` in production — enforced as lint errors (`force_unwrapping/cast/try`). Only tests and `#Preview`s are exempt (child `.swiftlint.yml`s).
- Isolation by default: app target stays `MainActor`-isolated; background work explicitly `nonisolated` / detached (see `Veronica/Bridge/`).
- TCA for new features: `@Reducer` + `@ObservableState`, `@Bindable` stores, `@Presents` destinations; `State: Sendable + Equatable`; effects capture snapshots, never mutable state.
- `@Observable`, never `ObservableObject` / `@Published`, for non-TCA view models.
- Swift Testing (`@Test` / `#expect` / `#require`, parameterized over copy-paste) for unit tests; XCTest only for `XCUIApplication` UI tests and perf metrics.
- Typed `throws(MyError)` at fallible domain boundaries; `precondition` (not `fatalError`) for programmer errors.
- `#Preview` for every SwiftUI view and component — canvas feedback stays instant as the UI grows. Previews live in app sources, so they follow production lint (no force-ops); seed them with literal state instead.
- Enforcement lives in `.swiftlint.yml` (`strict: true`); never `only_rules`; SwiftUI-noisy rules stay disabled with reasons in the config comments.

## Lint + test gates (all mandatory, both languages)
- Swift lint: `swiftlint lint` from repo root (config `.swiftlint.yml`) — zero violations.
- Rust lint, from `rust/`: `cargo fmt --check` and `cargo clippy --workspace --all-targets -- -D warnings` — zero findings.
- Swift unit tests: `VeronicaTests`, Swift Testing (`@Test`, `#expect`).
- Swift integration tests: `VeronicaUITests`, XCTest + `XCUIApplication` (launch the app, drive real flows). New user-facing flow = new UI test.
- Rust unit tests: `#[cfg(test)] mod tests` inside each module. Rust integration tests: `rust/crates/<crate>/tests/*.rs` against the public API. Iterate scoped: `cargo test -p <crate>`; iterate Swift scoped: `-only-testing:<Target>/<Test>`.
- Coverage (both languages, floor 85% — never ship below it). The coverage scripts EXECUTE the suites, so they are the single full-suite runs — never add a plain `cargo test --workspace` or full-scheme `xcodebuild test` next to them; that executes every test twice:
  - Rust: `./scripts/coverage-rust.sh` (cargo-llvm-cov `--fail-under-lines 85`, workspace-wide).
  - Swift: `./scripts/coverage-swift.sh` (`xcodebuild -enableCodeCoverage YES` + `scripts/xccov-coverage.py` threshold check).
  - Reading a red coverage gate: no coverage table in the output means tests failed; a table below 85 means tests passed but coverage is short.
- Never ship with a red gate; fix the finding, don't weaken the config.

## Intended architecture (user-directed, scaffold accordingly)
- SwiftUI owns: node editor UI, viewport hosting, menus, document model. Bevy owns: procedural evaluation / scene data.
- When creating the Rust side: new `rust/` Cargo workspace (edition 2024), Bevy as headless/staticlib dependency, exposed to Swift via UniFFI or C-ABI + `Veronica/Bridge/`. Do not embed Bevy event loop inside SwiftUI view hierarchy without a bridge layer.
- Procedural graph state must live in Rust (single source of truth); Swift mirrors it read-only for rendering/editing. No dual-writable graph model.

## Agent skills

### Issue tracker

Issues live in GitHub Issues (`miamisunset/VeronicaDCC`, via `gh`). See `docs/agents/issue-tracker.md`.

### Triage labels

Five canonical roles, label strings equal to role names. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: root `CONTEXT.md` + `docs/adr/`, created lazily. See `docs/agents/domain.md`.
