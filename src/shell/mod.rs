use crate::InkResult;
use crate::backend::executor::RowWrapper;
use crate::db::Database;
use crate::vfs::disk::DiskVfs;
use rustyline::DefaultEditor;
use rustyline::error::ReadlineError;

pub struct InkShell;

impl InkShell {
    pub fn run(database: &mut Database<DiskVfs>) {
        let mut rl = DefaultEditor::new().expect("failed to initialize line editor");
        loop {
            let cmd = match Self::read_statement(&mut rl) {
                Ok(cmd) => cmd,
                Err(ReadlineError::Interrupted) => {
                    println!("^C");
                    break;
                }
                Err(ReadlineError::Eof) => break,
                Err(e) => {
                    eprintln!("input error: {e}");
                    continue;
                }
            };

            let cmd = cmd.trim();
            if cmd.is_empty() {
                continue;
            }
            match Self::exec(database, cmd) {
                Ok(()) => {}
                Err(e) => match crate::errors::render_syntax_error(cmd, &e) {
                    Some(rendered) => println!("{rendered}"),
                    None => println!("Error: {e}"),
                },
            }
        }
    }

    pub fn test(database: &mut Database<DiskVfs>, cmd: &str) -> InkResult<()> {
        Self::exec(database, cmd)
    }

    pub fn exec(database: &mut Database<DiskVfs>, cmd: &str) -> InkResult<()> {
        let mut stmt = database.execute(cmd)?;
        for row in stmt.rows() {
            println!("{}", RowWrapper(row?));
        }
        Ok(())
    }

    fn read_statement(rl: &mut DefaultEditor) -> Result<String, ReadlineError> {
        let mut buf = String::new();
        loop {
            let prompt = if buf.is_empty() { "ink> " } else { " ... " };
            let line = rl.readline(prompt)?;
            if !buf.is_empty() {
                buf.push(' ');
            }
            buf.push_str(line.trim());

            if buf.trim_end().ends_with(';') {
                break;
            }
        }
        if !buf.trim().is_empty() {
            rl.add_history_entry(buf.trim())?;
        }
        Ok(buf.trim().trim_end_matches(';').trim().to_string())
    }
}
