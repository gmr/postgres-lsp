use pg_parse::document::ParseError;
use tower_lsp::lsp_types::*;

/// Convert parse errors into LSP diagnostics.
pub fn to_diagnostics(errors: &[ParseError]) -> Vec<Diagnostic> {
    errors
        .iter()
        .map(|e| Diagnostic {
            range: Range {
                start: Position {
                    line: e.start_line as u32,
                    character: e.start_col as u32,
                },
                end: Position {
                    line: e.end_line as u32,
                    character: e.end_col as u32,
                },
            },
            severity: Some(DiagnosticSeverity::ERROR),
            source: Some("pg-lsp".to_string()),
            message: e.message.clone(),
            ..Default::default()
        })
        .collect()
}
