mod model;
mod parser;
mod printer;

use std::env;
use std::fs::File;
use std::io::{self, BufReader, Write};
use std::process::ExitCode;

use parser::CommitReader;

fn main() -> ExitCode {
    let path = env::args().nth(1);

    let reader: Box<dyn io::BufRead> = match path {
        Some(path) => match File::open(&path) {
            Ok(f) => Box::new(BufReader::new(f)),
            Err(e) => {
                eprintln!("commit-log-fmt: cannot open {}: {}", path, e);
                return ExitCode::FAILURE;
            }
        },
        None => Box::new(BufReader::new(io::stdin())),
    };

    let stdout = io::stdout();
    let mut out = stdout.lock();
    let mut count = 0usize;

    for result in CommitReader::new(reader) {
        match result {
            Ok(commit) => {
                if count > 0 {
                    let _ = writeln!(out);
                }
                let _ = write!(out, "{}", printer::pretty_print(&commit));
                count += 1;
            }
            Err(e) => {
                eprintln!("commit-log-fmt: {}", e);
                return ExitCode::FAILURE;
            }
        }
    }

    ExitCode::SUCCESS
}
