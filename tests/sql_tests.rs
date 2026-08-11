use inkdb::sql::ast::{Affinity, Ast, Constraint};
use inkdb::sql::lexer::Lexer;
use inkdb::sql::parser::Parser;

fn parse(sql: &str) -> Ast {
    let tokens = Lexer::tokenize(sql).expect("lex failed");
    Parser::parse(tokens).expect("parse failed")
}

#[test]
fn create_table_basic() {
    let ast = parse("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL, score FLOAT)");
    match ast {
        Ast::CreateTableAst(t) => {
            assert_eq!(t.name, "users");
            assert_eq!(t.columns.len(), 3);
            assert_eq!(t.columns[0].name, "id");
            assert_eq!(t.columns[0].affinity, Affinity::Int);
            assert_eq!(
                t.columns[0].constraints.as_ref().unwrap(),
                &vec![Constraint::PrimaryKey]
            );
            assert_eq!(t.columns[1].affinity, Affinity::Text);
            assert_eq!(
                t.columns[1].constraints.as_ref().unwrap(),
                &vec![Constraint::NotNull]
            );
            assert_eq!(t.columns[2].affinity, Affinity::Float);
            assert!(t.columns[2].constraints.is_none());
        }
        other => panic!("expected CreateTable, got {other:?}"),
    }
}

#[test]
fn create_table_with_type_names() {
    let ast = parse("CREATE TABLE t (a VARCHAR(255), b DECIMAL(10, 2), c BLOB)");
    match ast {
        Ast::CreateTableAst(t) => {
            assert_eq!(t.columns[0].affinity, Affinity::Text);
            assert_eq!(t.columns[1].affinity, Affinity::Blob);
            assert_eq!(t.columns[2].affinity, Affinity::Blob);
        }
        other => panic!("expected CreateTable, got {other:?}"),
    }
}

#[test]
fn create_index_basic() {
    let ast = parse("CREATE INDEX idx_name ON users (name)");
    match ast {
        Ast::CreateIndexAst(i) => {
            assert_eq!(i.name, "idx_name");
            assert_eq!(i.table, "users");
            assert_eq!(i.columns, vec!["name".to_string()]);
            assert!(!i.unique);
        }
        other => panic!("expected CreateIndex, got {other:?}"),
    }
}

#[test]
fn create_unique_index() {
    let ast = parse("CREATE UNIQUE INDEX idx_email ON users (email, id)");
    match ast {
        Ast::CreateIndexAst(i) => {
            assert!(i.unique);
            assert_eq!(i.columns, vec!["email".to_string(), "id".to_string()]);
        }
        other => panic!("expected CreateIndex, got {other:?}"),
    }
}
