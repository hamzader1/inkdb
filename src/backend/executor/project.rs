use crate::backend::analyze::MultiIndexColumn;
use crate::sql::parser::Arena;

use super::Executor;

pub struct Project<E: Executor> {
    child: E,
    arena: Arena,
    columns: Vec<MultiIndexColumn>,
}

// take the row
// apply eval on it
// return val
// replace it with the same index
// impl  {

// }
