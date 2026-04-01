use std::sync::Arc;

use crate::symbols::Symbol;

/// Build hover content for a symbol definition.
pub fn hover_for_symbol(symbol: &Arc<Symbol>) -> String {
    let kind_label = symbol.kind.label();
    let name = symbol.name.display();

    // Show the kind and name as a header, then the full definition.
    // Use a fenced code block with enough backticks to avoid conflicts
    // if the definition text itself contains triple backticks.
    let mut content = format!("**{kind_label}** `{name}`\n\n");
    let fence = if symbol.definition_text.contains("```") {
        "````"
    } else {
        "```"
    };
    content.push_str(fence);
    content.push_str("sql\n");
    content.push_str(&symbol.definition_text);
    content.push('\n');
    content.push_str(fence);

    content
}
