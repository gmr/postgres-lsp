use ropey::Rope;
use tree_sitter::{InputEdit, Point, Tree};

use crate::parser::{Language, ParserPool};

/// A parsed SQL or PL/pgSQL document with incremental edit support.
///
/// Maintains both the source text (as a `Rope` for efficient edits) and the
/// current tree-sitter parse tree. Supports incremental re-parsing after edits.
pub struct Document {
    uri: String,
    language: Language,
    rope: Rope,
    tree: Option<Tree>,
}

impl Document {
    /// Create a new document by parsing the given source text.
    pub fn new(uri: String, text: &str, pool: &ParserPool) -> Self {
        let language = detect_language(&uri);
        let rope = Rope::from_str(text);
        let mut guard = pool.acquire(language);
        let tree = guard.parser_mut().parse(text, None);

        Self {
            uri,
            language,
            rope,
            tree,
        }
    }

    /// Apply a full-text replacement and re-parse.
    pub fn replace_full(&mut self, new_text: &str, pool: &ParserPool) {
        self.rope = Rope::from_str(new_text);
        let mut guard = pool.acquire(self.language);
        self.tree = guard.parser_mut().parse(new_text, None);
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
        let len = self.rope.len_bytes();
        let start_byte = self.position_to_byte(start_line, start_col).min(len);
        let old_end_byte = self.position_to_byte(end_line, end_col).min(len);

        // Apply the edit to the rope.
        let start_char = self.rope.byte_to_char(start_byte);
        let end_char = self.rope.byte_to_char(old_end_byte);
        self.rope.remove(start_char..end_char);
        self.rope.insert(start_char, new_text);

        let new_end_byte = start_byte + new_text.len();
        let new_end_line = self.rope.byte_to_line(new_end_byte);
        let new_end_col = new_end_byte - self.rope.line_to_byte(new_end_line);

        // Inform tree-sitter about the edit for incremental re-parsing.
        // Point.column must be byte offsets, not UTF-16 code units.
        let start_byte_col = start_byte - self.rope.line_to_byte(start_line);
        let old_end_byte_col = old_end_byte - self.rope.line_to_byte(end_line);
        if let Some(ref mut tree) = self.tree {
            tree.edit(&InputEdit {
                start_byte,
                old_end_byte,
                new_end_byte,
                start_position: Point {
                    row: start_line,
                    column: start_byte_col,
                },
                old_end_position: Point {
                    row: end_line,
                    column: old_end_byte_col,
                },
                new_end_position: Point {
                    row: new_end_line,
                    column: new_end_col,
                },
            });
        }

        // Re-parse incrementally.
        let source = self.text();
        let mut guard = pool.acquire(self.language);
        self.tree = guard.parser_mut().parse(&source, self.tree.as_ref());
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

    /// Get the full source text as a String.
    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    /// Get the underlying rope.
    pub fn rope(&self) -> &Rope {
        &self.rope
    }

    /// Convert a 0-based (line, utf16_col) to a byte offset.
    ///
    /// LSP uses UTF-16 code units for character positions, so we must
    /// convert from UTF-16 units to byte offsets within the line.
    fn position_to_byte(&self, line: usize, utf16_col: usize) -> usize {
        let line_start = self.rope.line_to_byte(line);
        let line_text = self.rope.line(line);
        let mut utf16_count = 0;
        let mut byte_offset = 0;
        for ch in line_text.chars() {
            if utf16_count >= utf16_col {
                break;
            }
            utf16_count += ch.len_utf16();
            byte_offset += ch.len_utf8();
        }
        line_start + byte_offset
    }

    /// Collect all ERROR and MISSING nodes from the parse tree as diagnostics.
    pub fn errors(&self) -> Vec<ParseError> {
        let Some(tree) = &self.tree else {
            return vec![];
        };
        let mut errors = Vec::new();
        collect_errors(tree.root_node(), &mut errors);
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
}
