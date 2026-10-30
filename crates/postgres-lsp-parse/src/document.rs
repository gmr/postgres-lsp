use ropey::Rope;
use tree_sitter::{InputEdit, Point, Tree};

use crate::injection::{self, InjectedRegion};
use crate::parser::{Language, ParserPool};

/// A parsed SQL or PL/pgSQL document with incremental edit support.
///
/// Maintains the source text, the primary parse tree, and any injected
/// PL/pgSQL regions found within CREATE FUNCTION bodies.
pub struct Document {
    uri: String,
    language: Language,
    rope: Rope,
    tree: Option<Tree>,
    injections: Vec<InjectedRegion>,
}

impl Document {
    /// Create a new document by parsing the given source text.
    pub fn new(uri: String, text: &str, pool: &ParserPool) -> Self {
        let language = detect_language(&uri);
        let rope = Rope::from_str(text);
        let mut guard = pool.acquire(language);
        let tree = guard.parser_mut().parse(text, None);
        drop(guard);

        let injections = tree
            .as_ref()
            .map(|t| injection::extract_plpgsql_bodies(t, text, pool))
            .unwrap_or_default();

        Self {
            uri,
            language,
            rope,
            tree,
            injections,
        }
    }

    /// Apply a full-text replacement and re-parse.
    pub fn replace_full(&mut self, new_text: &str, pool: &ParserPool) {
        self.rope = Rope::from_str(new_text);
        let mut guard = pool.acquire(self.language);
        self.tree = guard.parser_mut().parse(new_text, None);
        drop(guard);

        self.injections = self
            .tree
            .as_ref()
            .map(|t| injection::extract_plpgsql_bodies(t, new_text, pool))
            .unwrap_or_default();
    }

    /// Apply an incremental edit (LSP-style line/col range) and re-parse.
    ///
    /// `start_line`/`start_col` and `end_line`/`end_col` are 0-based.
    /// `new_text` is the replacement text for the range.
    pub fn apply_edit(
        &mut self,
        start_line: usize,
        start_col: usize,
        end_line: usize,
        end_col: usize,
        new_text: &str,
        pool: &ParserPool,
    ) {
        let start_byte = self.position_to_byte(start_line, start_col);
        let old_end_byte = self.position_to_byte(end_line, end_col).max(start_byte);

        // Compute the old positions before the rope changes. The range
        // refers to the old text, so its lines can be past the end of the
        // new text (for example, after a format edit removes lines).
        // Point.column must be byte offsets, not UTF-16 code units.
        let start_position = self.byte_to_point(start_byte);
        let old_end_position = self.byte_to_point(old_end_byte);

        // Apply the edit to the rope.
        let start_char = self.rope.byte_to_char(start_byte);
        let end_char = self.rope.byte_to_char(old_end_byte);
        self.rope.remove(start_char..end_char);
        self.rope.insert(start_char, new_text);

        let new_end_byte = start_byte + new_text.len();
        let new_end_position = self.byte_to_point(new_end_byte);

        // Inform tree-sitter about the edit for incremental re-parsing.
        if let Some(ref mut tree) = self.tree {
            tree.edit(&InputEdit {
                start_byte,
                old_end_byte,
                new_end_byte,
                start_position,
                old_end_position,
                new_end_position,
            });
        }

        // Re-parse incrementally.
        let source = self.text();
        let mut guard = pool.acquire(self.language);
        self.tree = guard.parser_mut().parse(&source, self.tree.as_ref());
        drop(guard);

        self.injections = self
            .tree
            .as_ref()
            .map(|t| injection::extract_plpgsql_bodies(t, &source, pool))
            .unwrap_or_default();
    }

    pub fn uri(&self) -> &str {
        &self.uri
    }

    pub fn language(&self) -> Language {
        self.language
    }

    pub fn tree(&self) -> Option<&Tree> {
        self.tree.as_ref()
    }

    /// Get the injected PL/pgSQL regions (function bodies).
    pub fn injections(&self) -> &[InjectedRegion] {
        &self.injections
    }

    /// Get the full source text as a String.
    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    /// Get the underlying rope.
    pub fn rope(&self) -> &Rope {
        &self.rope
    }

    /// Get the LSP position of the end of the document as a 0-based
    /// (line, utf16_col) pair.
    pub fn end_position(&self) -> (usize, usize) {
        let last_line = self.rope.len_lines() - 1;
        let col = self.rope.line(last_line).chars().map(char::len_utf16).sum();
        (last_line, col)
    }

    /// Convert a 0-based (line, utf16_col) to a byte offset.
    ///
    /// LSP uses UTF-16 code units for character positions, so we must
    /// convert from UTF-16 units to byte offsets within the line.
    fn position_to_byte(&self, line: usize, utf16_col: usize) -> usize {
        // The position comes from the client. A line past the end maps to
        // the end of the document, and a column past the end of the line
        // maps to the end of the line, as the LSP specification requires.
        let Some(line_text) = self.rope.get_line(line) else {
            return self.rope.len_bytes();
        };
        let line_start = self.rope.line_to_byte(line);
        let mut utf16_count = 0;
        let mut byte_offset = 0;
        for ch in line_text.chars() {
            if utf16_count >= utf16_col || ch == '\n' || ch == '\r' {
                break;
            }
            utf16_count += ch.len_utf16();
            byte_offset += ch.len_utf8();
        }
        line_start + byte_offset
    }

    /// Convert a byte offset in the current rope to a tree-sitter point.
    fn byte_to_point(&self, byte: usize) -> Point {
        let row = self.rope.byte_to_line(byte);
        Point {
            row,
            column: byte - self.rope.line_to_byte(row),
        }
    }

    /// Collect all ERROR and MISSING nodes from the parse tree and injected regions.
    pub fn errors(&self) -> Vec<ParseError> {
        let Some(tree) = &self.tree else {
            return vec![];
        };
        let mut errors = Vec::new();
        collect_errors(tree.root_node(), &mut errors);

        // Collect errors from injected PL/pgSQL regions.
        let source = self.text();
        for region in &self.injections {
            let mut injection_errors = Vec::new();
            collect_errors(region.tree.root_node(), &mut injection_errors);

            // Map injection-local positions to parent document positions.
            let (parent_line, parent_col) = region.parent_position(&source);

            for mut err in injection_errors {
                if err.start_line == 0 {
                    err.start_col += parent_col;
                }
                if err.end_line == 0 {
                    err.end_col += parent_col;
                }
                err.start_line += parent_line;
                err.end_line += parent_line;
                errors.push(err);
            }
        }

        errors
    }
}

/// A parse error with position information.
#[derive(Debug, Clone)]
pub struct ParseError {
    pub start_line: usize,
    pub start_col: usize,
    pub end_line: usize,
    pub end_col: usize,
    pub message: String,
}

fn collect_errors(node: tree_sitter::Node, errors: &mut Vec<ParseError>) {
    if node.is_error() {
        errors.push(ParseError {
            start_line: node.start_position().row,
            start_col: node.start_position().column,
            end_line: node.end_position().row,
            end_col: node.end_position().column,
            message: format!("Syntax error: unexpected `{}`", node.kind()),
        });
    } else if node.is_missing() {
        errors.push(ParseError {
            start_line: node.start_position().row,
            start_col: node.start_position().column,
            end_line: node.end_position().row,
            end_col: node.end_position().column,
            message: format!("Missing `{}`", node.kind()),
        });
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_errors(child, errors);
    }
}

/// Detect the language from a file URI or path.
fn detect_language(uri: &str) -> Language {
    if uri.ends_with(".plpgsql") || uri.ends_with(".plsql") {
        Language::PlPgSql
    } else {
        // Default to PostgreSQL SQL for .sql and everything else.
        Language::Postgres
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_select() {
        let pool = ParserPool::new();
        let doc = Document::new("test.sql".into(), "SELECT 1;", &pool);
        assert!(doc.tree().is_some());
        assert!(doc.errors().is_empty());
    }

    #[test]
    fn parse_error_reported() {
        let pool = ParserPool::new();
        let doc = Document::new("test.sql".into(), "SELECTZ 1;", &pool);
        let errors = doc.errors();
        assert!(!errors.is_empty(), "expected parse errors for invalid SQL");
    }

    #[test]
    fn incremental_edit() {
        let pool = ParserPool::new();
        let mut doc = Document::new("test.sql".into(), "SELECT 1;", &pool);

        // Change "1" to "42"
        doc.apply_edit(0, 7, 0, 8, "42", &pool);
        assert_eq!(doc.text(), "SELECT 42;");
        assert!(doc.tree().is_some());
        assert!(doc.errors().is_empty());
    }

    #[test]
    fn full_replace() {
        let pool = ParserPool::new();
        let mut doc = Document::new("test.sql".into(), "SELECT 1;", &pool);
        doc.replace_full("INSERT INTO t VALUES (1);", &pool);
        assert_eq!(doc.text(), "INSERT INTO t VALUES (1);");
        assert!(doc.tree().is_some());
    }

    #[test]
    fn multiline_edit() {
        let pool = ParserPool::new();
        let mut doc = Document::new("test.sql".into(), "SELECT\n  1\nFROM\n  t;", &pool);
        // Replace "1" with "a, b"
        doc.apply_edit(1, 2, 1, 3, "a, b", &pool);
        assert_eq!(doc.text(), "SELECT\n  a, b\nFROM\n  t;");
        assert!(doc.tree().is_some());
    }

    #[test]
    fn detect_language_from_extension() {
        assert_eq!(detect_language("file.sql"), Language::Postgres);
        assert_eq!(detect_language("file.plpgsql"), Language::PlPgSql);
        assert_eq!(detect_language("file.plsql"), Language::PlPgSql);
        assert_eq!(
            detect_language("file:///path/to/schema.sql"),
            Language::Postgres
        );
    }

    #[test]
    fn plpgsql_injections_detected() {
        let pool = ParserPool::new();
        let sql = r#"CREATE FUNCTION test() RETURNS void LANGUAGE plpgsql AS $$
BEGIN
  RAISE NOTICE 'hello';
END;
$$;"#;
        let doc = Document::new("test.sql".into(), sql, &pool);
        assert!(doc.tree().is_some());
        assert!(
            !doc.injections().is_empty(),
            "should detect PL/pgSQL injection"
        );
        assert!(doc.injections()[0].text.contains("BEGIN"));
    }

    #[test]
    fn no_injections_in_plain_sql() {
        let pool = ParserPool::new();
        let doc = Document::new("test.sql".into(), "SELECT 1;", &pool);
        assert!(doc.injections().is_empty());
    }

    /// Regression test for issue #8. A client applies a whole-document
    /// format edit and then sends the edit back as an incremental change.
    /// The range uses line numbers of the old text, which has more lines
    /// than the new text.
    #[test]
    fn edit_that_removes_lines() {
        let pool = ParserPool::new();
        let old: String = (0..12).map(|i| format!("SELECT {i};\n")).collect();
        let mut doc = Document::new("test.sql".into(), &old, &pool);
        assert_eq!(doc.rope().len_lines(), 13);

        let new: String = (0..8).map(|i| format!("SELECT {i};\n")).collect();
        doc.apply_edit(0, 0, 12, 0, &new, &pool);
        assert_eq!(doc.text(), new);
        assert!(doc.errors().is_empty());
    }

    #[test]
    fn edit_that_removes_lines_crlf() {
        let pool = ParserPool::new();
        let old: String = (0..12).map(|i| format!("SELECT {i};\r\n")).collect();
        let mut doc = Document::new("test.sql".into(), &old, &pool);

        let new: String = (0..8).map(|i| format!("SELECT {i};\r\n")).collect();
        doc.apply_edit(0, 0, 12, 0, &new, &pool);
        assert_eq!(doc.text(), new);
        assert!(doc.errors().is_empty());
    }

    #[test]
    fn edit_that_removes_lines_in_middle() {
        let pool = ParserPool::new();
        let mut doc = Document::new("test.sql".into(), "SELECT\n  a,\n  b,\n  c\nFROM t;", &pool);
        doc.apply_edit(1, 0, 4, 0, "  x\n", &pool);
        assert_eq!(doc.text(), "SELECT\n  x\nFROM t;");
        assert!(doc.errors().is_empty());
    }

    #[test]
    fn edit_with_positions_past_end_does_not_panic() {
        let pool = ParserPool::new();
        let mut doc = Document::new("test.sql".into(), "SELECT 1;\n", &pool);
        // Line past the end clamps to the end of the document.
        doc.apply_edit(50, 0, 99, 7, "SELECT 2;", &pool);
        assert_eq!(doc.text(), "SELECT 1;\nSELECT 2;");
        // Column past the end of a line clamps to the end of that line.
        doc.apply_edit(0, 100, 0, 200, " -- x", &pool);
        assert_eq!(doc.text(), "SELECT 1; -- x\nSELECT 2;");
        // A start after the end does not panic.
        doc.apply_edit(1, 5, 0, 0, "", &pool);
        assert!(doc.tree().is_some());
    }

    #[test]
    fn end_position() {
        let pool = ParserPool::new();
        let doc = Document::new("test.sql".into(), "", &pool);
        assert_eq!(doc.end_position(), (0, 0));
        let doc = Document::new("test.sql".into(), "SELECT 1;\n", &pool);
        assert_eq!(doc.end_position(), (1, 0));
        let doc = Document::new("test.sql".into(), "SELECT 1;\r\nSELECT 'é😀';", &pool);
        // UTF-16 length of the last line: "SELECT 'é😀';" is 13 code units.
        assert_eq!(doc.end_position(), (1, 13));
    }
}
