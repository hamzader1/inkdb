use std::ops::{Deref, DerefMut};

use crate::errors::SqliteError;
use crate::pager::pager::Pager;
use crate::record::Value;

pub mod create;
pub mod delete;
pub mod eval;
pub mod filter;
pub mod insert;
pub mod limit;
pub mod project;
pub mod tablescan;
pub mod transaction;

#[derive(Debug)]
pub struct Row {
    key: u64,
    data: Vec<Value<'static>>,
}

impl Row {
    pub fn new(key: u64, data: Vec<Value<'static>>) -> Self {
        Self { key, data }
    }
    pub fn key(&self) -> u64 {
        self.key
    }
    pub(crate) fn row(&self) -> &Vec<Value<'static>> {
        &self.data
    }
}
#[repr(transparent)]
pub struct RowWrapper(pub Row);
impl std::fmt::Display for RowWrapper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let row = self.0.deref();
        for (i, val) in row.iter().enumerate() {
            write!(f, "{}", val)?;
            if i < row.len() - 1 {
                write!(f, ", ")?;
            }
        }
        Ok(())
    }
}
impl Deref for Row {
    type Target = Vec<Value<'static>>;

    fn deref(&self) -> &Self::Target {
        &self.data
    }
}
impl DerefMut for Row {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.data
    }
}
