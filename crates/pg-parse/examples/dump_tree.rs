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

    let sources = &[
        "CREATE TABLE users (id int, name text);",
        "CREATE TABLE public.users (id int);",
        "CREATE FUNCTION add(a int, b int) RETURNS int LANGUAGE sql AS 'SELECT a + b';",
        "CREATE FUNCTION test() RETURNS void LANGUAGE plpgsql AS $$BEGIN RAISE NOTICE 'hi'; END;$$;",
        "SET search_path = $$public$$;",
        "SELECT my_func(1, 'hello', 42);",
        "CREATE FUNCTION my_func(a int, b text, c int) RETURNS void LANGUAGE sql AS 'SELECT 1';",
    ];

    for source in sources {
        println!("=== {} ===", source);
        let tree = parser.parse(source, None).unwrap();
        dump(tree.root_node(), source, 0);
        println!();
    }
}
