//! The cook entry point: graph in, payloads out.

use veronica_core::NodeId;
use veronica_graph::{OperatorGraph, OperatorKind};

use crate::{CookError, CubeParams, GeometryPayload, ImplicitGeometry};

/// Cook every geometry operator in dependency-first evaluation order.
///
/// Returns `(operator, payload)` pairs in the order they were cooked, so
/// callers observe topological order without re-sorting. Containers
/// organize subnetworks and produce nothing: they are skipped by design,
/// not by omission. Every cube becomes
/// [`GeometryPayload::Implicit`] carrying its parsed parameters.
///
/// The `match` on [`OperatorKind`] is exhaustive on purpose: adding an
/// operator kind breaks compilation here, forcing its cook path to exist
/// before it can ever flow downstream.
///
/// # Errors
///
/// Returns [`CookError::Cycle`] when the graph has no evaluation order,
/// [`CookError::UnknownOperator`] when evaluation order names an operator
/// the graph does not hold (broken invariant, never user input), or
/// [`CookError::InvalidParameter`] when a cube's parameters are mistyped.
#[must_use = "cooked payloads are returned, not applied — feed them to realization or the handoff"]
pub fn cook(graph: &OperatorGraph) -> Result<Vec<(NodeId, GeometryPayload)>, CookError> {
    let order = graph
        .topology()
        .evaluation_order()
        .map_err(|_| CookError::Cycle)?;
    let mut cooked = Vec::with_capacity(order.len());
    for id in order {
        let operator = graph.operator(id).ok_or(CookError::UnknownOperator(id))?;
        match operator.kind {
            OperatorKind::Container => {
                // Organizers hold subnetworks; nothing to cook.
            }
            OperatorKind::Cube => {
                let params = CubeParams::parse(&operator.parameters)?;
                cooked.push((
                    id,
                    GeometryPayload::Implicit(ImplicitGeometry::Cube(params)),
                ));
            }
        }
    }
    Ok(cooked)
}
