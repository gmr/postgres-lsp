use tree_sitter::Node;

use crate::index::WorkspaceIndex;
use crate::resolve;
use crate::symbols::{QualifiedName, Symbol, SymbolKind};

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
pub fn extract_signature(symbol: &Symbol) -> Option<SignatureInfo> {
    if symbol.kind != SymbolKind::Function && symbol.kind != SymbolKind::Procedure {
        return None;
    }

    let mut parser = tree_sitter::Parser::new();
    let lang: tree_sitter::Language = tree_sitter_postgres::LANGUAGE.into();
    parser.set_language(&lang).ok()?;
    let tree = parser.parse(&symbol.definition_text, None)?;
    let root = tree.root_node();

    // Find the CreateFunctionStmt inside the parse tree.
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
/// `byte_offset` is the cursor position in byte offset within the source.
/// Returns `(func_name, active_param_index)`.
pub fn find_active_function_call(
    tree: &tree_sitter::Tree,
    source: &str,
    byte_offset: usize,
) -> Option<(String, usize)> {
    let root = tree.root_node();
    let node = root.descendant_for_byte_range(byte_offset, byte_offset)?;

    // Walk up to find the nearest func_application.
    let mut current = Some(node);
    while let Some(n) = current {
        if n.kind() == "func_application" {
            let func_name = extract_func_name_text(n, source)?;
            let param_index = count_commas_before(n, byte_offset);
            return Some((func_name, param_index));
        }
        current = n.parent();
    }
    None
}

/// Look up signature help for a function call at the given position.
pub fn signature_help(
    index: &WorkspaceIndex,
    tree: &tree_sitter::Tree,
    source: &str,
    byte_offset: usize,
) -> Option<(SignatureInfo, usize)> {
    let (func_name, active_param) = find_active_function_call(tree, source, byte_offset)?;
    let name = QualifiedName::new(func_name);
    let symbols = resolve::resolve_name(index, &name);

    let func_sym = symbols
        .iter()
        .find(|s| s.kind == SymbolKind::Function || s.kind == SymbolKind::Procedure)?;

    let sig = extract_signature(func_sym)?;
    Some((sig, active_param))
}

fn extract_func_name_text(func_app: Node, source: &str) -> Option<String> {
    let mut cursor = func_app.walk();
    for child in func_app.children(&mut cursor) {
        if child.kind() == "func_name" {
            return leaf_text(child, source);
        }
    }
    None
}

/// Count commas before the byte offset within a func_application's argument list.
fn count_commas_before(func_app: Node, byte_offset: usize) -> usize {
    let mut count = 0;
    count_commas_recursive(func_app, byte_offset, &mut count);
    count
}

fn count_commas_recursive(node: Node, byte_offset: usize, count: &mut usize) {
    if node.kind() == ","
        && node.start_byte() < byte_offset
        && node.parent().is_some_and(|p| p.kind() == "func_arg_list")
    {
        *count += 1;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.start_byte() <= byte_offset {
            count_commas_recursive(child, byte_offset, count);
        }
    }
}

/// Extract parameters from a CreateFunctionStmt node.
fn extract_params(func_stmt: Node, source: &str) -> Vec<ParamInfo> {
    let mut params = Vec::new();
    collect_params(func_stmt, source, &mut params);
    params
}

fn collect_params(node: Node, source: &str, params: &mut Vec<ParamInfo>) {
    if node.kind() == "func_arg" {
        let param_name = find_child_text(node, "param_name", source);
        let type_name =
            find_child_text(node, "func_type", source).unwrap_or_else(|| "unknown".to_string());
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
    let text = func_return.utf8_text(source.as_bytes()).ok()?;
    Some(text.trim().to_string())
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

fn find_child_text(node: Node, kind: &str, source: &str) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == kind {
            return leaf_text(child, source);
        }
    }
    None
}

fn leaf_text(node: Node, source: &str) -> Option<String> {
    if node.child_count() == 0 {
        return node
            .utf8_text(source.as_bytes())
            .ok()
            .map(|s| s.trim().to_string());
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "identifier" || child.kind().starts_with("kw_") {
            return child
                .utf8_text(source.as_bytes())
                .ok()
                .map(|s| s.trim().to_string());
        }
        if let Some(text) = leaf_text(child, source) {
            return Some(text);
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
        let (index, _) = make_index_with_func();
        let syms = index.find_definitions(SymbolKind::Function, "my_func");
        assert_eq!(syms.len(), 1);

        let sig = extract_signature(&syms[0]).unwrap();
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

        // Cursor inside first argument (position of "1")
        let result = find_active_function_call(&tree, sql, 15);
        assert!(result.is_some());
        let (name, idx) = result.unwrap();
        assert_eq!(name, "my_func");
        assert_eq!(idx, 0);
    }

    #[test]
    fn find_active_param_second() {
        let pool = ParserPool::new();
        let sql = "SELECT my_func(1, 'hello', 42);";
        let mut guard = pool.acquire(pg_parse::parser::Language::Postgres);
        let tree = guard.parser_mut().parse(sql, None).unwrap();
        drop(guard);

        // Cursor inside second argument (position of "'hello'")
        let result = find_active_function_call(&tree, sql, 19);
        assert!(result.is_some());
        let (name, idx) = result.unwrap();
        assert_eq!(name, "my_func");
        assert_eq!(idx, 1);
    }

    #[test]
    fn find_active_param_third() {
        let pool = ParserPool::new();
        let sql = "SELECT my_func(1, 'hello', 42);";
        let mut guard = pool.acquire(pg_parse::parser::Language::Postgres);
        let tree = guard.parser_mut().parse(sql, None).unwrap();
        drop(guard);

        // Cursor inside third argument (position of "42")
        let result = find_active_function_call(&tree, sql, 27);
        assert!(result.is_some());
        let (name, idx) = result.unwrap();
        assert_eq!(name, "my_func");
        assert_eq!(idx, 2);
    }

    #[test]
    fn signature_help_full() {
        let (index, _) = make_index_with_func();

        let pool = ParserPool::new();
        let call_sql = "SELECT my_func(1, 'hello', 42);";
        let mut guard = pool.acquire(pg_parse::parser::Language::Postgres);
        let tree = guard.parser_mut().parse(call_sql, None).unwrap();
        drop(guard);

        // Cursor at second argument
        let result = signature_help(&index, &tree, call_sql, 19);
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
        let result = signature_help(&index, &tree, sql, 7);
        assert!(result.is_none());
    }
}
