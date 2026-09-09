use super::planner::plan::{self, Plan};
use crate::vfs::file::SqliteFile;

pub struct Optimazer;

impl Optimazer {
    fn optimaze_select<F: SqliteFile>(plan: Plan<F>) {
        todo!()
    }
    
    
}
