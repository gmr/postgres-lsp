use tree_sitter::Parser;

fn dump(node: tree_sitter::Node, source: &str, indent: usize) {
    let prefix = "  ".repeat(indent);
    let text = if node.child_count() == 0 {
        format!(" {:?}", node.utf8_text(source.as_bytes()).unwrap_or(""))
    } else {
        String::new()
    };
    println!(
        "{}{} [{}-{}]{}",
        prefix,
        node.kind(),
        node.start_position(),
        node.end_position(),
        text
    );
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        dump(child, source, indent + 1);
    }
}

fn main() {
    let mut parser = Parser::new();
    let lang: tree_sitter::Language = tree_sitter_postgres::LANGUAGE.into();
    parser.set_language(&lang).unwrap();

    let sources = &["CREATE TABLE users (id int, name text);"];

    for source in sources {
        println!("=== {} ===", source);
        let tree = parser.parse(source, None).unwrap();
        dump(tree.root_node(), source, 0);
        println!();
    }

    // PL/pgSQL grammar
    let mut plpgsql_parser = Parser::new();
    let plpgsql_lang: tree_sitter::Language = tree_sitter_postgres::LANGUAGE_PLPGSQL.into();
    plpgsql_parser.set_language(&plpgsql_lang).unwrap();

    let plpgsql_sources = &[
        "DECLARE\n  v_count int;\nBEGIN\n  SELECT count(*) INTO v_count FROM users;\n  IF v_count > 0 THEN\n    RAISE NOTICE 'found %', v_count;\n  END IF;\n  RETURN v_count;\nEND;",
    ];

    for source in plpgsql_sources {
        println!("=== PL/pgSQL ===");
        let tree = plpgsql_parser.parse(source, None).unwrap();
        dump(tree.root_node(), source, 0);
        println!();
    }
}
