# AGENTS.md — Veronica

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

## Project quirks (do not guess wrong)
- `Veronica/`, `VeronicaTests/`, `VeronicaUITests/` are `PBXFileSystemSynchronizedRootGroup` — files are auto-added by filesystem. Never hand-edit `project.pbxproj` to add sources.
- App target is Swift 6.4, `SWIFT_DEFAULT_ACTOR_ISOLATION=MainActor`, `SWIFT_STRICT_MEMORY_SAFETY=YES`, `SWIFT_APPROACHABLE_CONCURRENCY=YES`. Mark background work `nonisolated` / detached explicitly; no implicit background mutation.
- Tests differ: `VeronicaTests` uses Swift Testing (`@Test`, `#expect`), `VeronicaUITests` uses XCTest + `XCUIApplication`. Don't mix frameworks.
- App is sandboxed (`ENABLE_APP_SANDBOX=YES`, `ENABLE_USER_SELECTED_FILES=readonly`) with hardened runtime. File access outside the sandbox needs entitlements + open-panel flow — procedural cache/graph files must account for this.
- Deployment target `MACOSX_DEPLOYMENT_TARGET=27.0`, `SDKROOT=macosx`, bundle `com.github.miamisunset.Veronica`. Keep new targets consistent.
- `ENABLE_USER_SCRIPT_SANDBOXING=YES` — custom build phases needing network/fs access will fail unless allowlisted.

## Rust error handling (mandated)
- Every crate defines domain errors with `thiserror` (`#[derive(Error)]`); it is in `rust/` `[workspace.dependencies]` — add it to any new crate's `[dependencies]`.
- `anyhow` is banned in library crates; allowed only in future binaries/harnesses at the edge.
- No `unwrap()` / `expect()` / `panic!()` in production code — enforced by Clippy (`unwrap_used`, `expect_used`, `panic` + `-D warnings`). Only tests and `build.rs` are exempt (see `rust/clippy.toml`, `build.rs` header).
- FFI boundary maps errors to `VrnResult` codes; never propagate panics or Rust error types into Swift.

## Lint + test gates (all mandatory, both languages)
- Swift lint: `swiftlint lint` from repo root (config `.swiftlint.yml`) — zero violations.
- Rust lint, from `rust/`: `cargo fmt --check` and `cargo clippy --workspace --all-targets -- -D warnings` — zero findings.
- Swift unit tests: `VeronicaTests`, Swift Testing (`@Test`, `#expect`).
- Swift integration tests: `VeronicaUITests`, XCTest + `XCUIApplication` (launch the app, drive real flows). New user-facing flow = new UI test.
- Rust unit tests: `#[cfg(test)] mod tests` inside each module. Rust integration tests: `rust/crates/<crate>/tests/*.rs` against the public API (no `tests/` dirs exist yet — first integration-test task scaffolds them). Run: `cargo test --workspace` (or `-p <crate>`).
- Coverage (both languages, floor 85% — never ship below it):
  - Rust: `./scripts/coverage-rust.sh` (cargo-llvm-cov `--fail-under-lines 85`, workspace-wide).
  - Swift: `./scripts/coverage-swift.sh` (`xcodebuild -enableCodeCoverage YES` + `scripts/xccov-coverage.py` threshold check).
- Never ship with a red gate; fix the finding, don't weaken the config.

## Intended architecture (user-directed, scaffold accordingly)
- SwiftUI owns: node editor UI, viewport hosting, menus, document model. Bevy owns: procedural evaluation / scene data.
- When creating the Rust side: new `rust/` Cargo workspace (edition 2024), Bevy as headless/staticlib dependency, exposed to Swift via UniFFI or C-ABI + `Veronica/Bridge/`. Do not embed Bevy event loop inside SwiftUI view hierarchy without a bridge layer.
- Procedural graph state must live in Rust (single source of truth); Swift mirrors it read-only for rendering/editing. No dual-writable graph model.
