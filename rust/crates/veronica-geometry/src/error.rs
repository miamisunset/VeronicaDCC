//! Domain errors for geometry cooking.

use thiserror::Error;
use veronica_core::NodeId;

/// Errors for [`crate::cook`] and parameter parsing.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum CookError {
    /// The graph contains a dependency cycle, so no evaluation order exists.
    #[error("cannot cook a graph containing a dependency cycle")]
    Cycle,
    /// Evaluation order named an operator the graph does not hold.
    ///
    /// Defensive: topology and operators are inserted together and restore
    /// validates edge endpoints, so this fires only on a broken invariant —
    /// never on user input. It is an error, not a panic, so the FFI
    /// boundary can still map it to a result code.
    #[error("unknown operator: {0:?}")]
    UnknownOperator(NodeId),
    /// A present parameter had the wrong [`veronica_graph::ParamValue`]
    /// variant. Names the key and the expected shape; cooking never coerces
    /// or defaults a mistyped value silently.
    #[error("parameter \"{key}\" must be {expected}")]
    InvalidParameter {
        /// The offending parameter key (e.g. `"size"`).
        key: String,
        /// Human-readable expected shape (e.g. `"a vec3 of three f64 numbers"`).
        expected: &'static str,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_names_the_key_and_the_expected_shape() {
        let error = CookError::InvalidParameter {
            key: "size".to_owned(),
            expected: "a vec3 of three f64 numbers",
        };
        assert_eq!(
            error.to_string(),
            "parameter \"size\" must be a vec3 of three f64 numbers"
        );
        assert_eq!(
            CookError::Cycle.to_string(),
            "cannot cook a graph containing a dependency cycle"
        );
    }
}
