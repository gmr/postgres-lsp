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

    /// Compute the (line, column) of this region's start within the parent document.
    pub fn parent_position(&self, source: &str) -> (usize, usize) {
        let parent_line = source[..self.parent_start_byte].matches('\n').count();
        let parent_col = self.parent_start_byte
            - source[..self.parent_start_byte]
                .rfind('\n')
                .map(|p| p + 1)
                .unwrap_or(0);
        (parent_line, parent_col)
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
pub fn extract_plpgsql_bodies(tree: &Tree, source: &str, pool: &ParserPool) -> Vec<InjectedRegion> {
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
        && is_plpgsql_function_body(node, source)
        && let Some(body) = extract_dollar_quote_content(node, source)
    {
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

/// Check if a dollar_quoted_string node is the function body in the AS clause
/// of a CreateFunctionStmt whose LANGUAGE is plpgsql.
///
/// Expected tree path: dollar_quoted_string → Sconst → func_as →
/// createfunc_opt_item (with kw_as) → createfunc_opt_list → ... → CreateFunctionStmt
fn is_plpgsql_function_body(node: Node, source: &str) -> bool {
    // Walk up to verify the node is inside a func_as (the AS clause body).
    let func_stmt = {
        let mut current = node.parent();
        let mut found_func_as = false;
        let mut result = None;
        while let Some(p) = current {
            if p.kind() == "func_as" {
                found_func_as = true;
            }
            if p.kind() == "CreateFunctionStmt" {
                if found_func_as {
                    result = Some(p);
                }
                break;
            }
            current = p.parent();
        }
        result
    };

    let Some(stmt) = func_stmt else {
        return false;
    };

    // Find the LANGUAGE option structurally in the CreateFunctionStmt.
    has_language_plpgsql(stmt, source)
}

/// Check if a CreateFunctionStmt has LANGUAGE plpgsql by walking its
/// createfunc_opt_item children looking for kw_language followed by
/// a NonReservedWord_or_Sconst containing "plpgsql".
fn has_language_plpgsql(stmt: Node, source: &str) -> bool {
    fn check_opt_items(node: Node, source: &str) -> bool {
        if node.kind() == "createfunc_opt_item" {
            let mut found_language = false;
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "kw_language" {
                    found_language = true;
                }
                if found_language
                    && (child.kind() == "NonReservedWord_or_Sconst"
                        || child.kind() == "NonReservedWord"
                        || child.kind() == "identifier")
                {
                    let text = child.utf8_text(source.as_bytes()).unwrap_or("");
                    // Recurse into wrapper nodes to find the leaf identifier.
                    let lang = extract_leaf_text(child, source).unwrap_or(text);
                    if lang.eq_ignore_ascii_case("plpgsql") {
                        return true;
                    }
                }
            }
            return false;
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if check_opt_items(child, source) {
                return true;
            }
        }
        false
    }
    check_opt_items(stmt, source)
}

/// Extract the deepest leaf text from a node (finds identifier or keyword).
fn extract_leaf_text<'a>(node: Node<'a>, source: &'a str) -> Option<&'a str> {
    if node.child_count() == 0 {
        return node.utf8_text(source.as_bytes()).ok();
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(text) = extract_leaf_text(child, source) {
            return Some(text);
        }
    }
    None
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

    #[test]
    fn dollar_quoted_set_value_not_injected() {
        let pool = ParserPool::new();
        let source = "SET search_path = $$public$$;";

        let mut guard = pool.acquire(Language::Postgres);
        let tree = guard.parser_mut().parse(source, None).unwrap();
        drop(guard);

        let regions = extract_plpgsql_bodies(&tree, source, &pool);
        assert!(
            regions.is_empty(),
            "dollar-quoted SET value should not be treated as PL/pgSQL"
        );
    }

    #[test]
    fn sql_language_function_not_injected() {
        let pool = ParserPool::new();
        // Function whose body mentions "LANGUAGE plpgsql" in a string but
        // whose actual LANGUAGE is sql — should NOT be treated as PL/pgSQL.
        let source =
            r#"CREATE FUNCTION test() RETURNS text LANGUAGE sql AS $$SELECT 'LANGUAGE plpgsql'$$;"#;

        let mut guard = pool.acquire(Language::Postgres);
        let tree = guard.parser_mut().parse(source, None).unwrap();
        drop(guard);

        let regions = extract_plpgsql_bodies(&tree, source, &pool);
        assert!(
            regions.is_empty(),
            "LANGUAGE sql function should not be treated as PL/pgSQL"
        );
    }
}
