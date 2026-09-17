//! Procedural geometry cooking: parameters in, payloads out.
//!
//! Bevy-free by design: nothing here may depend on engine crates, so the
//! pure graph core never inherits render-engine types or precision. The
//! pipeline works in `f64` throughout; the single `f64`→`f32` conversion
//! lives at the render handoff in `veronica-scene` (ADR-0005).
//!
//! Cooking is two-phase. [`cook`] walks the graph in dependency-first order
//! and emits lightweight [`ImplicitGeometry`] — parameters, not vertices —
//! so downstream nodes that only transform parameters never pay for
//! topology. [`realize`] turns implicit geometry into [`EvaluatedMesh`]
//! vertices on demand, only when a consumer needs topology.

mod cook;
mod cube;
mod error;
mod face_id;
mod payload;
mod realize;
mod sphere;

pub use cook::cook;
pub use cube::{
    CUBE_CENTER_KEY, CUBE_SIZE_KEY, CubeParams, DEFAULT_CUBE_CENTER, DEFAULT_CUBE_SIZE,
};
pub use error::CookError;
pub use face_id::{
    FACE_ID_BITS, FaceIdError, HIT_ALPHA, MAX_FACE_ORDINAL, MISS_ALPHA, MISS_PIXEL,
    decode_face_ordinal, decode_pick_pixel, encode_face_ordinal, selection_survives_recook,
};
pub use payload::{
    AttributeData, EvaluatedMesh, GeometryPayload, ImplicitGeometry, PRIMVAR_NORMAL, PRIMVAR_UV,
};
pub use realize::realize;
pub use sphere::{
    DEFAULT_SPHERE_CENTER, DEFAULT_SPHERE_RADIUS, DEFAULT_SPHERE_RINGS, DEFAULT_SPHERE_SEGMENTS,
    MAX_SPHERE_RINGS, MAX_SPHERE_SEGMENTS, MIN_SPHERE_RINGS, MIN_SPHERE_SEGMENTS,
    SPHERE_CENTER_KEY, SPHERE_RADIUS_KEY, SPHERE_RINGS_KEY, SPHERE_SEGMENTS_KEY, SphereParams,
};
