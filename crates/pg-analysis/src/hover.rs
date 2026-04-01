use std::sync::Arc;

use crate::symbols::Symbol;

/// Build hover content for a symbol definition.
pub fn hover_for_symbol(symbol: &Arc<Symbol>) -> String {
    let kind_label = symbol.kind.label();
    let name = symbol.name.display();

    // Show the kind and name as a header, then the full definition.
    let mut content = format!("**{kind_label}** `{name}`\n\n");
    content.push_str("```sql\n");
    content.push_str(&symbol.definition_text);
    content.push_str("\n```");

    content
}
