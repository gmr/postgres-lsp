use std::collections::HashMap;

use pg_analysis::symbols::{QualifiedName, Symbol, SymbolKind};
use tokio_postgres::Client;

/// The synthetic URI used for database-sourced symbols.
pub const DB_URI: &str = "pg-catalog://database";

/// Namespace exclusion filter — skip system and information_schema namespaces.
const NS_FILTER: &str = "n.nspname NOT LIKE 'pg_%' AND n.nspname != 'information_schema'";

/// Load all schemas, tables, columns, functions, types, and sequences
/// from a live PostgreSQL database via `pg_catalog` queries.
pub async fn load_catalog(client: &Client) -> Result<Vec<Symbol>, CatalogError> {
    let mut symbols = Vec::new();

    load_schemas(client, &mut symbols).await?;
    load_tables_and_columns(client, &mut symbols).await?;
    load_functions(client, &mut symbols).await?;
    load_types(client, &mut symbols).await?;
    load_sequences(client, &mut symbols).await?;

    Ok(symbols)
}

async fn load_schemas(client: &Client, symbols: &mut Vec<Symbol>) -> Result<(), CatalogError> {
    let query = format!("SELECT nspname FROM pg_catalog.pg_namespace n WHERE {NS_FILTER}");
    let rows = client.query(&query, &[]).await?;

    for row in rows {
        let name: String = row.get(0);
        symbols.push(make_symbol(SymbolKind::Schema, None, &name, ""));
    }

    Ok(())
}

async fn load_tables_and_columns(
    client: &Client,
    symbols: &mut Vec<Symbol>,
) -> Result<(), CatalogError> {
    // Single batch query for tables/views and their columns (avoids N+1).
    let query = format!(
        "SELECT n.nspname, c.relname, c.relkind, \
                a.attname, format_type(a.atttypid, a.atttypmod) \
         FROM pg_catalog.pg_class c \
         JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
         LEFT JOIN pg_catalog.pg_attribute a \
           ON a.attrelid = c.oid AND a.attnum > 0 AND NOT a.attisdropped \
         WHERE c.relkind IN ('r', 'v', 'm', 'f') AND {NS_FILTER} \
         ORDER BY n.nspname, c.relname, a.attnum"
    );
    let rows = client.query(&query, &[]).await?;

    // Group columns by (schema, table).
    let mut tables: Vec<(String, String, SymbolKind)> = Vec::new();
    let mut columns: HashMap<(String, String), Vec<Symbol>> = HashMap::new();

    for row in &rows {
        let schema: String = row.get(0);
        let name: String = row.get(1);
        // PostgreSQL "char" type maps to i8 in tokio_postgres; all relkind values are ASCII.
        let relkind: i8 = row.get(2);
        let col_name: Option<String> = row.get(3);
        let col_type: Option<String> = row.get(4);

        let kind = match relkind as u8 as char {
            'r' => SymbolKind::Table,
            'v' => SymbolKind::View,
            'm' => SymbolKind::MaterializedView,
            'f' => SymbolKind::ForeignTable,
            _ => continue,
        };

        let key = (schema.clone(), name.clone());
        if !columns.contains_key(&key) {
            tables.push((schema.clone(), name.clone(), kind));
            columns.insert(key.clone(), Vec::new());
        }

        if let (Some(cn), Some(ct)) = (col_name, col_type) {
            let col_def = format!("{cn} {ct}");
            columns.get_mut(&key).unwrap().push(make_symbol(
                SymbolKind::Column,
                None,
                &cn,
                &col_def,
            ));
        }
    }

    for (schema, name, kind) in tables {
        let def_text = format!("{kind_label} {schema}.{name}", kind_label = kind.label());
        let mut sym = make_symbol(kind, Some(&schema), &name, &def_text);
        sym.children = columns.remove(&(schema, name)).unwrap_or_default();
        symbols.push(sym);
    }

    Ok(())
}

async fn load_functions(client: &Client, symbols: &mut Vec<Symbol>) -> Result<(), CatalogError> {
    let query = format!(
        "SELECT n.nspname, p.proname, \
                pg_catalog.pg_get_function_arguments(p.oid), \
                pg_catalog.pg_get_function_result(p.oid), \
                p.prokind \
         FROM pg_catalog.pg_proc p \
         JOIN pg_catalog.pg_namespace n ON n.oid = p.pronamespace \
         WHERE {NS_FILTER}"
    );
    let rows = client.query(&query, &[]).await?;

    for row in rows {
        let schema: String = row.get(0);
        let name: String = row.get(1);
        let args: String = row.get(2);
        let ret: String = row.get(3);
        let prokind: i8 = row.get(4);

        let kind = match prokind as u8 as char {
            'p' => SymbolKind::Procedure,
            _ => SymbolKind::Function,
        };

        let def_text = format!(
            "{kind_label} {schema}.{name}({args}) RETURNS {ret}",
            kind_label = kind.label()
        );
        symbols.push(make_symbol(kind, Some(&schema), &name, &def_text));
    }

    Ok(())
}

async fn load_types(client: &Client, symbols: &mut Vec<Symbol>) -> Result<(), CatalogError> {
    let query = format!(
        "SELECT n.nspname, t.typname, t.typtype \
         FROM pg_catalog.pg_type t \
         JOIN pg_catalog.pg_namespace n ON n.oid = t.typnamespace \
         WHERE {NS_FILTER} \
           AND t.typtype IN ('c', 'e', 'r', 'd') \
           AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_class c WHERE c.reltype = t.oid)"
    );
    let rows = client.query(&query, &[]).await?;

    for row in rows {
        let schema: String = row.get(0);
        let name: String = row.get(1);
        let typtype: i8 = row.get(2);

        let kind = match typtype as u8 as char {
            'd' => SymbolKind::Domain,
            _ => SymbolKind::Type,
        };

        let def_text = format!("{kind_label} {schema}.{name}", kind_label = kind.label());
        symbols.push(make_symbol(kind, Some(&schema), &name, &def_text));
    }

    Ok(())
}

async fn load_sequences(client: &Client, symbols: &mut Vec<Symbol>) -> Result<(), CatalogError> {
    let query = format!(
        "SELECT n.nspname, c.relname \
         FROM pg_catalog.pg_class c \
         JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
         WHERE c.relkind = 'S' AND {NS_FILTER}"
    );
    let rows = client.query(&query, &[]).await?;

    for row in rows {
        let schema: String = row.get(0);
        let name: String = row.get(1);
        let def_text = format!("sequence {schema}.{name}");
        symbols.push(make_symbol(
            SymbolKind::Sequence,
            Some(&schema),
            &name,
            &def_text,
        ));
    }

    Ok(())
}

fn make_symbol(
    kind: SymbolKind,
    schema: Option<&str>,
    name: &str,
    definition_text: &str,
) -> Symbol {
    let qname = match schema {
        Some(s) => QualifiedName::with_schema(s.to_string(), name.to_string()),
        None => QualifiedName::new(name.to_string()),
    };
    Symbol {
        kind,
        name: qname,
        uri: DB_URI.to_string(),
        start_byte: 0,
        end_byte: 0,
        start_line: 0,
        start_col: 0,
        end_line: 0,
        end_col: 0,
        name_start_line: 0,
        name_start_col: 0,
        name_end_line: 0,
        name_end_col: 0,
        definition_text: definition_text.to_string(),
        children: Vec::new(),
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("database error: {0}")]
    Database(#[from] tokio_postgres::Error),
}
