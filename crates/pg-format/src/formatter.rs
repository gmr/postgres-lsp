use libpgfmt::style::Style;

/// Options for SQL formatting.
#[derive(Debug, Clone)]
#[derive(Default)]
pub struct FormatOptions {
    /// The formatting style to use.
    pub style: Style,
}


/// Format SQL or PL/pgSQL source code.
///
/// Detects whether the input is PL/pgSQL (starts with DECLARE or BEGIN)
/// and delegates to the appropriate libpgfmt function.
pub fn format_sql(source: &str, options: &FormatOptions) -> Result<String, FormatError> {
    let trimmed = source.trim_start().to_uppercase();
    if trimmed.starts_with("DECLARE") || trimmed.starts_with("BEGIN") {
        libpgfmt::format_plpgsql(source, options.style).map_err(FormatError::from)
    } else {
        libpgfmt::format(source, options.style).map_err(FormatError::from)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FormatError {
    #[error("{0}")]
    Fmt(#[from] libpgfmt::error::FormatError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_simple_select() {
        let sql = "select a, b from users where active = true";
        let result = format_sql(sql, &FormatOptions::default()).unwrap();
        // Should contain uppercase keywords with the default (Aweber) style.
        assert!(result.contains("SELECT"), "expected uppercase SELECT, got: {result}");
        assert!(result.contains("FROM"), "expected uppercase FROM, got: {result}");
    }

    #[test]
    fn format_create_table() {
        let sql = "create table users (id int primary key, name text not null);";
        let result = format_sql(sql, &FormatOptions::default()).unwrap();
        assert!(result.contains("CREATE TABLE"), "expected uppercase CREATE TABLE, got: {result}");
    }

    #[test]
    fn format_with_style() {
        let sql = "select 1";
        let opts = FormatOptions {
            style: Style::Mozilla,
        };
        let result = format_sql(sql, &opts).unwrap();
        assert!(result.contains("SELECT"), "expected uppercase SELECT with Mozilla style");
    }

    #[test]
    fn format_plpgsql_block() {
        let code = "begin\nraise notice 'hello';\nend;";
        let result = format_sql(code, &FormatOptions::default()).unwrap();
        assert!(result.contains("BEGIN") || result.contains("begin"),
            "expected formatted PL/pgSQL, got: {result}");
    }
}
