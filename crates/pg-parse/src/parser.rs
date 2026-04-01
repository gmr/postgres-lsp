use std::sync::Mutex;

use tree_sitter::Parser;

/// Language variant for the parser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Postgres,
    PlPgSql,
}

/// A pool of tree-sitter parsers for concurrent access.
///
/// Tree-sitter parsers are not `Send`, so each parse operation needs its own
/// parser instance. This pool recycles parsers to avoid repeated allocation.
pub struct ParserPool {
    postgres: Mutex<Vec<Parser>>,
    plpgsql: Mutex<Vec<Parser>>,
}

impl ParserPool {
    pub fn new() -> Self {
        Self {
            postgres: Mutex::new(Vec::new()),
            plpgsql: Mutex::new(Vec::new()),
        }
    }

    /// Acquire a parser for the given language. Returns a guard that
    /// automatically returns the parser to the pool on drop.
    pub fn acquire(&self, lang: Language) -> ParserGuard<'_> {
        let pool = match lang {
            Language::Postgres => &self.postgres,
            Language::PlPgSql => &self.plpgsql,
        };

        let parser = pool
            .lock()
            .unwrap()
            .pop()
            .unwrap_or_else(|| Self::create_parser(lang));

        ParserGuard {
            parser: Some(parser),
            pool,
        }
    }

    fn create_parser(lang: Language) -> Parser {
        let mut parser = Parser::new();
        let language: tree_sitter::Language = match lang {
            Language::Postgres => tree_sitter_postgres::LANGUAGE.into(),
            Language::PlPgSql => tree_sitter_postgres::LANGUAGE_PLPGSQL.into(),
        };
        parser
            .set_language(&language)
            .expect("failed to set tree-sitter language");
        parser
    }
}

impl Default for ParserPool {
    fn default() -> Self {
        Self::new()
    }
}

/// RAII guard that returns a parser to its pool on drop.
pub struct ParserGuard<'a> {
    parser: Option<Parser>,
    pool: &'a Mutex<Vec<Parser>>,
}

impl ParserGuard<'_> {
    pub fn parser_mut(&mut self) -> &mut Parser {
        self.parser.as_mut().unwrap()
    }
}

const MAX_POOL_SIZE: usize = 4;

impl Drop for ParserGuard<'_> {
    fn drop(&mut self) {
        if let Some(parser) = self.parser.take()
            && let Ok(mut pool) = self.pool.lock()
                && pool.len() < MAX_POOL_SIZE {
                    pool.push(parser);
                }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acquire_and_parse_postgres() {
        let pool = ParserPool::new();
        let mut guard = pool.acquire(Language::Postgres);
        let tree = guard
            .parser_mut()
            .parse("SELECT 1;", None)
            .expect("parse failed");
        assert_eq!(tree.root_node().kind(), "source_file");
    }

    #[test]
    fn acquire_and_parse_plpgsql() {
        let pool = ParserPool::new();
        let mut guard = pool.acquire(Language::PlPgSql);
        let tree = guard
            .parser_mut()
            .parse("BEGIN\n  RAISE NOTICE 'hello';\nEND;", None)
            .expect("parse failed");
        assert_eq!(tree.root_node().kind(), "source_file");
    }

    #[test]
    fn parser_reuse() {
        let pool = ParserPool::new();
        {
            let mut guard = pool.acquire(Language::Postgres);
            guard.parser_mut().parse("SELECT 1;", None).unwrap();
        }
        // Parser should be returned to the pool and reused.
        assert_eq!(pool.postgres.lock().unwrap().len(), 1);
        {
            let _guard = pool.acquire(Language::Postgres);
            assert_eq!(pool.postgres.lock().unwrap().len(), 0);
        }
        assert_eq!(pool.postgres.lock().unwrap().len(), 1);
    }
}
