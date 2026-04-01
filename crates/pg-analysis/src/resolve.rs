use std::sync::Arc;

use crate::index::WorkspaceIndex;
use crate::symbols::{QualifiedName, Symbol};

/// Resolve a name reference to its definition(s).
///
/// Resolution order:
/// 1. Exact qualified match (schema.name)
/// 2. Match in `public` schema
/// 3. Match in any schema
pub fn resolve_name(
    index: &WorkspaceIndex,
    name: &QualifiedName,
) -> Vec<Arc<Symbol>> {
    let results = index.resolve(name);
    if !results.is_empty() {
        return results;
    }

    if name.schema.is_none() {
        let public_name = QualifiedName::with_schema("public".to_string(), name.name.clone());
        let results = index.resolve(&public_name);
        if !results.is_empty() {
            return results;
        }
    }

    index.find_by_name(&name.name)
}
