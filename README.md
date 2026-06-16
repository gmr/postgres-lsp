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
- **Signature Help** — Parameter hints for function calls
- **Folding Ranges** — Collapse multi-line statements
- **Rename** — Rename symbols across the workspace
- **Code Actions** — Quick fixes and refactor rewrites
- **Formatting** — Reformat SQL using one of several style guides
- **PL/pgSQL Support** — Parses PL/pgSQL function bodies with language injection

## Installation

### Homebrew (macOS / Linux)

```bash
brew tap gmr/postgres
brew install postgres-lsp
```

> [!NOTE]
> Homebrew 6.0 added [tap trust](https://docs.brew.sh/Tap-Trust), and some
> versions fail to install third-party taps inside the build sandbox (the
> error mentions `build.rb ... exited with 1`). If you hit this, trust the
> formula first:
>
> ```bash
> brew trust --formula gmr/postgres/postgres-lsp
> ```
>
> or, as a temporary workaround, set `HOMEBREW_NO_REQUIRE_TAP_TRUST=1` for
> the install.

### From Source (via Cargo)

```bash
cargo install postgres-lsp
```

## Building

```bash
cargo build
```

Requires the [tree-sitter-postgres](https://github.com/gmr/tree-sitter-postgres) repository cloned as a sibling directory (`../tree-sitter-postgres`).

## Usage

The server communicates over stdio:

```bash
cargo run -p postgres-lsp
```

Configure your editor to use `postgres-lsp` as the language server for `.sql` files.

## License

BSD-3-Clause
