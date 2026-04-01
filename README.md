# postgres-lsp

A Language Server Protocol (LSP) implementation for PostgreSQL and PL/pgSQL, built with [tree-sitter-postgres](https://github.com/gmr/tree-sitter-postgres) and [tower-lsp](https://github.com/ebkalderon/tower-lsp).

## Features

- **Diagnostics** — Parse errors from tree-sitter reported as LSP diagnostics
- **Semantic Tokens** — Syntax highlighting via semantic token classification
- **Document Symbols** — Outline of DDL statements (tables, functions, views, etc.)
- **Workspace Symbols** — Search across all open files
- **Go to Definition** — Navigate to table, function, type, and column definitions
- **Find References** — Find all usages of a symbol across the workspace
- **Hover** — Show definition source on hover
- **Completion** — Context-aware completion for keywords, tables, columns, and functions
- **Folding Ranges** — Collapse multi-line statements
- **Rename** — Rename symbols across the workspace
- **PL/pgSQL Support** — Parses PL/pgSQL function bodies with language injection

## Building

```bash
cargo build
```

Requires the [tree-sitter-postgres](https://github.com/gmr/tree-sitter-postgres) repository cloned as a sibling directory (`../tree-sitter-postgres`).

## Usage

The server communicates over stdio:

```bash
cargo run -p pg-lsp
```

Configure your editor to use `pg-lsp` as the language server for `.sql` files.

## License

BSD-3-Clause
