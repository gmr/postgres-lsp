use std::sync::Arc;

use dashmap::DashMap;
use tree_sitter::Tree;

use crate::symbols::{self, QualifiedName, Symbol, SymbolKind, SymbolRef};

/// A concurrent workspace-wide symbol index.
///
/// Stores definitions and references keyed by file URI, with lookup
/// by qualified name. Uses `DashMap` for lock-free concurrent reads.
pub struct WorkspaceIndex {
    /// Definitions grouped by URI.
    definitions: DashMap<String, Vec<Arc<Symbol>>>,
    /// References grouped by URI.
    references: DashMap<String, Vec<Arc<SymbolRef>>>,
    /// Definitions keyed by (kind, name) for fast lookup.
    by_name: DashMap<(SymbolKind, String), Vec<Arc<Symbol>>>,
}

impl WorkspaceIndex {
    pub fn new() -> Self {
        Self {
            definitions: DashMap::new(),
            references: DashMap::new(),
            by_name: DashMap::new(),
        }
    }

    /// Re-index a file: remove old entries and extract new ones from the tree.
    pub fn update_file(&self, uri: &str, tree: &Tree, source: &str) {
        self.remove_file(uri);

        let symbols = symbols::extract_symbols(tree, source, uri);
        let refs = symbols::extract_references(tree, source, uri);

        let symbol_arcs: Vec<Arc<Symbol>> = symbols.into_iter().map(Arc::new).collect();

        // Index by name
        for sym in &symbol_arcs {
            let key = (sym.kind, sym.name.name.to_lowercase());
            self.by_name.entry(key).or_default().push(Arc::clone(sym));

            // Also index children (columns)
            for child in &sym.children {
                let child_arc = Arc::new(child.clone());
                let child_key = (child.kind, child.name.name.to_lowercase());
                self.by_name
                    .entry(child_key)
                    .or_default()
                    .push(child_arc);
            }
        }

        self.definitions.insert(uri.to_string(), symbol_arcs);
        self.references.insert(
            uri.to_string(),
            refs.into_iter().map(Arc::new).collect(),
        );
    }

    /// Remove all entries for a file.
    pub fn remove_file(&self, uri: &str) {
        if let Some((_, old_symbols)) = self.definitions.remove(uri) {
            let mut keys_to_check = Vec::new();
            for sym in &old_symbols {
                let key = (sym.kind, sym.name.name.to_lowercase());
                self.by_name.entry(key.clone()).and_modify(|v| {
                    v.retain(|s| s.uri != uri);
                });
                keys_to_check.push(key);
                for child in &sym.children {
                    let child_key = (child.kind, child.name.name.to_lowercase());
                    self.by_name.entry(child_key.clone()).and_modify(|v| {
                        v.retain(|s| s.uri != uri);
                    });
                    keys_to_check.push(child_key);
                }
            }
            // Clean up empty entries.
            for key in keys_to_check {
                self.by_name.remove_if(&key, |_, v| v.is_empty());
            }
        }
        self.references.remove(uri);
    }

    /// Find definitions by kind and name (case-insensitive).
    pub fn find_definitions(&self, kind: SymbolKind, name: &str) -> Vec<Arc<Symbol>> {
        let key = (kind, name.to_lowercase());
        self.by_name
            .get(&key)
            .map(|v| v.value().clone())
            .unwrap_or_default()
    }

    /// Find any definition matching a name (case-insensitive), regardless of kind.
    pub fn find_by_name(&self, name: &str) -> Vec<Arc<Symbol>> {
        let lower = name.to_lowercase();
        let mut results = Vec::new();
        for entry in self.by_name.iter() {
            if entry.key().1 == lower {
                results.extend(entry.value().iter().cloned());
            }
        }
        results
    }

    /// Find definitions matching a qualified name (with schema resolution).
    pub fn resolve(&self, name: &QualifiedName) -> Vec<Arc<Symbol>> {
        let lower = name.name.to_lowercase();
        let mut results = Vec::new();

        for entry in self.by_name.iter() {
            if entry.key().1 != lower {
                continue;
            }
            for sym in entry.value() {
                if let Some(ref schema) = name.schema {
                    if sym.name.schema.as_ref().map(|s| s.to_lowercase())
                        == Some(schema.to_lowercase())
                    {
                        results.push(Arc::clone(sym));
                    }
                } else {
                    // No schema specified — match any
                    results.push(Arc::clone(sym));
                }
            }
        }
        results
    }

    /// Find all references to a name across the workspace.
    pub fn find_references(&self, name: &str) -> Vec<Arc<SymbolRef>> {
        let lower = name.to_lowercase();
        let mut results = Vec::new();
        for entry in self.references.iter() {
            for r in entry.value() {
                if r.name.name.to_lowercase() == lower {
                    results.push(Arc::clone(r));
                }
            }
        }
        results
    }

    /// Get all definitions in a specific file.
    pub fn file_symbols(&self, uri: &str) -> Vec<Arc<Symbol>> {
        self.definitions
            .get(uri)
            .map(|v| v.value().clone())
            .unwrap_or_default()
    }

    /// Get all definitions across the workspace.
    pub fn all_symbols(&self) -> Vec<Arc<Symbol>> {
        let mut all = Vec::new();
        for entry in self.definitions.iter() {
            all.extend(entry.value().iter().cloned());
        }
        all
    }

    /// Search for symbols matching a query string (for workspace/symbol).
    pub fn search(&self, query: &str) -> Vec<Arc<Symbol>> {
        let lower = query.to_lowercase();
        let mut results = Vec::new();
        for entry in self.by_name.iter() {
            if entry.key().1.contains(&lower) {
                results.extend(entry.value().iter().cloned());
            }
        }
        results
    }
}

impl Default for WorkspaceIndex {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use pg_parse::ParserPool;

    use super::*;

    #[test]
    fn index_and_lookup() {
        let pool = ParserPool::new();
        let index = WorkspaceIndex::new();

        let sql = "CREATE TABLE users (id int, name text);";
        let mut guard = pool.acquire(pg_parse::parser::Language::Postgres);
        let tree = guard.parser_mut().parse(sql, None).unwrap();
        drop(guard);

        index.update_file("file:///test.sql", &tree, sql);

        let results = index.find_definitions(SymbolKind::Table, "users");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name.name, "users");
    }

    #[test]
    fn remove_file_cleans_index() {
        let pool = ParserPool::new();
        let index = WorkspaceIndex::new();

        let sql = "CREATE TABLE users (id int);";
        let mut guard = pool.acquire(pg_parse::parser::Language::Postgres);
        let tree = guard.parser_mut().parse(sql, None).unwrap();
        drop(guard);

        index.update_file("file:///test.sql", &tree, sql);
        assert!(!index.find_definitions(SymbolKind::Table, "users").is_empty());

        index.remove_file("file:///test.sql");
        assert!(index.find_definitions(SymbolKind::Table, "users").is_empty());
    }

    #[test]
    fn search_by_query() {
        let pool = ParserPool::new();
        let index = WorkspaceIndex::new();

        let sql = "CREATE TABLE user_accounts (id int);\nCREATE TABLE user_profiles (id int);";
        let mut guard = pool.acquire(pg_parse::parser::Language::Postgres);
        let tree = guard.parser_mut().parse(sql, None).unwrap();
        drop(guard);

        index.update_file("file:///test.sql", &tree, sql);

        let results = index.search("user");
        assert!(results.len() >= 2);
    }
}
