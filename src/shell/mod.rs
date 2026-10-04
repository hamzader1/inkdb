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
        let mut pending = String::new();
        loop {
            let cmds = match Self::read_statements(&mut rl, &mut pending) {
                Ok(cmds) => cmds,
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

            for cmd in cmds {
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
    }

    pub fn test(database: &mut Database<DiskVfs>, cmd: &str) -> InkResult<()> {
        Self::exec(database, cmd)
    }

    pub fn run_query(database: &mut Database<DiskVfs>, cmd: &str) -> InkResult<()> {
        Self::exec(database, cmd)
    }
    fn exec(database: &mut Database<DiskVfs>, cmd: &str) -> InkResult<()> {
        let mut stmt = database.execute(cmd)?;
        for row in stmt.rows() {
            println!("{}", RowWrapper(row?));
        }
        Ok(())
    }

    fn split_statements(input: &str) -> (Vec<String>, String) {
        let mut complete = Vec::new();
        let mut start = 0;
        let mut quote: Option<char> = None;
        for (i, ch) in input.char_indices() {
            if let Some(q) = quote {
                if ch == q {
                    quote = None;
                }
            } else if ch == '\'' || ch == '"' {
                quote = Some(ch);
            } else if ch == ';' {
                let stmt = input[start..i].trim();
                if !stmt.is_empty() {
                    complete.push(stmt.to_string());
                }
                start = i + ch.len_utf8();
            }
        }
        (complete, input[start..].to_string())
    }

    fn read_statements(
        rl: &mut DefaultEditor,
        pending: &mut String,
    ) -> Result<Vec<String>, ReadlineError> {
        loop {
            let (complete, rest) = Self::split_statements(pending);
            if !complete.is_empty() {
                *pending = rest;
                for cmd in &complete {
                    rl.add_history_entry(cmd)?;
                }
                return Ok(complete);
            }
            let prompt = if pending.trim().is_empty() {
                "ink> "
            } else {
                " ... "
            };
            match rl.readline(prompt) {
                Ok(line) => {
                    if !pending.is_empty() {
                        pending.push(' ');
                    }
                    pending.push_str(line.trim());
                }
                Err(ReadlineError::Eof) => {
                    if pending.trim().is_empty() {
                        return Err(ReadlineError::Eof);
                    }
                    let cmd = std::mem::take(pending);
                    return Ok(vec![cmd]);
                }
                Err(e) => return Err(e),
            }
        }
    }
}
