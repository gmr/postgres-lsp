use clap::Parser;
use postgres_lsp_format::Style;
use tokio::io::{stdin, stdout};
use tower_lsp::{LspService, Server};
use tracing_subscriber::EnvFilter;

mod capabilities;
mod diagnostics;
mod semantic_tokens;
mod server;

#[derive(Parser)]
#[command(
    name = "postgres-lsp",
    about = "Language Server for PostgreSQL and PL/pgSQL"
)]
struct Cli {
    /// SQL formatting style
    #[arg(
        long,
        short = 's',
        default_value = "aweber",
        value_parser = parse_style,
        help = "Formatting style: river, mozilla, aweber, dbt, gitlab, kickstarter, mattmc3"
    )]
    format_style: Style,

    /// PostgreSQL connection URL for live schema introspection
    #[arg(
        long,
        short = 'd',
        env = "DATABASE_URL",
        help = "e.g., postgres://user:pass@localhost/dbname"
    )]
    database_url: Option<String>,
}

fn parse_style(s: &str) -> Result<Style, String> {
    s.parse::<Style>().map_err(|_| {
        format!(
            "unknown style '{s}'; options: river, mozilla, aweber, dbt, gitlab, kickstarter, mattmc3"
        )
    })
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    let format_style = cli.format_style;
    let database_url = cli.database_url;
    let (service, socket) = LspService::new(move |client| {
        server::Backend::new(client, format_style, database_url.clone())
    });
    Server::new(stdin(), stdout(), socket).serve(service).await;
}
