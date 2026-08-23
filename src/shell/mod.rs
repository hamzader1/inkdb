use crate::db::Database;
#[cfg(feature = "debug-instrument")]
use crate::debug::{breakpoint, trace};
use rustyline::DefaultEditor;
use rustyline::error::ReadlineError;
use std::time::Instant;

pub struct SqliteShell;
impl SqliteShell {
    pub fn run(database: &mut Database) {
        let mut rl = DefaultEditor::new().expect("Error initiliazing the shell");
        loop {
            let command = match SqliteShell::read_command(&mut rl) {
                Ok(cmd) => {
                    let start = Instant::now();
                    if cmd.starts_with(".debug ") {
                        Self::handle_debug_cmd(&cmd, database);
                    } else {
                        match database.execute(cmd.as_str()) {
                            Ok(_) => {}
                            Err(e) => {
                                println!("Runtime Error: {}", e);
                            }
                        }
                    }
                    let end = start.elapsed();
                    println!("Time elapsed: {:.2} s", end.as_secs_f64());
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

    #[cfg(feature = "debug-instrument")]
    fn handle_debug_cmd(cmd: &str, database: &mut Database) {
        let args: Vec<&str> = cmd.split_whitespace().collect();
        if args.len() < 2 {
            Self::print_debug_help();
            return;
        }

        match args[1] {
            "break" => {
                if args.len() >= 3 {
                    if let Ok(seq) = args[2].parse::<u64>() {
                        breakpoint::set_break_at_seq(seq);
                        println!("Breakpoint set at seq={}", seq);
                    } else {
                        println!("Invalid sequence number");
                    }
                } else {
                    println!("Usage: .debug break <seq>");
                }
            }
            "break-kind" => {
                if args.len() >= 3 {
                    let kind = match args[2].to_lowercase().as_str() {
                        "insert" => trace::EventKind::Insert,
                        "split" => trace::EventKind::Split,
                        "pagealloc" => trace::EventKind::PageAlloc,
                        "pagewrite" => trace::EventKind::PageWrite,
                        "pageread" => trace::EventKind::PageRead,
                        "overflow" => trace::EventKind::Overflow,
                        _ => {
                            println!("Unknown kind: {}", args[2]);
                            return;
                        }
                    };
                    breakpoint::set_break_at_kind(kind);
                    println!("Breakpoint set on kind={:?}", kind);
                } else {
                    println!(
                        "Usage: .debug break-kind <insert|split|pagealloc|pagewrite|pageread|overflow>"
                    );
                }
            }
            "break-row" => {
                if args.len() >= 3 {
                    if let Ok(row_id) = args[2].parse::<i64>() {
                        breakpoint::set_break_on_row_id(row_id);
                        println!("Breakpoint set on row_id={}", row_id);
                    } else {
                        println!("Invalid row_id");
                    }
                } else {
                    println!("Usage: .debug break-row <row_id>");
                }
            }
            "break-page" => {
                if args.len() >= 3 {
                    if let Ok(page_no) = args[2].parse::<u32>() {
                        breakpoint::set_break_on_page(page_no);
                        println!("Breakpoint set on page_no={}", page_no);
                    } else {
                        println!("Invalid page_no");
                    }
                } else {
                    println!("Usage: .debug break-page <page_no>");
                }
            }
            "traces" => {
                let start = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
                let end = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(u64::MAX);
                let events = trace::get_range(start, end);
                println!("Found {} events:", events.len());
                for ev in events {
                    println!("  {:?}", ev);
                }
            }
            "dump" => {
                if args.len() >= 3 {
                    if let Ok(seq) = args[2].parse::<u64>() {
                        let window = trace::get_range(seq.saturating_sub(10), seq + 10);
                        println!("=== STATE DUMP AT SEQ {} ===", seq);
                        println!("Trace window ({} events):", window.len());
                        for ev in &window {
                            println!("  {:?}", ev);
                        }
                        println!("=== END DUMP ===");
                    } else {
                        println!("Invalid sequence number");
                    }
                } else {
                    println!("Usage: .debug dump <seq>");
                }
            }
            "export" => {
                let path = args.get(2).map_or("trace.json", |v| v);
                trace::export_json(std::path::Path::new(path)).ok();
                println!("Exported trace to {}", path);
            }
            "clear" => {
                breakpoint::clear_breakpoints();
                println!("All breakpoints cleared");
            }
            "stats" => {
                let buf = trace::with(|t| {
                    println!("Trace buffer: {}/{} events", t.len(), t.capacity());
                    println!("Break hit count: {}", breakpoint::get_break_hit_count());
                });
            }
            _ => {
                Self::print_debug_help();
            }
        }
    }

    #[cfg(feature = "debug-instrument")]
    fn print_debug_help() {
        println!(".debug commands:");
        println!("  break <seq>           - Break at exact sequence number");
        println!(
            "  break-kind <kind>     - Break on event kind (insert|split|pagealloc|pagewrite|pageread|overflow)"
        );
        println!("  break-row <row_id>    - Break when row_id is inserted");
        println!("  break-page <page_no>  - Break when page is accessed");
        println!("  traces [start] [end]  - Show trace events in range");
        println!("  dump <seq>            - Show state dump at sequence");
        println!("  export [path]         - Export full trace to JSON");
        println!("  clear                 - Clear all breakpoints");
        println!("  stats                 - Show trace buffer stats");
    }

    #[cfg(not(feature = "debug-instrument"))]
    fn print_debug_help() {
        println!("Debug commands not available. Build with --features debug-instrument");
    }

    #[cfg(not(feature = "debug-instrument"))]
    fn handle_debug_cmd(_cmd: &str, _database: &mut Database) {
        println!("Debug commands not available. Build with --features debug-instrument");
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
                // Show help for dot commands
                if upper.as_str() == ".HELP" {
                    println!("SQL commands end with ';'");
                    println!("Dot commands:");
                    println!("  .help           - Show this help");
                    #[cfg(feature = "debug-instrument")]
                    println!("  .debug <cmd>    - Debug commands (see .debug help)");
                    println!("  .quit / .exit   - Exit shell");
                    buffer.clear();
                    continue;
                }
            }
        }
    }
}
