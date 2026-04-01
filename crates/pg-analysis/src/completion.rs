use crate::index::WorkspaceIndex;
use crate::symbols::SymbolKind;

#[derive(Debug, Clone)]
pub struct CompletionItem {
    pub label: String,
    pub kind: CompletionKind,
    pub detail: Option<String>,
    pub documentation: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionKind {
    Keyword,
    Table,
    View,
    Column,
    Function,
    Procedure,
    Type,
    Schema,
    Sequence,
}

const SQL_KEYWORDS: &[&str] = &[
    "ADD",
    "ALL",
    "ALTER",
    "ANALYZE",
    "AND",
    "AS",
    "ASC",
    "BEGIN",
    "BETWEEN",
    "BY",
    "CASCADE",
    "CASE",
    "CHECK",
    "COLUMN",
    "COMMIT",
    "CONCURRENTLY",
    "CONSTRAINT",
    "COPY",
    "COST",
    "CREATE",
    "CROSS",
    "DATABASE",
    "DEALLOCATE",
    "DEFAULT",
    "DEFINER",
    "DELETE",
    "DESC",
    "DISTINCT",
    "DOMAIN",
    "DROP",
    "ELSE",
    "END",
    "EXCEPT",
    "EXCLUDING",
    "EXECUTE",
    "EXISTS",
    "EXPLAIN",
    "EXTENSION",
    "FALSE",
    "FETCH",
    "FOREIGN",
    "FROM",
    "FULL",
    "FUNCTION",
    "GRANT",
    "GROUP",
    "HAVING",
    "IF",
    "ILIKE",
    "IMMUTABLE",
    "IN",
    "INCLUDING",
    "INDEX",
    "INHERITS",
    "INNER",
    "INSERT",
    "INTERSECT",
    "INTO",
    "INVOKER",
    "IS",
    "JOIN",
    "KEY",
    "LANGUAGE",
    "LEFT",
    "LIKE",
    "LIMIT",
    "LISTEN",
    "MATERIALIZED",
    "NOT",
    "NOTIFY",
    "NULL",
    "OFFSET",
    "ON",
    "OR",
    "ORDER",
    "OUTER",
    "OWNER",
    "PARALLEL",
    "PARTITION",
    "PREPARE",
    "PRIMARY",
    "PROCEDURE",
    "REFERENCES",
    "REFRESH",
    "REINDEX",
    "RENAME",
    "REPLACE",
    "RESTRICT",
    "RESTRICTED",
    "RETURNING",
    "RETURNS",
    "REVOKE",
    "RIGHT",
    "ROLLBACK",
    "ROWS",
    "SAFE",
    "SCHEMA",
    "SECURITY",
    "SELECT",
    "SEQUENCE",
    "SET",
    "STABLE",
    "STRICT",
    "TABLE",
    "TABLESPACE",
    "THEN",
    "TO",
    "TRIGGER",
    "TRUE",
    "TRUNCATE",
    "TYPE",
    "UNION",
    "UNIQUE",
    "UNSAFE",
    "UPDATE",
    "USING",
    "VACUUM",
    "VALUES",
    "VIEW",
    "VOLATILE",
    "WHEN",
    "WHERE",
    "WITH",
];

pub fn completions(index: &WorkspaceIndex, context: &CompletionContext) -> Vec<CompletionItem> {
    let mut items = Vec::new();

    match context {
        CompletionContext::AfterFrom | CompletionContext::AfterJoin => {
            for sym in index.all_symbols() {
                match sym.kind {
                    SymbolKind::Table
                    | SymbolKind::View
                    | SymbolKind::MaterializedView
                    | SymbolKind::ForeignTable => {
                        items.push(CompletionItem {
                            label: sym.name.display(),
                            kind: CompletionKind::Table,
                            detail: Some(sym.kind.label().to_string()),
                            documentation: None,
                        });
                    }
                    _ => {}
                }
            }
        }
        CompletionContext::AfterSelect | CompletionContext::ColumnPosition => {
            let all = index.all_symbols();
            for sym in &all {
                if sym.kind == SymbolKind::Table
                    || sym.kind == SymbolKind::View
                    || sym.kind == SymbolKind::MaterializedView
                {
                    for col in &sym.children {
                        items.push(CompletionItem {
                            label: col.name.name.clone(),
                            kind: CompletionKind::Column,
                            detail: Some(format!("{}.{}", sym.name.display(), col.name.name)),
                            documentation: None,
                        });
                    }
                }
                if sym.kind == SymbolKind::Function {
                    items.push(CompletionItem {
                        label: format!("{}()", sym.name.name),
                        kind: CompletionKind::Function,
                        detail: Some("function".to_string()),
                        documentation: None,
                    });
                }
            }
        }
        CompletionContext::General => {
            for kw in SQL_KEYWORDS {
                items.push(CompletionItem {
                    label: (*kw).to_string(),
                    kind: CompletionKind::Keyword,
                    detail: None,
                    documentation: None,
                });
            }
            for sym in index.all_symbols() {
                items.push(CompletionItem {
                    label: sym.name.display(),
                    kind: symbol_kind_to_completion(sym.kind),
                    detail: Some(sym.kind.label().to_string()),
                    documentation: None,
                });
            }
        }
    }

    items
}

#[derive(Debug, Clone)]
pub enum CompletionContext {
    AfterFrom,
    AfterJoin,
    AfterSelect,
    ColumnPosition,
    General,
}

fn symbol_kind_to_completion(kind: SymbolKind) -> CompletionKind {
    match kind {
        SymbolKind::Table | SymbolKind::ForeignTable => CompletionKind::Table,
        SymbolKind::View | SymbolKind::MaterializedView => CompletionKind::View,
        SymbolKind::Column => CompletionKind::Column,
        SymbolKind::Function => CompletionKind::Function,
        SymbolKind::Procedure => CompletionKind::Procedure,
        SymbolKind::Type | SymbolKind::Domain => CompletionKind::Type,
        SymbolKind::Schema => CompletionKind::Schema,
        SymbolKind::Sequence => CompletionKind::Sequence,
        SymbolKind::Index
        | SymbolKind::Trigger
        | SymbolKind::Extension
        | SymbolKind::Role
        | SymbolKind::Policy
        | SymbolKind::Publication
        | SymbolKind::Subscription
        | SymbolKind::Variable
        | SymbolKind::Cursor => CompletionKind::Table,
    }
}
