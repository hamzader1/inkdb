use super::ast::Constraint as ColumnConstrait;
use super::ast::*;
use super::parser::Parser;
use super::tokens::TokenKind::{self, *};
use std::string::String;
impl Parser {
    fn parse_create(&mut self) -> Ast {
        // either Table, Index
        match self.peek() {
            Some(Table) => {
                self.next();
            }
            _ => panic!(),
        }
        todo!()
    }
    fn parse_create_table(&mut self) -> Ast {
        self.expect(&Table);
        let name = self.expect_ident();
        self.expect(&LeftParen);
        let mut columns: Vec<Column> = Vec::new();
        while !self.at(&RightParen) {}

        Ast::Null
    }
    fn parse_column(&mut self) -> Option<Column> {
        while !self.at(&Comma) {
            let name = self.expect_ident();
            let mut affinity: Option<Affinity> = None;
            let mut constraint = Vec::new();
            // can be data type or constraint

            while !self.at(&Comma) {
                match self.peek() {
                    Some(Integer) | Some(Text) | Some(Float) | Some(Blob) => {
                        let curr = self.next().unwrap().kind;
                        affinity = Some(Affinity::from(curr));
                    }

                    Some(Primary) => {
                        self.eat(&Primary);
                        self.eat(&Key);
                        constraint.push(ColumnConstrait::PrimaryKey);
                    }
                    Some(Unique) => {
                        self.eat(&Unique);
                        constraint.push(ColumnConstrait::Unique);
                    }
                    Some(Not) => {
                        self.eat(&Not);
                        self.eat(&Null);
                        constraint.push(ColumnConstrait::NotNull);
                    }
                    Some(NotNull) => {
                        self.eat(&NotNull);
                        constraint.push(ColumnConstrait::NotNull);
                    }
                    _ => todo!(),
                }
            }
        }
        None
    }
}
