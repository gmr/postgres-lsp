use std::sync::Arc;

use dashmap::DashMap;
use pg_analysis::WorkspaceIndex;
use pg_analysis::completion::{self, CompletionContext};
use pg_analysis::hover;
use pg_analysis::resolve;
use pg_analysis::signature;
use pg_analysis::symbols::QualifiedName;
use pg_format::Style;
use pg_parse::ParserPool;
use pg_parse::document::Document;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer};
use tracing::info;

use crate::capabilities;
use crate::diagnostics;
use crate::semantic_tokens;

pub struct Backend {
    client: Client,
    pool: Arc<ParserPool>,
    documents: DashMap<String, Document>,
    index: Arc<WorkspaceIndex>,
    format_style: Style,
}

impl Backend {
    pub fn new(client: Client, format_style: Style) -> Self {
        Self {
            client,
            pool: Arc::new(ParserPool::new()),
            documents: DashMap::new(),
            index: Arc::new(WorkspaceIndex::new()),
            format_style,
        }
    }

    async fn publish_diagnostics(&self, uri: &Url, doc: &Document) {
        if let Some(tree) = doc.tree() {
            self.index
                .update_file(uri.as_str(), tree, &doc.text(), doc.injections());
        }
        let diags = diagnostics::to_diagnostics(&doc.errors());
        self.client
            .publish_diagnostics(uri.clone(), diags, None)
            .await;
    }

    fn node_at_position<'a>(
        tree: &'a tree_sitter::Tree,
        source: &str,
        line: u32,
        character: u32,
    ) -> Option<tree_sitter::Node<'a>> {
        let line_text = source.lines().nth(line as usize).unwrap_or("");
        let byte_col = utf16_to_byte_offset(line_text, character as usize);
        let point = tree_sitter::Point {
            row: line as usize,
            column: byte_col,
        };
        tree.root_node().descendant_for_point_range(point, point)
    }

    fn name_at_position(
        tree: &tree_sitter::Tree,
        source: &str,
        line: u32,
        character: u32,
    ) -> Option<(String, Range)> {
        let node = Self::node_at_position(tree, source, line, character)?;
        let text = node.utf8_text(source.as_bytes()).ok()?;
        let name = text.trim().replace('"', "");
        if name.is_empty() {
            return None;
        }
        let range = node_range(node, source);
        Some((name, range))
    }

    fn completion_context(
        tree: &tree_sitter::Tree,
        source: &str,
        line: u32,
        character: u32,
    ) -> CompletionContext {
        let line_text = source.lines().nth(line as usize).unwrap_or("");
        let byte_col = utf16_to_byte_offset(line_text, character as usize);
        let point = tree_sitter::Point {
            row: line as usize,
            column: byte_col,
        };

        if let Some(node) = tree.root_node().descendant_for_point_range(point, point) {
            let mut current = Some(node);
            while let Some(n) = current {
                match n.kind() {
                    "from_clause" | "from_list" => return CompletionContext::AfterFrom,
                    "join_type" | "JoinExpr" => return CompletionContext::AfterJoin,
                    "target_list" => return CompletionContext::AfterSelect,
                    "set_clause_list" | "set_clause" => return CompletionContext::ColumnPosition,
                    _ => {}
                }
                current = n.parent();
            }
        }

        let line_text = source.lines().nth(line as usize).unwrap_or("");
        // Convert LSP UTF-16 character offset to a byte offset within the line.
        let byte_offset = utf16_to_byte_offset(line_text, character as usize);
        let before_cursor = &line_text[..byte_offset];
        let trimmed = before_cursor.trim_end().to_uppercase();

        if trimmed.ends_with("FROM") || trimmed.ends_with("JOIN") {
            CompletionContext::AfterFrom
        } else if trimmed.ends_with("SELECT") {
            CompletionContext::AfterSelect
        } else {
            CompletionContext::General
        }
    }
}

/// Convert an LSP UTF-16 character offset to a byte offset within a line.
fn utf16_to_byte_offset(line: &str, utf16_col: usize) -> usize {
    let mut utf16_count = 0;
    let mut byte_offset = 0;
    for ch in line.chars() {
        if utf16_count >= utf16_col {
            break;
        }
        utf16_count += ch.len_utf16();
        byte_offset += ch.len_utf8();
    }
    byte_offset.min(line.len())
}

/// Convert a byte column offset to UTF-16 code units within a line.
fn byte_col_to_utf16(line: &str, byte_col: usize) -> u32 {
    let end = byte_col.min(line.len());
    line[..end].encode_utf16().count() as u32
}

/// Build a Position from a line number and byte column, converting to UTF-16 if source is available.
fn make_position(line: usize, byte_col: usize, lines: &[&str]) -> Position {
    let line_text = lines.get(line).copied().unwrap_or("");
    Position {
        line: line as u32,
        character: byte_col_to_utf16(line_text, byte_col),
    }
}

/// Build an LSP Range from a tree-sitter node, converting byte columns to UTF-16.
fn node_range(node: tree_sitter::Node, source: &str) -> Range {
    let lines: Vec<&str> = source.lines().collect();
    Range {
        start: make_position(
            node.start_position().row,
            node.start_position().column,
            &lines,
        ),
        end: make_position(node.end_position().row, node.end_position().column, &lines),
    }
}

/// Build an LSP Range from a Symbol's statement range, converting byte columns to UTF-16.
/// `source` should be the file containing the symbol; pass the document map to look it up.
fn symbol_range_with_source(
    sym: &pg_analysis::symbols::Symbol,
    docs: &DashMap<String, Document>,
) -> Range {
    if let Some(doc) = docs.get(&sym.uri) {
        let source = doc.text();
        let lines: Vec<&str> = source.lines().collect();
        Range {
            start: make_position(sym.start_line, sym.start_col, &lines),
            end: make_position(sym.end_line, sym.end_col, &lines),
        }
    } else {
        // Fallback: byte == utf16 (correct for ASCII)
        Range {
            start: Position {
                line: sym.start_line as u32,
                character: sym.start_col as u32,
            },
            end: Position {
                line: sym.end_line as u32,
                character: sym.end_col as u32,
            },
        }
    }
}

/// Build an LSP Range from a SymbolRef, converting byte columns to UTF-16.
fn ref_range_with_source(
    r: &pg_analysis::symbols::SymbolRef,
    docs: &DashMap<String, Document>,
) -> Range {
    if let Some(doc) = docs.get(&r.uri) {
        let source = doc.text();
        let lines: Vec<&str> = source.lines().collect();
        Range {
            start: make_position(r.start_line, r.start_col, &lines),
            end: make_position(r.end_line, r.end_col, &lines),
        }
    } else {
        Range {
            start: Position {
                line: r.start_line as u32,
                character: r.start_col as u32,
            },
            end: Position {
                line: r.end_line as u32,
                character: r.end_col as u32,
            },
        }
    }
}

fn symbol_kind_to_lsp(kind: pg_analysis::SymbolKind) -> tower_lsp::lsp_types::SymbolKind {
    use pg_analysis::SymbolKind as SK;
    match kind {
        SK::Schema => tower_lsp::lsp_types::SymbolKind::NAMESPACE,
        SK::Table | SK::ForeignTable => tower_lsp::lsp_types::SymbolKind::CLASS,
        SK::View | SK::MaterializedView => tower_lsp::lsp_types::SymbolKind::CLASS,
        SK::Column => tower_lsp::lsp_types::SymbolKind::FIELD,
        SK::Function => tower_lsp::lsp_types::SymbolKind::FUNCTION,
        SK::Procedure => tower_lsp::lsp_types::SymbolKind::FUNCTION,
        SK::Trigger => tower_lsp::lsp_types::SymbolKind::EVENT,
        SK::Index => tower_lsp::lsp_types::SymbolKind::KEY,
        SK::Sequence => tower_lsp::lsp_types::SymbolKind::CONSTANT,
        SK::Type | SK::Domain => tower_lsp::lsp_types::SymbolKind::TYPE_PARAMETER,
        SK::Extension => tower_lsp::lsp_types::SymbolKind::PACKAGE,
        SK::Role => tower_lsp::lsp_types::SymbolKind::OBJECT,
        SK::Policy | SK::Publication | SK::Subscription => tower_lsp::lsp_types::SymbolKind::OBJECT,
        SK::Variable => tower_lsp::lsp_types::SymbolKind::VARIABLE,
        SK::Cursor => tower_lsp::lsp_types::SymbolKind::VARIABLE,
    }
}

fn symbol_to_symbol_information(
    sym: &pg_analysis::symbols::Symbol,
    uri: Url,
    docs: &DashMap<String, Document>,
) -> SymbolInformation {
    #[allow(deprecated)]
    SymbolInformation {
        name: sym.name.display(),
        kind: symbol_kind_to_lsp(sym.kind),
        tags: None,
        deprecated: None,
        location: Location {
            uri,
            range: symbol_range_with_source(sym, docs),
        },
        container_name: None,
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _params: InitializeParams) -> Result<InitializeResult> {
        info!("pg-lsp initializing");
        Ok(InitializeResult {
            capabilities: capabilities::server_capabilities(),
            server_info: Some(ServerInfo {
                name: "pg-lsp".to_string(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
        })
    }

    async fn initialized(&self, _params: InitializedParams) {
        info!("pg-lsp initialized");
    }

    async fn shutdown(&self) -> Result<()> {
        info!("pg-lsp shutting down");
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;
        let text = params.text_document.text;
        let doc = Document::new(uri.to_string(), &text, &self.pool);
        self.publish_diagnostics(&uri, &doc).await;
        self.documents.insert(uri.to_string(), doc);
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        let uri_str = uri.to_string();

        // Apply edits under the DashMap guard, then drop it before awaiting.
        {
            let Some(mut doc) = self.documents.get_mut(&uri_str) else {
                return;
            };
            for change in params.content_changes {
                if let Some(range) = change.range {
                    doc.apply_edit(
                        range.start.line as usize,
                        range.start.character as usize,
                        range.end.line as usize,
                        range.end.character as usize,
                        &change.text,
                        &self.pool,
                    );
                } else {
                    doc.replace_full(&change.text, &self.pool);
                }
            }
        }
        // Guard dropped — safe to await now.
        if let Some(doc) = self.documents.get(&uri_str) {
            self.publish_diagnostics(&uri, &doc).await;
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri.to_string();
        self.documents.remove(&uri);
        self.index.remove_file(&uri);
        self.client
            .publish_diagnostics(params.text_document.uri, vec![], None)
            .await;
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = params.text_document_position.text_document.uri.to_string();
        let line = params.text_document_position.position.line;
        let character = params.text_document_position.position.character;

        let Some(doc) = self.documents.get(&uri) else {
            return Ok(None);
        };

        let ctx = if let Some(tree) = doc.tree() {
            let source = doc.text();
            Self::completion_context(tree, &source, line, character)
        } else {
            CompletionContext::General
        };

        let items = completion::completions(&self.index, &ctx);
        let lsp_items: Vec<CompletionItem> = items
            .into_iter()
            .map(|item| CompletionItem {
                label: item.label,
                kind: Some(match item.kind {
                    completion::CompletionKind::Keyword => CompletionItemKind::KEYWORD,
                    completion::CompletionKind::Table | completion::CompletionKind::View => {
                        CompletionItemKind::CLASS
                    }
                    completion::CompletionKind::Column => CompletionItemKind::FIELD,
                    completion::CompletionKind::Function
                    | completion::CompletionKind::Procedure => CompletionItemKind::FUNCTION,
                    completion::CompletionKind::Type => CompletionItemKind::TYPE_PARAMETER,
                    completion::CompletionKind::Schema => CompletionItemKind::MODULE,
                    completion::CompletionKind::Sequence => CompletionItemKind::CONSTANT,
                    completion::CompletionKind::Variable => CompletionItemKind::VARIABLE,
                }),
                detail: item.detail,
                documentation: item.documentation.map(|d| {
                    Documentation::MarkupContent(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: d,
                    })
                }),
                ..Default::default()
            })
            .collect();

        Ok(Some(CompletionResponse::Array(lsp_items)))
    }

    async fn signature_help(&self, params: SignatureHelpParams) -> Result<Option<SignatureHelp>> {
        let uri = params
            .text_document_position_params
            .text_document
            .uri
            .to_string();
        let line = params.text_document_position_params.position.line;
        let character = params.text_document_position_params.position.character;

        let Some(doc) = self.documents.get(&uri) else {
            return Ok(None);
        };
        let Some(tree) = doc.tree() else {
            return Ok(None);
        };
        let source = doc.text();

        let line_text = source.lines().nth(line as usize).unwrap_or("");
        let byte_col = utf16_to_byte_offset(line_text, character as usize);

        let Some((sig, active_param)) = signature::signature_help(
            &self.index,
            &self.pool,
            tree,
            &source,
            line as usize,
            byte_col,
        ) else {
            return Ok(None);
        };

        let parameters: Vec<ParameterInformation> = sig
            .params
            .iter()
            .map(|p| ParameterInformation {
                label: ParameterLabel::Simple(p.label()),
                documentation: None,
            })
            .collect();

        Ok(Some(SignatureHelp {
            signatures: vec![SignatureInformation {
                label: sig.label(),
                documentation: None,
                parameters: Some(parameters),
                active_parameter: Some(active_param as u32),
            }],
            active_signature: Some(0),
            active_parameter: Some(active_param as u32),
        }))
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = params
            .text_document_position_params
            .text_document
            .uri
            .to_string();
        let line = params.text_document_position_params.position.line;
        let character = params.text_document_position_params.position.character;

        let Some(doc) = self.documents.get(&uri) else {
            return Ok(None);
        };
        let Some(tree) = doc.tree() else {
            return Ok(None);
        };
        let source = doc.text();
        let Some((name, range)) = Self::name_at_position(tree, &source, line, character) else {
            return Ok(None);
        };

        let symbols = resolve::resolve_name(&self.index, &QualifiedName::new(name));
        if let Some(sym) = symbols.first() {
            return Ok(Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: hover::hover_for_symbol(sym),
                }),
                range: Some(range),
            }));
        }

        Ok(None)
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let uri = params
            .text_document_position_params
            .text_document
            .uri
            .to_string();
        let line = params.text_document_position_params.position.line;
        let character = params.text_document_position_params.position.character;

        let Some(doc) = self.documents.get(&uri) else {
            return Ok(None);
        };
        let Some(tree) = doc.tree() else {
            return Ok(None);
        };
        let source = doc.text();
        let Some((name, _)) = Self::name_at_position(tree, &source, line, character) else {
            return Ok(None);
        };

        let symbols = resolve::resolve_name(&self.index, &QualifiedName::new(name));
        let locations: Vec<Location> = symbols
            .iter()
            .filter_map(|sym| {
                Some(Location {
                    uri: Url::parse(&sym.uri).ok()?,
                    range: symbol_range_with_source(sym, &self.documents),
                })
            })
            .collect();

        match locations.len() {
            0 => Ok(None),
            1 => Ok(Some(GotoDefinitionResponse::Scalar(
                locations.into_iter().next().unwrap(),
            ))),
            _ => Ok(Some(GotoDefinitionResponse::Array(locations))),
        }
    }

    async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
        let uri = params.text_document_position.text_document.uri.to_string();
        let line = params.text_document_position.position.line;
        let character = params.text_document_position.position.character;

        let Some(doc) = self.documents.get(&uri) else {
            return Ok(None);
        };
        let Some(tree) = doc.tree() else {
            return Ok(None);
        };
        let source = doc.text();
        let Some((name, _)) = Self::name_at_position(tree, &source, line, character) else {
            return Ok(None);
        };

        let refs = self.index.find_references(&name);
        let locations: Vec<Location> = refs
            .iter()
            .filter_map(|r| {
                Some(Location {
                    uri: Url::parse(&r.uri).ok()?,
                    range: ref_range_with_source(r, &self.documents),
                })
            })
            .collect();

        if locations.is_empty() {
            Ok(None)
        } else {
            Ok(Some(locations))
        }
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        let uri = params.text_document.uri.to_string();
        let symbols = self.index.file_symbols(&uri);

        let lsp_symbols: Vec<SymbolInformation> = symbols
            .iter()
            .map(|sym| {
                symbol_to_symbol_information(sym, params.text_document.uri.clone(), &self.documents)
            })
            .collect();

        Ok(Some(DocumentSymbolResponse::Flat(lsp_symbols)))
    }

    async fn symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> Result<Option<Vec<SymbolInformation>>> {
        let symbols = self.index.search(&params.query);
        let lsp_symbols: Vec<SymbolInformation> = symbols
            .iter()
            .filter_map(|sym| {
                let uri = Url::parse(&sym.uri).ok()?;
                Some(symbol_to_symbol_information(sym, uri, &self.documents))
            })
            .collect();

        Ok(Some(lsp_symbols))
    }

    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> Result<Option<SemanticTokensResult>> {
        let uri = params.text_document.uri.to_string();

        let Some(doc) = self.documents.get(&uri) else {
            return Ok(None);
        };
        let Some(tree) = doc.tree() else {
            return Ok(None);
        };

        let source = doc.text();
        let tokens = semantic_tokens::collect_semantic_tokens(tree.root_node(), &source);

        Ok(Some(SemanticTokensResult::Tokens(SemanticTokens {
            result_id: None,
            data: tokens,
        })))
    }

    async fn folding_range(&self, params: FoldingRangeParams) -> Result<Option<Vec<FoldingRange>>> {
        let uri = params.text_document.uri.to_string();

        let Some(doc) = self.documents.get(&uri) else {
            return Ok(None);
        };
        let Some(tree) = doc.tree() else {
            return Ok(None);
        };

        let mut ranges = Vec::new();
        collect_folding_ranges(tree.root_node(), &mut ranges);
        Ok(Some(ranges))
    }

    async fn rename(&self, params: RenameParams) -> Result<Option<WorkspaceEdit>> {
        let uri = params.text_document_position.text_document.uri.to_string();
        let line = params.text_document_position.position.line;
        let character = params.text_document_position.position.character;
        let new_name = params.new_name;

        let Some(doc) = self.documents.get(&uri) else {
            return Ok(None);
        };
        let Some(tree) = doc.tree() else {
            return Ok(None);
        };
        let source = doc.text();
        let Some((name, _)) = Self::name_at_position(tree, &source, line, character) else {
            return Ok(None);
        };

        let mut changes: std::collections::HashMap<Url, Vec<TextEdit>> =
            std::collections::HashMap::new();

        // Rename all references.
        for r in &self.index.find_references(&name) {
            if let Ok(ref_uri) = Url::parse(&r.uri) {
                changes.entry(ref_uri).or_default().push(TextEdit {
                    range: ref_range_with_source(r, &self.documents),
                    new_text: new_name.clone(),
                });
            }
        }

        // Rename definition name nodes (using the name_* position, not the full statement).
        let defs = resolve::resolve_name(&self.index, &QualifiedName::new(name));
        for sym in &defs {
            if let Ok(sym_uri) = Url::parse(&sym.uri) {
                let name_range = if let Some(doc) = self.documents.get(&sym.uri) {
                    let src = doc.text();
                    let lines: Vec<&str> = src.lines().collect();
                    Range {
                        start: make_position(sym.name_start_line, sym.name_start_col, &lines),
                        end: make_position(sym.name_end_line, sym.name_end_col, &lines),
                    }
                } else {
                    Range {
                        start: Position {
                            line: sym.name_start_line as u32,
                            character: sym.name_start_col as u32,
                        },
                        end: Position {
                            line: sym.name_end_line as u32,
                            character: sym.name_end_col as u32,
                        },
                    }
                };
                changes.entry(sym_uri).or_default().push(TextEdit {
                    range: name_range,
                    new_text: new_name.clone(),
                });
            }
        }

        Ok(Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }))
    }

    async fn formatting(&self, params: DocumentFormattingParams) -> Result<Option<Vec<TextEdit>>> {
        let uri = params.text_document.uri.to_string();

        let Some(doc) = self.documents.get(&uri) else {
            return Ok(None);
        };

        let source = doc.text();
        let options = pg_format::FormatOptions {
            style: self.format_style,
        };

        match pg_format::format_sql(&source, &options) {
            Ok(formatted) => {
                if formatted == source {
                    return Ok(None);
                }
                let line_count = source.lines().count();
                let (end_line, end_char) = if source.ends_with('\n') {
                    // Trailing newline: position is start of the next (empty) line.
                    (line_count as u32, 0)
                } else if line_count == 0 {
                    (0, 0)
                } else {
                    let last_line_len = source.lines().last().map(|l| l.len()).unwrap_or(0);
                    ((line_count - 1) as u32, last_line_len as u32)
                };
                Ok(Some(vec![TextEdit {
                    range: Range {
                        start: Position {
                            line: 0,
                            character: 0,
                        },
                        end: Position {
                            line: end_line,
                            character: end_char,
                        },
                    },
                    new_text: formatted,
                }]))
            }
            Err(e) => {
                tracing::warn!("formatting failed: {e}");
                Ok(None)
            }
        }
    }
}

fn collect_folding_ranges(node: tree_sitter::Node, ranges: &mut Vec<FoldingRange>) {
    let start_line = node.start_position().row as u32;
    let end_line = node.end_position().row as u32;

    if end_line > start_line {
        let is_foldable = matches!(
            node.kind(),
            "CreateStmt"
                | "CreateFunctionStmt"
                | "ViewStmt"
                | "SelectStmt"
                | "InsertStmt"
                | "UpdateStmt"
                | "DeleteStmt"
                | "CreateSchemaStmt"
                | "CreateTrigStmt"
                | "IndexStmt"
                | "TransactionStmt"
                | "DoStmt"
                | "comment"
        );

        if is_foldable {
            ranges.push(FoldingRange {
                start_line,
                start_character: None,
                end_line,
                end_character: None,
                kind: if node.kind() == "comment" {
                    Some(FoldingRangeKind::Comment)
                } else {
                    Some(FoldingRangeKind::Region)
                },
                collapsed_text: None,
            });
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_folding_ranges(child, ranges);
    }
}
