pub mod catalog;

use pg_analysis::WorkspaceIndex;
use tracing::info;

pub use catalog::{CatalogError, DB_URI};

/// Load schema from a live PostgreSQL database and merge into the workspace index.
pub async fn load_database_schema(
    database_url: &str,
    index: &WorkspaceIndex,
) -> Result<usize, CatalogError> {
    let (client, connection) = tokio_postgres::connect(database_url, tokio_postgres::NoTls).await?;

    // Spawn the connection handler.
    tokio::spawn(async move {
        if let Err(e) = connection.await {
            tracing::error!("database connection error: {e}");
        }
    });

    let symbols = catalog::load_catalog(&client).await?;
    let count = symbols.len();

    info!("loaded {count} symbols from database");
    index.load_symbols(DB_URI, symbols);

    Ok(count)
}
