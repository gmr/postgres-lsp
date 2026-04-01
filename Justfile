# Default recipe: run checks
default: check

# Run all checks (format, lint, test)
check: fmt-check lint test

# Build the library
build:
    cargo build

# Run tests
test:
    cargo test

# Run clippy lints
lint:
    cargo clippy -- -D warnings

# Check formatting
fmt-check:
    cargo fmt --check

# Auto-format code
fmt:
    cargo fmt

# Run all checks then build in release mode
release-build: check
    cargo build --release

# Set the release version in Cargo.toml
set-version version:
    #!/usr/bin/env bash
    set -euo pipefail
    current=$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)"/\1/')
    if [ "{{ version }}" = "$current" ]; then
        echo "Version is already {{ version }}"
        exit 1
    fi
    # Update [workspace.package] version (^version = "...") and intra-workspace
    # dep versions (, version = "...") — does not affect third-party dep versions
    # which use the { version = "..." } form (brace prefix, not comma prefix).
    tmp=$(mktemp)
    sed -E 's/(^version = |, version = )"[^"]*"/\1"{{ version }}"/g' Cargo.toml > "$tmp"
    mv "$tmp" Cargo.toml
    cargo check
    echo "Updated version: $current -> {{ version }}"

# Tag a release (sets version, commits, tags, pushes)
release version: (set-version version)
    git add Cargo.toml Cargo.lock
    git commit -m "Release v{{ version }}"
    git tag -a "v{{ version }}" -m "v{{ version }}"
    git push origin main --tags

# Publish to crates.io (dry run) — in dependency order
publish-dry:
    cargo publish --dry-run -p postgres-lsp-parse
    cargo publish --dry-run -p postgres-lsp-format
    cargo publish --dry-run -p postgres-lsp-analysis
    cargo publish --dry-run -p postgres-lsp-schema
    cargo publish --dry-run -p postgres-lsp

# Publish to crates.io — in dependency order
publish:
    cargo publish -p postgres-lsp-parse
    cargo publish -p postgres-lsp-format
    cargo publish -p postgres-lsp-analysis
    cargo publish -p postgres-lsp-schema
    cargo publish -p postgres-lsp

# Clean build artifacts
clean:
    cargo clean
