//! Drift pin for the generated C header (ADR-0002).
//!
//! cbindgen (>= 0.28) emits `rust/crates/veronica-ffi/include/veronica.h`
//! from the `#[unsafe(no_mangle)]` exports at build time. It once silently
//! emitted an empty header (cbindgen 0.27 vs edition-2024 attributes), so
//! this test fails the gate if a Rust export lacks a header declaration.

/// Every symbol the Swift track links against must be declared.
const EXPORTS: [&str; 20] = [
    "vrn_context_create",
    "vrn_context_destroy",
    "vrn_validate_mesh",
    "vrn_tick",
    "vrn_tick_count",
    "vrn_entity_count",
    "vrn_frame_surface",
    "vrn_tick_timings",
    "vrn_viewport_set_size",
    "vrn_viewport_pick",
    "vrn_selection_clear",
    "vrn_graph_create_operator",
    "vrn_graph_move_operator",
    "vrn_graph_rename_operator",
    "vrn_graph_set_parameter",
    "vrn_graph_set_parameter_typed",
    "vrn_graph_delete_operator",
    "vrn_graph_snapshot",
    "vrn_graph_restore",
    "vrn_string_free",
];

#[test]
fn header_declares_every_export() {
    let header = include_str!("../include/veronica.h");
    assert!(header.contains("#ifndef VERONICA_H"));
    assert!(header.contains("VrnResult"));
    assert!(header.contains("VrnContextHandle"));
    for symbol in EXPORTS {
        assert!(
            header.contains(symbol),
            "header is missing declaration for {symbol}"
        );
    }
}
