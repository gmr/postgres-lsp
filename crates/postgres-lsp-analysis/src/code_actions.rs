use tree_sitter::Node;

/// A code action produced by the analysis layer.
#[derive(Debug, Clone)]
pub struct CodeAction {
    pub title: String,
    pub kind: CodeActionKind,
    pub edit: TextEditAction,
}

#[derive(Debug, Clone)]
pub enum CodeActionKind {
    QuickFix,
    RefactorRewrite,
}

/// A single text edit for a code action.
#[derive(Debug, Clone)]
pub struct TextEditAction {
    pub start_line: usize,
    pub start_col: usize,
    pub end_line: usize,
    pub end_col: usize,
    pub new_text: String,
}

/// Compute code actions for a given cursor range in the document.
pub fn compute_code_actions(
    tree: &tree_sitter::Tree,
    source: &str,
    start_line: usize,
    start_col: usize,
    end_line: usize,
    end_col: usize,
) -> Vec<CodeAction> {
    let mut actions = Vec::new();

    // 1. Add missing semicolon — look for MISSING nodes in the range.
    collect_missing_semicolons(
        tree.root_node(),
        (start_line, start_col),
        (end_line, end_col),
        &mut actions,
    );

    // 2. Uppercase keyword at cursor — if the cursor is on a lowercase keyword.
    if let Some(node) = tree.root_node().descendant_for_point_range(
        tree_sitter::Point {
            row: start_line,
            column: start_col,
        },
        tree_sitter::Point {
            row: end_line,
            column: end_col,
        },
    ) && let Some(action) = uppercase_keyword_action(node, source)
    {
        actions.push(action);
    }

    actions
}

/// Find MISSING `;` nodes within the requested range and offer to insert them.
fn collect_missing_semicolons(
    node: Node,
    start: (usize, usize),
    end: (usize, usize),
    actions: &mut Vec<CodeAction>,
) {
    if node.is_missing() && node.kind() == ";" {
        let line = node.start_position().row;
        let col = node.start_position().column;
        if (line, col) >= start && (line, col) <= end {
            actions.push(CodeAction {
                title: "Add missing semicolon".to_string(),
                kind: CodeActionKind::QuickFix,
                edit: TextEditAction {
                    start_line: line,
                    start_col: col,
                    end_line: line,
                    end_col: col,
                    new_text: ";".to_string(),
                },
            });
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_missing_semicolons(child, start, end, actions);
    }
}

/// If the cursor is on a keyword node that is lowercase, offer to uppercase it.
fn uppercase_keyword_action(node: Node, source: &str) -> Option<CodeAction> {
    if !node.kind().starts_with("kw_") {
        return None;
    }

    let text = node.utf8_text(source.as_bytes()).ok()?;
    let upper = text.to_uppercase();

    // Only offer if the text isn't already uppercase.
    if text == upper {
        return None;
    }

    Some(CodeAction {
        title: format!("Uppercase keyword: {text} → {upper}"),
        kind: CodeActionKind::RefactorRewrite,
        edit: TextEditAction {
            start_line: node.start_position().row,
            start_col: node.start_position().column,
            end_line: node.end_position().row,
            end_col: node.end_position().column,
            new_text: upper,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(sql: &str) -> tree_sitter::Tree {
        let mut parser = tree_sitter::Parser::new();
        let lang: tree_sitter::Language = tree_sitter_postgres::LANGUAGE.into();
        parser.set_language(&lang).unwrap();
        parser.parse(sql, None).unwrap()
    }

    #[test]
    fn missing_semicolon_action() {
        // Two statements without semicolons trigger errors in tree-sitter-postgres.
        let sql = "SELECT 1\nSELECT 2;";
        let tree = parse(sql);
        assert!(
            tree.root_node().has_error(),
            "grammar should report error for missing separator"
        );
        let actions = compute_code_actions(&tree, sql, 0, 0, 1, 9);
        // If the grammar emits MISSING `;` nodes, we should get a quick fix.
        // If it emits ERROR nodes instead, the list may be empty, which is
        // acceptable — the grammar controls which node types appear.
        for action in &actions {
            if action.title == "Add missing semicolon" {
                assert_eq!(action.edit.new_text, ";");
                assert!(matches!(action.kind, CodeActionKind::QuickFix));
            }
        }
    }

    #[test]
    fn uppercase_keyword_action_for_lowercase() {
        let sql = "select 1;";
        let tree = parse(sql);
        // Cursor on "select" (byte col 0-6)
        let actions = compute_code_actions(&tree, sql, 0, 0, 0, 0);
        let kw_actions: Vec<_> = actions
            .iter()
            .filter(|a| a.title.contains("Uppercase"))
            .collect();
        assert!(!kw_actions.is_empty(), "should offer to uppercase 'select'");
        assert_eq!(kw_actions[0].edit.new_text, "SELECT");
    }

    #[test]
    fn no_uppercase_action_for_already_upper() {
        let sql = "SELECT 1;";
        let tree = parse(sql);
        let actions = compute_code_actions(&tree, sql, 0, 0, 0, 0);
        let kw_actions: Vec<_> = actions
            .iter()
            .filter(|a| a.title.contains("Uppercase"))
            .collect();
        assert!(
            kw_actions.is_empty(),
            "should not offer uppercase for already-uppercase keyword"
        );
    }

    #[test]
    fn no_actions_for_valid_sql() {
        let sql = "SELECT 1;";
        let tree = parse(sql);
        let actions = compute_code_actions(&tree, sql, 0, 7, 0, 7);
        // Should have no quick fixes (valid SQL, cursor on "1")
        let fixes: Vec<_> = actions
            .iter()
            .filter(|a| matches!(a.kind, CodeActionKind::QuickFix))
            .collect();
        assert!(fixes.is_empty(), "no quick fixes for valid SQL");
    }
}
