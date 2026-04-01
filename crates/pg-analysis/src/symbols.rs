use tree_sitter::{Node, Tree};

/// The kind of a SQL symbol definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SymbolKind {
    Schema,
    Table,
    View,
    MaterializedView,
    Column,
    Function,
    Procedure,
    Trigger,
    Index,
    Sequence,
    Type,
    Domain,
    Extension,
    Role,
    Policy,
    Publication,
    Subscription,
    ForeignTable,
}

impl SymbolKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Schema => "schema",
            Self::Table => "table",
            Self::View => "view",
            Self::MaterializedView => "materialized view",
            Self::Column => "column",
            Self::Function => "function",
            Self::Procedure => "procedure",
            Self::Trigger => "trigger",
            Self::Index => "index",
            Self::Sequence => "sequence",
            Self::Type => "type",
            Self::Domain => "domain",
            Self::Extension => "extension",
            Self::Role => "role",
            Self::Policy => "policy",
            Self::Publication => "publication",
            Self::Subscription => "subscription",
            Self::ForeignTable => "foreign table",
        }
    }
}

/// A fully qualified name (schema.name).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct QualifiedName {
    pub schema: Option<String>,
    pub name: String,
}

impl QualifiedName {
    pub fn new(name: String) -> Self {
        Self { schema: None, name }
    }

    pub fn with_schema(schema: String, name: String) -> Self {
        Self {
            schema: Some(schema),
            name,
        }
    }

    pub fn display(&self) -> String {
        match &self.schema {
            Some(s) => format!("{s}.{}", self.name),
            None => self.name.clone(),
        }
    }
}

/// A symbol definition extracted from the parse tree.
#[derive(Debug, Clone)]
pub struct Symbol {
    pub kind: SymbolKind,
    pub name: QualifiedName,
    pub uri: String,
    /// Full statement range.
    pub start_byte: usize,
    pub end_byte: usize,
    pub start_line: usize,
    pub start_col: usize,
    pub end_line: usize,
    pub end_col: usize,
    /// Name-only range (for rename operations).
    pub name_start_line: usize,
    pub name_start_col: usize,
    pub name_end_line: usize,
    pub name_end_col: usize,
    pub definition_text: String,
    pub children: Vec<Symbol>,
}

/// A reference to a symbol (usage, not definition).
#[derive(Debug, Clone)]
pub struct SymbolRef {
    /// The qualified name being referenced.
    pub name: QualifiedName,
    /// The URI of the document containing this reference.
    pub uri: String,
    /// Byte range within the source.
    pub start_byte: usize,
    pub end_byte: usize,
    /// Line/column range (0-based).
    pub start_line: usize,
    pub start_col: usize,
    pub end_line: usize,
    pub end_col: usize,
}

/// Extract all symbol definitions from a parsed document.
pub fn extract_symbols(tree: &Tree, source: &str, uri: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    collect_definitions(tree.root_node(), source, uri, &mut symbols);
    symbols
}

/// Recursively walk the tree looking for DDL statement nodes.
///
/// The tree structure is: source_file -> toplevel_stmt -> stmt -> CreateStmt/etc.
/// We recurse through wrapper nodes to find the actual statement types.
fn collect_definitions(node: Node, source: &str, uri: &str, symbols: &mut Vec<Symbol>) {
    if let Some(symbol) = try_extract_definition(node, source, uri) {
        symbols.push(symbol);
        return; // Don't recurse into statements we've already extracted.
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_definitions(child, source, uri, symbols);
    }
}

/// Try to extract a symbol definition from a node.
fn try_extract_definition(node: Node, source: &str, uri: &str) -> Option<Symbol> {
    // Each arm returns (kind, name, name_node) where name_node has the position of the name.
    let result: Option<(SymbolKind, QualifiedName, Node)> = match node.kind() {
        "CreateStmt" => {
            let kind = if has_child_kind(node, "kw_materialized") {
                SymbolKind::MaterializedView
            } else {
                SymbolKind::Table
            };
            find_descendant(node, "qualified_name")
                .and_then(|n| extract_qualified_name_node(n, source).map(|qn| (kind, qn, n)))
        }
        "ViewStmt" => find_descendant(node, "qualified_name")
            .and_then(|n| extract_qualified_name_node(n, source).map(|qn| (SymbolKind::View, qn, n))),
        "CreateFunctionStmt" => {
            let kind = if has_child_kind(node, "kw_procedure") {
                SymbolKind::Procedure
            } else {
                SymbolKind::Function
            };
            find_descendant(node, "func_name")
                .and_then(|n| extract_func_name(n, source).map(|qn| (kind, qn, n)))
        }
        "IndexStmt" => extract_name_or_col_id(node, source, SymbolKind::Index),
        "CreateSeqStmt" => find_descendant(node, "qualified_name")
            .and_then(|n| extract_qualified_name_node(n, source).map(|qn| (SymbolKind::Sequence, qn, n))),
        "CreateSchemaStmt" => find_descendant(node, "ColId")
            .and_then(|n| extract_leaf_name(n, source).map(|qn| (SymbolKind::Schema, qn, n))),
        "CompositeTypeStmt" | "CreateEnumStmt" | "CreateRangeStmt" => {
            find_descendant(node, "any_name")
                .and_then(|n| extract_qualified_name_node(n, source).map(|qn| (SymbolKind::Type, qn, n)))
                .or_else(|| {
                    find_descendant(node, "qualified_name")
                        .and_then(|n| extract_qualified_name_node(n, source).map(|qn| (SymbolKind::Type, qn, n)))
                })
        }
        "CreateDomainStmt" => find_descendant(node, "any_name")
            .and_then(|n| extract_qualified_name_node(n, source).map(|qn| (SymbolKind::Domain, qn, n))),
        "CreateExtensionStmt" => extract_name_or_col_id(node, source, SymbolKind::Extension),
        "CreateTrigStmt" => extract_name_or_col_id(node, source, SymbolKind::Trigger),
        "CreateRoleStmt" => find_descendant(node, "RoleId")
            .and_then(|n| extract_leaf_name(n, source).map(|qn| (SymbolKind::Role, qn, n)))
            .or_else(|| {
                find_descendant(node, "ColId")
                    .and_then(|n| extract_leaf_name(n, source).map(|qn| (SymbolKind::Role, qn, n)))
            }),
        "CreatePolicyStmt" => find_descendant(node, "name")
            .and_then(|n| extract_leaf_name(n, source).map(|qn| (SymbolKind::Policy, qn, n))),
        "CreatePublicationStmt" => find_descendant(node, "ColId")
            .and_then(|n| extract_leaf_name(n, source).map(|qn| (SymbolKind::Publication, qn, n))),
        "CreateSubscriptionStmt" => find_descendant(node, "ColId")
            .and_then(|n| extract_leaf_name(n, source).map(|qn| (SymbolKind::Subscription, qn, n))),
        "CreateForeignTableStmt" => find_descendant(node, "qualified_name")
            .and_then(|n| extract_qualified_name_node(n, source).map(|qn| (SymbolKind::ForeignTable, qn, n))),
        _ => None,
    };

    let (kind, name, name_node) = result?;
    let def_text = node.utf8_text(source.as_bytes()).unwrap_or("").to_string();

    let mut symbol = Symbol {
        kind,
        name,
        uri: uri.to_string(),
        start_byte: node.start_byte(),
        end_byte: node.end_byte(),
        start_line: node.start_position().row,
        start_col: node.start_position().column,
        end_line: node.end_position().row,
        end_col: node.end_position().column,
        name_start_line: name_node.start_position().row,
        name_start_col: name_node.start_position().column,
        name_end_line: name_node.end_position().row,
        name_end_col: name_node.end_position().column,
        definition_text: def_text,
        children: Vec::new(),
    };

    if kind == SymbolKind::Table {
        symbol.children = extract_columns(node, source, uri);
    }

    Some(symbol)
}

/// Helper: try "name" descendant then "ColId" descendant for a given kind.
fn extract_name_or_col_id<'a>(
    node: Node<'a>,
    source: &str,
    kind: SymbolKind,
) -> Option<(SymbolKind, QualifiedName, Node<'a>)> {
    find_descendant(node, "name")
        .and_then(|n| extract_leaf_name(n, source).map(|qn| (kind, qn, n)))
        .or_else(|| {
            find_descendant(node, "ColId")
                .and_then(|n| extract_leaf_name(n, source).map(|qn| (kind, qn, n)))
        })
}

/// Extract a QualifiedName from a `qualified_name` or `any_name` node.
///
/// Tree structure for unqualified: `qualified_name` -> `ColId` -> `identifier`
/// Tree structure for qualified: `qualified_name` -> `ColId` + `indirection` ->
///   `indirection_el` -> `.` + `attr_name` -> `ColLabel` -> `identifier`
fn extract_qualified_name_node(node: Node, source: &str) -> Option<QualifiedName> {
    let mut first_name = None;
    let mut indirection_name = None;

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "ColId" => {
                first_name = leaf_text(child, source);
            }
            "indirection" => {
                // Look for attr_name inside indirection_el
                let mut ic = child.walk();
                for indirection_child in child.children(&mut ic) {
                    if indirection_child.kind() == "indirection_el" {
                        let mut ec = indirection_child.walk();
                        for el_child in indirection_child.children(&mut ec) {
                            if el_child.kind() == "attr_name" {
                                indirection_name = leaf_text(el_child, source);
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    match (first_name, indirection_name) {
        (Some(schema), Some(name)) => Some(QualifiedName::with_schema(schema, name)),
        (Some(name), None) => Some(QualifiedName::new(name)),
        _ => None,
    }
}

/// Extract a name from a `func_name` node.
///
/// Tree structure: `func_name` -> `type_function_name` -> (`identifier` | `unreserved_keyword` -> `kw_*`)
fn extract_func_name(node: Node, source: &str) -> Option<QualifiedName> {
    // func_name may contain qualified names too
    if let Some(indirection) = find_child(node, "indirection") {
        // Schema-qualified function name
        let schema = find_child(node, "ColId").and_then(|n| leaf_text(n, source));
        let mut name = None;
        let mut ic = indirection.walk();
        for child in indirection.children(&mut ic) {
            if child.kind() == "indirection_el" {
                let mut ec = child.walk();
                for el_child in child.children(&mut ec) {
                    if el_child.kind() == "attr_name" {
                        name = leaf_text(el_child, source);
                    }
                }
            }
        }
        if let (Some(s), Some(n)) = (schema, name) { return Some(QualifiedName::with_schema(s, n)) }
    }

    // Simple function name: func_name -> type_function_name -> identifier/kw_*
    leaf_text(node, source).map(QualifiedName::new)
}

/// Extract a simple name from a leaf-like node.
fn extract_leaf_name(node: Node, source: &str) -> Option<QualifiedName> {
    leaf_text(node, source).map(QualifiedName::new)
}

/// Get the text content of a node, recursing to find the deepest identifier/keyword text.
fn leaf_text(node: Node, source: &str) -> Option<String> {
    // If it's a leaf, return its text directly.
    if node.child_count() == 0 {
        let text = node.utf8_text(source.as_bytes()).ok()?;
        return Some(text.trim().replace('"', ""));
    }

    // Recurse into children to find identifier or keyword text.
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "identifier" => {
                return child
                    .utf8_text(source.as_bytes())
                    .ok()
                    .map(|t| t.trim().replace('"', ""));
            }
            _ if child.kind().starts_with("kw_") => {
                return child
                    .utf8_text(source.as_bytes())
                    .ok()
                    .map(|t| t.trim().to_string());
            }
            _ => {
                if let Some(text) = leaf_text(child, source) {
                    return Some(text);
                }
            }
        }
    }
    None
}

/// Find a direct child node of the given kind.
fn find_child<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    node.children(&mut cursor).find(|c| c.kind() == kind)
}

/// Find a descendant node of the given kind (breadth-first, limited depth).
fn find_descendant<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
    // Direct children first
    if let Some(child) = find_child(node, kind) {
        return Some(child);
    }
    // Then grandchildren
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(grandchild) = find_child(child, kind) {
            return Some(grandchild);
        }
    }
    None
}

/// Check if a node has a direct child of the given kind.
fn has_child_kind(node: Node, kind: &str) -> bool {
    find_child(node, kind).is_some()
}

/// Extract column symbols from a CREATE TABLE statement.
fn extract_columns(table_node: Node, source: &str, uri: &str) -> Vec<Symbol> {
    let mut columns = Vec::new();
    collect_column_defs(table_node, source, uri, &mut columns);
    columns
}

fn collect_column_defs(node: Node, source: &str, uri: &str, columns: &mut Vec<Symbol>) {
    if node.kind() == "columnDef" {
        if let Some(col_id) = find_descendant(node, "ColId")
            && let Some(name) = leaf_text(col_id, source) {
                let def_text = node.utf8_text(source.as_bytes()).unwrap_or("").to_string();
                columns.push(Symbol {
                    kind: SymbolKind::Column,
                    name: QualifiedName::new(name),
                    uri: uri.to_string(),
                    start_byte: node.start_byte(),
                    end_byte: node.end_byte(),
                    start_line: node.start_position().row,
                    start_col: node.start_position().column,
                    end_line: node.end_position().row,
                    end_col: node.end_position().column,
                    name_start_line: col_id.start_position().row,
                    name_start_col: col_id.start_position().column,
                    name_end_line: col_id.end_position().row,
                    name_end_col: col_id.end_position().column,
                    definition_text: def_text,
                    children: Vec::new(),
                });
            }
        return;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_column_defs(child, source, uri, columns);
    }
}

/// Extract all symbol references (usages) from a parsed document.
pub fn extract_references(tree: &Tree, source: &str, uri: &str) -> Vec<SymbolRef> {
    let mut refs = Vec::new();
    collect_references(tree.root_node(), source, uri, &mut refs);
    refs
}

fn collect_references(node: Node, source: &str, uri: &str, refs: &mut Vec<SymbolRef>) {
    match node.kind() {
        "columnref" | "relation_expr" | "func_application" => {
            // Extract the name from within this reference node.
            let name = match node.kind() {
                "func_application" => {
                    find_descendant(node, "func_name")
                        .and_then(|n| extract_func_name(n, source))
                }
                _ => {
                    // For columnref/relation_expr, get the full text as a qualified name
                    let text = node.utf8_text(source.as_bytes()).unwrap_or("").trim().replace('"', "");
                    if text.is_empty() {
                        None
                    } else if let Some((schema, name)) = text.split_once('.') {
                        Some(QualifiedName::with_schema(
                            schema.to_string(),
                            name.to_string(),
                        ))
                    } else {
                        Some(QualifiedName::new(text))
                    }
                }
            };

            if let Some(name) = name {
                refs.push(SymbolRef {
                    name,
                    uri: uri.to_string(),
                    start_byte: node.start_byte(),
                    end_byte: node.end_byte(),
                    start_line: node.start_position().row,
                    start_col: node.start_position().column,
                    end_line: node.end_position().row,
                    end_col: node.end_position().column,
                });
                // Don't recurse into reference nodes we've already extracted.
                return;
            }
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_references(child, source, uri, refs);
    }
}

#[cfg(test)]
mod tests {
    use pg_parse::ParserPool;

    use super::*;

    fn parse_and_extract(sql: &str) -> Vec<Symbol> {
        let pool = ParserPool::new();
        let mut guard = pool.acquire(pg_parse::parser::Language::Postgres);
        let tree = guard.parser_mut().parse(sql, None).unwrap();
        extract_symbols(&tree, sql, "test.sql")
    }

    #[test]
    fn extract_create_table() {
        let symbols = parse_and_extract("CREATE TABLE users (id int, name text);");
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].kind, SymbolKind::Table);
        assert_eq!(symbols[0].name.name, "users");
        assert_eq!(symbols[0].children.len(), 2);
        assert_eq!(symbols[0].children[0].name.name, "id");
        assert_eq!(symbols[0].children[1].name.name, "name");
    }

    #[test]
    fn extract_create_function() {
        let symbols = parse_and_extract(
            "CREATE FUNCTION add(a int, b int) RETURNS int LANGUAGE sql AS 'SELECT a + b';",
        );
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].kind, SymbolKind::Function);
        assert_eq!(symbols[0].name.name, "add");
    }

    #[test]
    fn extract_schema_qualified() {
        let symbols = parse_and_extract("CREATE TABLE public.users (id int);");
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name.schema.as_deref(), Some("public"));
        assert_eq!(symbols[0].name.name, "users");
    }

    #[test]
    fn extract_references_from_select() {
        let pool = ParserPool::new();
        let sql = "SELECT id, name FROM users WHERE id > 0;";
        let mut guard = pool.acquire(pg_parse::parser::Language::Postgres);
        let tree = guard.parser_mut().parse(sql, None).unwrap();
        let refs = extract_references(&tree, sql, "test.sql");
        assert!(!refs.is_empty(), "should find references in SELECT");
    }
}
