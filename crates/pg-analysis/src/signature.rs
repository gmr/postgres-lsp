use tree_sitter::Node;

use crate::index::WorkspaceIndex;
use crate::resolve;
use crate::symbols::{self, QualifiedName, Symbol, SymbolKind};

/// A parameter extracted from a function definition.
#[derive(Debug, Clone)]
pub struct ParamInfo {
    pub name: Option<String>,
    pub type_name: String,
}

impl ParamInfo {
    pub fn label(&self) -> String {
        match &self.name {
            Some(name) => format!("{name} {}", self.type_name),
            None => self.type_name.clone(),
        }
    }
}

/// Signature information for a function.
#[derive(Debug, Clone)]
pub struct SignatureInfo {
    pub name: String,
    pub params: Vec<ParamInfo>,
    pub return_type: Option<String>,
}

impl SignatureInfo {
    pub fn label(&self) -> String {
        let params = self
            .params
            .iter()
            .map(|p| p.label())
            .collect::<Vec<_>>()
            .join(", ");
        match &self.return_type {
            Some(ret) => format!("{}({}) -> {}", self.name, params, ret),
            None => format!("{}({})", self.name, params),
        }
    }
}

/// Extract parameter information from a function definition's source text
/// by re-parsing its definition tree.
pub fn extract_signature(symbol: &Symbol, pool: &pg_parse::ParserPool) -> Option<SignatureInfo> {
    if symbol.kind != SymbolKind::Function && symbol.kind != SymbolKind::Procedure {
        return None;
    }

    let mut guard = pool.acquire(pg_parse::parser::Language::Postgres);
    let tree = guard.parser_mut().parse(&symbol.definition_text, None)?;
    let root = tree.root_node();

    let func_stmt = find_node_by_kind(root, "CreateFunctionStmt")?;

    let params = extract_params(func_stmt, &symbol.definition_text);
    let return_type = extract_return_type(func_stmt, &symbol.definition_text);

    Some(SignatureInfo {
        name: symbol.name.display(),
        params,
        return_type,
    })
}

/// Find the enclosing func_application node and determine the active parameter index.
///
/// Uses a tree-sitter Point (row, byte_column) for cursor position.
/// Returns `(qualified_name, active_param_index)`.
pub fn find_active_function_call(
    tree: &tree_sitter::Tree,
    source: &str,
    row: usize,
    byte_col: usize,
) -> Option<(QualifiedName, usize)> {
    let point = tree_sitter::Point {
        row,
        column: byte_col,
    };
    let cursor_node = tree.root_node().descendant_for_point_range(point, point)?;
    let cursor_byte = cursor_node.start_byte();

    // Walk up to find the nearest func_application.
    let mut current = Some(cursor_node);
    while let Some(n) = current {
        if n.kind() == "func_application" {
            let name_node = symbols::find_child(n, "func_name")?;
            let qname = symbols::extract_func_name(name_node, source)?;
            let param_index = count_commas_before(n, cursor_byte);
            return Some((qname, param_index));
        }
        current = n.parent();
    }
    None
}

/// Look up signature help for a function call at the given position.
pub fn signature_help(
    index: &WorkspaceIndex,
    pool: &pg_parse::ParserPool,
    tree: &tree_sitter::Tree,
    source: &str,
    row: usize,
    byte_col: usize,
) -> Option<(SignatureInfo, usize)> {
    let (name, active_param) = find_active_function_call(tree, source, row, byte_col)?;
    let defs = resolve::resolve_name(index, &name);

    let func_sym = defs
        .iter()
        .find(|s| s.kind == SymbolKind::Function || s.kind == SymbolKind::Procedure)?;

    let sig = extract_signature(func_sym, pool)?;
    Some((sig, active_param))
}

/// Count commas before the byte offset within a func_application's argument list.
/// Iterates only the direct children of func_arg_list — no deep recursion needed.
fn count_commas_before(func_app: Node, byte_offset: usize) -> usize {
    // func_arg_list is a left-recursive list; commas are direct children.
    fn count_in_list(node: Node, byte_offset: usize) -> usize {
        let mut count = 0;
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "," && child.start_byte() < byte_offset {
                count += 1;
            } else if child.kind() == "func_arg_list" {
                count += count_in_list(child, byte_offset);
            }
        }
        count
    }

    let mut cursor = func_app.walk();
    for child in func_app.children(&mut cursor) {
        if child.kind() == "func_arg_list" {
            return count_in_list(child, byte_offset);
        }
    }
    0
}

/// Extract parameters from a CreateFunctionStmt node.
fn extract_params(func_stmt: Node, source: &str) -> Vec<ParamInfo> {
    let mut params = Vec::new();
    collect_params(func_stmt, source, &mut params);
    params
}

fn collect_params(node: Node, source: &str, params: &mut Vec<ParamInfo>) {
    if node.kind() == "func_arg" {
        let param_name =
            symbols::find_child(node, "param_name").and_then(|n| symbols::leaf_text(n, source));
        let type_name = symbols::find_child(node, "func_type")
            .map(|n| node_full_text(n, source))
            .unwrap_or_else(|| "unknown".to_string());
        params.push(ParamInfo {
            name: param_name,
            type_name,
        });
        return;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_params(child, source, params);
    }
}

/// Extract the return type from a CreateFunctionStmt.
fn extract_return_type(func_stmt: Node, source: &str) -> Option<String> {
    let func_return = find_node_by_kind(func_stmt, "func_return")?;
    let func_type = find_node_by_kind(func_return, "func_type")?;
    Some(node_full_text(func_type, source))
}

/// Get the full text of a node, preserving multi-token types like
/// `double precision`, `timestamp with time zone`, `numeric(10,2)`, etc.
fn node_full_text(node: Node, source: &str) -> String {
    node.utf8_text(source.as_bytes())
        .unwrap_or("unknown")
        .trim()
        .to_string()
}

fn find_node_by_kind<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
    if node.kind() == kind {
        return Some(node);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(found) = find_node_by_kind(child, kind) {
            return Some(found);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use pg_parse::ParserPool;

    use super::*;

    fn make_index_with_func() -> (WorkspaceIndex, String) {
        let pool = ParserPool::new();
        let sql = "CREATE FUNCTION my_func(a int, b text, c int) RETURNS void LANGUAGE sql AS 'SELECT 1';";
        let mut guard = pool.acquire(pg_parse::parser::Language::Postgres);
        let tree = guard.parser_mut().parse(sql, None).unwrap();
        drop(guard);

        let index = WorkspaceIndex::new();
        index.update_file("file:///test.sql", &tree, sql);
        (index, sql.to_string())
    }

    #[test]
    fn extract_signature_from_function() {
        let pool = ParserPool::new();
        let (index, _) = make_index_with_func();
        let syms = index.find_definitions(SymbolKind::Function, "my_func");
        assert_eq!(syms.len(), 1);

        let sig = extract_signature(&syms[0], &pool).unwrap();
        assert_eq!(sig.name, "my_func");
        assert_eq!(sig.params.len(), 3);
        assert_eq!(sig.params[0].name.as_deref(), Some("a"));
        assert_eq!(sig.params[0].type_name, "int");
        assert_eq!(sig.params[1].name.as_deref(), Some("b"));
        assert_eq!(sig.params[1].type_name, "text");
        assert_eq!(sig.params[2].name.as_deref(), Some("c"));
        assert_eq!(sig.params[2].type_name, "int");
        assert!(sig.return_type.is_some());
    }

    #[test]
    fn find_active_param_first() {
        let pool = ParserPool::new();
        let sql = "SELECT my_func(1, 'hello', 42);";
        let mut guard = pool.acquire(pg_parse::parser::Language::Postgres);
        let tree = guard.parser_mut().parse(sql, None).unwrap();
        drop(guard);

        // Cursor inside first argument (byte column 15 = position of "1")
        let result = find_active_function_call(&tree, sql, 0, 15);
        assert!(result.is_some());
        let (name, idx) = result.unwrap();
        assert_eq!(name.display(), "my_func");
        assert_eq!(idx, 0);
    }

    #[test]
    fn find_active_param_second() {
        let pool = ParserPool::new();
        let sql = "SELECT my_func(1, 'hello', 42);";
        let mut guard = pool.acquire(pg_parse::parser::Language::Postgres);
        let tree = guard.parser_mut().parse(sql, None).unwrap();
        drop(guard);

        // Cursor inside second argument (byte column 19 = inside "'hello'")
        let result = find_active_function_call(&tree, sql, 0, 19);
        assert!(result.is_some());
        let (name, idx) = result.unwrap();
        assert_eq!(name.display(), "my_func");
        assert_eq!(idx, 1);
    }

    #[test]
    fn find_active_param_third() {
        let pool = ParserPool::new();
        let sql = "SELECT my_func(1, 'hello', 42);";
        let mut guard = pool.acquire(pg_parse::parser::Language::Postgres);
        let tree = guard.parser_mut().parse(sql, None).unwrap();
        drop(guard);

        // Cursor inside third argument (byte column 27 = position of "42")
        let result = find_active_function_call(&tree, sql, 0, 27);
        assert!(result.is_some());
        let (name, idx) = result.unwrap();
        assert_eq!(name.display(), "my_func");
        assert_eq!(idx, 2);
    }

    #[test]
    fn signature_help_full() {
        let pool = ParserPool::new();
        let (index, _) = make_index_with_func();

        let call_sql = "SELECT my_func(1, 'hello', 42);";
        let mut guard = pool.acquire(pg_parse::parser::Language::Postgres);
        let tree = guard.parser_mut().parse(call_sql, None).unwrap();
        drop(guard);

        // Cursor at second argument (byte column 19)
        let result = signature_help(&index, &pool, &tree, call_sql, 0, 19);
        assert!(result.is_some());
        let (sig, active) = result.unwrap();
        assert_eq!(sig.name, "my_func");
        assert_eq!(sig.params.len(), 3);
        assert_eq!(active, 1);
    }

    #[test]
    fn no_signature_outside_function_call() {
        let pool = ParserPool::new();
        let sql = "SELECT 1;";
        let mut guard = pool.acquire(pg_parse::parser::Language::Postgres);
        let tree = guard.parser_mut().parse(sql, None).unwrap();
        drop(guard);

        let index = WorkspaceIndex::new();
        let result = signature_help(&index, &pool, &tree, sql, 0, 7);
        assert!(result.is_none());
    }
}
