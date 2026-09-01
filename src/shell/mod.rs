use crate::db::Database;
use crate::vfs::disk::DiskFile;
use rustyline::DefaultEditor;
use rustyline::error::ReadlineError;
use std::time::Instant;

pub struct SqliteShell;
impl SqliteShell {
    pub fn run(database: &mut Database<DiskFile>) {
        let mut rl = DefaultEditor::new().expect("Error initiliazing the shell");
        loop {
            let command = match SqliteShell::read_command(&mut rl) {
                Ok(cmd) => {
                    let start = Instant::now();
                    match database.execute(cmd.as_str()) {
                        Ok(_) => {}
                        Err(e) => {
                            println!("Runtime Error: {}", e);
                        }
                    }
                    let end = start.elapsed();
                    println!("Time elapsed: {:.3} s", end.as_secs_f64());
                    cmd
                }
                Err(ReadlineError::Interrupted) => {
                    println!("^C");
                    break;
                }
                Err(ReadlineError::Eof) => {
                    break;
                }
                Err(e) => {
                    eprintln!("Error: {}", e);
                    continue;
                }
            };

            if command.trim().is_empty() {
                continue;
            }
            if command.trim().eq_ignore_ascii_case("quit")
                || command.trim().eq_ignore_ascii_case("exit")
            {
                break;
            }
        }
    }

    fn read_command(rl: &mut DefaultEditor) -> Result<String, ReadlineError> {
        let mut buffer = String::new();
        let mut line_number = 0;

        loop {
            line_number += 1;

            let prompt = if line_number == 1 || buffer.is_empty() {
                "ink> "
            } else {
                "      -> "
            };

            let line = rl.readline(prompt)?;

            if !buffer.is_empty() {
                buffer.push(' ');
            }
            buffer.push_str(line.trim());

            if buffer.ends_with(';') {
                rl.add_history_entry(&buffer)?;

                return Ok(buffer.trim_end_matches(';').trim().to_string());
            }

            if line_number == 1 {
                let upper = buffer.trim().to_uppercase();
                if matches!(upper.as_str(), "QUIT" | "EXIT" | "HELP" | "STATUS") {
                    rl.add_history_entry(&buffer)?;
                    return Ok(buffer);
                }
            }
        }
    }
}
