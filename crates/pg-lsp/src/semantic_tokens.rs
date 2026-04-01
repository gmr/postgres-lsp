use std::sync::LazyLock;

use tower_lsp::lsp_types::*;
use tree_sitter::Node;

pub static LEGEND: LazyLock<SemanticTokensLegend> = LazyLock::new(|| SemanticTokensLegend {
    token_types: TOKEN_TYPES.to_vec(),
    token_modifiers: TOKEN_MODIFIERS.to_vec(),
});

const TOKEN_TYPES: &[SemanticTokenType] = &[
    SemanticTokenType::KEYWORD,
    SemanticTokenType::STRING,
    SemanticTokenType::NUMBER,
    SemanticTokenType::COMMENT,
    SemanticTokenType::OPERATOR,
    SemanticTokenType::VARIABLE,
    SemanticTokenType::FUNCTION,
    SemanticTokenType::TYPE,
    SemanticTokenType::PARAMETER,
    SemanticTokenType::PROPERTY,
];

const TOKEN_MODIFIERS: &[SemanticTokenModifier] = &[
    SemanticTokenModifier::DECLARATION,
    SemanticTokenModifier::DEFINITION,
    SemanticTokenModifier::READONLY,
];

/// Walk the tree and produce semantic tokens based on node kinds.
///
/// This uses the node kinds from the tree-sitter-postgres grammar directly,
/// mapping them to LSP semantic token types. Positions and lengths are
/// converted to UTF-16 code units as required by LSP.
pub fn collect_semantic_tokens(root: Node, source: &str) -> Vec<SemanticToken> {
    let mut tokens = Vec::new();
    let mut prev_line = 0u32;
    let mut prev_start = 0u32;

    // Pre-split source into lines for efficient UTF-16 offset computation.
    let lines: Vec<&str> = source.lines().collect();

    collect_tokens_recursive(root, source, &lines, &mut tokens, &mut prev_line, &mut prev_start);
    tokens
}

/// Compute the UTF-16 column offset for a byte column within a line.
fn byte_col_to_utf16(line: &str, byte_col: usize) -> u32 {
    let end = byte_col.min(line.len());
    line[..end].encode_utf16().count() as u32
}

/// Compute the UTF-16 length of a byte range within the source text.
fn byte_len_to_utf16(source: &str, start_byte: usize, end_byte: usize) -> u32 {
    let end = end_byte.min(source.len());
    let start = start_byte.min(end);
    source[start..end].encode_utf16().count() as u32
}

fn collect_tokens_recursive(
    node: Node,
    source: &str,
    lines: &[&str],
    tokens: &mut Vec<SemanticToken>,
    prev_line: &mut u32,
    prev_start: &mut u32,
) {
    let kind = node.kind();

    // Determine the token type for this node.
    if let Some(token_type) = classify_node(kind) {
        let start_line = node.start_position().row as u32;
        let byte_col = node.start_position().column;
        let start_char = if (start_line as usize) < lines.len() {
            byte_col_to_utf16(lines[start_line as usize], byte_col)
        } else {
            byte_col as u32
        };
        let length = byte_len_to_utf16(source, node.start_byte(), node.end_byte());

        if length > 0 {
            let delta_line = start_line - *prev_line;
            let delta_start = if delta_line == 0 {
                start_char - *prev_start
            } else {
                start_char
            };

            tokens.push(SemanticToken {
                delta_line,
                delta_start,
                length,
                token_type,
                token_modifiers_bitset: 0,
            });

            *prev_line = start_line;
            *prev_start = start_char;

            // Don't recurse into classified leaf nodes.
            return;
        }
    }

    // Recurse into children.
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_tokens_recursive(child, source, lines, tokens, prev_line, prev_start);
    }
}

/// Map a tree-sitter node kind to a semantic token type index.
fn classify_node(kind: &str) -> Option<u32> {
    // Keywords (kw_* nodes from tree-sitter-postgres)
    if kind.starts_with("kw_") {
        return Some(0); // KEYWORD
    }

    match kind {
        // Literals
        "string_literal" | "bit_string_literal" | "hex_string_literal"
        | "dollar_quoted_string" => Some(1), // STRING
        "integer_literal" | "float_literal" => Some(2), // NUMBER
        "comment" => Some(3),                            // COMMENT

        // Operators
        "operator" | "+" | "-" | "*" | "/" | "%" | "^" | "<" | ">" | "=" | "!=" | "<>"
        | "<=" | ">=" | "||" | "::" => Some(4), // OPERATOR

        // Identifiers
        "identifier" | "ColId" => Some(5), // VARIABLE

        // Function-related
        "func_name" => Some(6), // FUNCTION

        // Types
        "SimpleTypename" | "GenericType" | "type_function_name" => Some(7), // TYPE

        // Parameters
        "param" => Some(8), // PARAMETER

        // Column references
        "columnref" => Some(9), // PROPERTY

        _ => None,
    }
}
