# CLAUDE.md

This file provides guidance to Claude Code when working with this repository.

## Build & Test Commands

```bash
# Build the entire workspace
cargo build

# Run all tests
cargo test

# Run tests for a specific crate
cargo test -p postgres-lsp-parse
cargo test -p postgres-lsp-analysis
cargo test -p postgres-lsp

# Run a specific test
cargo test -p postgres-lsp-analysis -- symbols::tests::extract_create_table

# Run the LSP server (stdio transport)
cargo run -p postgres-lsp

# Dump parse tree for debugging
cargo run -p postgres-lsp-parse --example dump_tree
```

## Architecture

This is a Cargo workspace with five crates:

- **postgres-lsp-parse** — Document model with tree-sitter incremental parsing, parser pool, and PL/pgSQL injection handling. Core types: `Document`, `ParserPool`.
- **postgres-lsp-analysis** — Symbol extraction from parse trees, `DashMap`-backed workspace index, name resolution, completion, and hover logic. Core types: `Symbol`, `SymbolKind`, `QualifiedName`, `WorkspaceIndex`.
- **postgres-lsp-schema** — Optional live database introspection via `tokio-postgres` against `pg_catalog` (Phase 7).
- **postgres-lsp-format** — SQL formatting powered by [libpgfmt](https://crates.io/crates/libpgfmt). Supports 7 styles (River, Mozilla, Aweber, Dbt, Gitlab, Kickstarter, Mattmc3). Public API: `format_sql(source, options)` and `FormatOptions { style }`.
- **postgres-lsp** — Binary crate implementing the LSP via `tower-lsp`. Handles document sync, diagnostics, semantic tokens, go-to-definition, find references, completion, hover, document/workspace symbols, folding ranges, and rename.

### Key Design Constraints

- **No named fields in the tree-sitter grammar.** All tree navigation uses `node.child()` iteration with kind matching (e.g., find the `qualified_name` child of a `CreateStmt`), not named field accessors.
- **Tree structure wrapping.** Top-level statements are wrapped: `source_file` → `toplevel_stmt` → `stmt` → `CreateStmt`. Symbol extraction recurses through these wrappers.
- **Qualified names.** Schema-qualified names use `ColId` + `indirection` → `indirection_el` → `attr_name` → `ColLabel` → `identifier`. Unqualified names use `ColId` → `identifier`.
- **ropey::Rope** for text representation — O(log n) line/byte/char offset conversions for incremental edits.
- **DashMap** for the workspace index — lock-free concurrent reads across async LSP handlers.

### Dependencies

- `tree-sitter-postgres` from crates.io (PostgreSQL grammar for tree-sitter).
- `libpgfmt` from crates.io (SQL/PL-pgSQL formatter).

## Testing

Unit tests live alongside the code in each crate. Integration tests and fixtures will be added in `tests/` directories.
