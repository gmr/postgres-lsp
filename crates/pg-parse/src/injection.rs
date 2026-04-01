use tree_sitter::{Node, Tree};

use crate::parser::{Language, ParserPool};

/// An injected parse region — a sub-tree parsed with a different language.
///
/// For example, a PL/pgSQL function body inside a CREATE FUNCTION statement
/// is parsed with the plpgsql grammar, and SQL expressions within PL/pgSQL
/// are injected back with the postgres grammar.
#[derive(Debug)]
pub struct InjectedRegion {
    /// Byte offset of this region within the parent document.
    pub parent_start_byte: usize,
    /// The language of the injected region.
    pub language: Language,
    /// The parse tree for the injected content.
    pub tree: Tree,
    /// The source text of the injected region.
    pub text: String,
}

impl InjectedRegion {
    /// Convert a byte offset within this injected region to the parent document offset.
    pub fn to_parent_byte(&self, local_byte: usize) -> usize {
        self.parent_start_byte + local_byte
    }

    /// Convert a parent document byte offset to a local offset within this region.
    /// Returns None if the parent offset is outside this region.
    pub fn to_local_byte(&self, parent_byte: usize) -> Option<usize> {
        if parent_byte >= self.parent_start_byte
            && parent_byte < self.parent_start_byte + self.text.len()
        {
            Some(parent_byte - self.parent_start_byte)
        } else {
            None
        }
    }
}

/// Find dollar-quoted string bodies in CREATE FUNCTION/PROCEDURE statements
/// and parse them with the PL/pgSQL grammar.
///
/// This handles the first level of injection: SQL -> PL/pgSQL.
pub fn extract_plpgsql_bodies(
    tree: &Tree,
    source: &str,
    pool: &ParserPool,
) -> Vec<InjectedRegion> {
    let mut regions = Vec::new();
    find_dollar_quoted_bodies(tree.root_node(), source, pool, &mut regions);
    regions
}

fn find_dollar_quoted_bodies(
    node: Node,
    source: &str,
    pool: &ParserPool,
    regions: &mut Vec<InjectedRegion>,
) {
    // Look for dollar_quoted_string nodes inside function/procedure definitions
    // that have LANGUAGE plpgsql.
    if node.kind() == "dollar_quoted_string"
        && is_inside_plpgsql_function(node, source)
        && let Some(body) = extract_dollar_quote_content(node, source) {
            let mut guard = pool.acquire(Language::PlPgSql);
            if let Some(tree) = guard.parser_mut().parse(&body.text, None) {
                regions.push(InjectedRegion {
                    parent_start_byte: body.start_byte,
                    language: Language::PlPgSql,
                    tree,
                    text: body.text,
                });
            }
        }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        find_dollar_quoted_bodies(child, source, pool, regions);
    }
}

/// Check if a node is inside a CREATE FUNCTION/PROCEDURE with LANGUAGE plpgsql.
fn is_inside_plpgsql_function(node: Node, source: &str) -> bool {
    let mut current = node.parent();
    while let Some(parent) = current {
        if parent.kind() == "CreateFunctionStmt" {
            // Look for a LANGUAGE clause that specifies plpgsql
            let mut cursor = parent.walk();
            for child in parent.children(&mut cursor) {
                if child.kind() == "func_body_item" || child.kind() == "createfunc_opt_item" {
                    let text = child
                        .utf8_text(source.as_bytes())
                        .unwrap_or("")
                        .to_uppercase();
                    if text.contains("LANGUAGE") && text.contains("PLPGSQL") {
                        return true;
                    }
                }
                // Also check direct keyword sequences
                if child.kind() == "kw_language" {
                    // The next sibling should be the language name
                    if let Some(next) = child.next_sibling() {
                        let lang = next
                            .utf8_text(source.as_bytes())
                            .unwrap_or("")
                            .to_uppercase();
                        if lang.trim() == "PLPGSQL" {
                            return true;
                        }
                    }
                }
            }
            // Check the full statement text as a fallback
            let stmt_text = parent
                .utf8_text(source.as_bytes())
                .unwrap_or("")
                .to_uppercase();
            return stmt_text.contains("LANGUAGE PLPGSQL")
                || stmt_text.contains("LANGUAGE 'PLPGSQL'");
        }
        current = parent.parent();
    }
    false
}

struct DollarQuoteContent {
    start_byte: usize,
    text: String,
}

/// Extract the content between dollar-quote delimiters.
/// E.g., `$$BEGIN ... END;$$` -> `BEGIN ... END;`
///
/// Note: `rfind` is safe here because tree-sitter already parsed the outer
/// `dollar_quoted_string` node boundary. Any nested dollar-quotes with
/// different tags (e.g., `$inner$...$inner$`) will be part of the content.
fn extract_dollar_quote_content(node: Node, source: &str) -> Option<DollarQuoteContent> {
    let full = node.utf8_text(source.as_bytes()).ok()?;

    // Find the opening delimiter ($$, $tag$, etc.)
    let first_dollar = full.find('$')?;
    let delim_end = full[first_dollar + 1..].find('$')? + first_dollar + 2;
    let delimiter = &full[first_dollar..delim_end];

    // Find the closing delimiter (last occurrence, which is the outer close)
    let content_start = delim_end;
    let content_end = full.rfind(delimiter)?;
    if content_end <= content_start {
        return None;
    }

    let text = full[content_start..content_end].to_string();
    let start_byte = node.start_byte() + content_start;

    Some(DollarQuoteContent { start_byte, text })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_plpgsql_from_create_function() {
        let pool = ParserPool::new();
        let source = r#"CREATE FUNCTION test() RETURNS void LANGUAGE plpgsql AS $$
BEGIN
  RAISE NOTICE 'hello';
END;
$$;"#;

        let mut guard = pool.acquire(Language::Postgres);
        let tree = guard.parser_mut().parse(source, None).unwrap();
        drop(guard);

        let regions = extract_plpgsql_bodies(&tree, source, &pool);
        assert!(!regions.is_empty(), "should find PL/pgSQL body");
        assert_eq!(regions[0].language, Language::PlPgSql);
        assert!(regions[0].text.contains("BEGIN"));
        assert!(regions[0].text.contains("RAISE NOTICE"));
    }

    #[test]
    fn offset_mapping() {
        let region = InjectedRegion {
            parent_start_byte: 100,
            language: Language::PlPgSql,
            tree: {
                let pool = ParserPool::new();
                let mut guard = pool.acquire(Language::PlPgSql);
                guard.parser_mut().parse("BEGIN\nEND;", None).unwrap()
            },
            text: "BEGIN\nEND;".to_string(),
        };

        assert_eq!(region.to_parent_byte(0), 100);
        assert_eq!(region.to_parent_byte(5), 105);
        assert_eq!(region.to_local_byte(100), Some(0));
        assert_eq!(region.to_local_byte(105), Some(5));
        assert_eq!(region.to_local_byte(99), None);
        assert_eq!(region.to_local_byte(110), None);
    }
}
