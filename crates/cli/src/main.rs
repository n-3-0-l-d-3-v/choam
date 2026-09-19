//! `choamc [--dir DIR] [-c SQL | -f FILE]`: with no -c/-f, reads SQL from
//! stdin (interactively if stdin is a terminal). Exits 1 if any statement
//! failed.

use std::io::{self, BufReader, IsTerminal};
use std::process::ExitCode;

use engine::Engine;

const USAGE: &str = "usage: choamc [--dir DIR] [-c SQL | -f FILE]\n  --dir DIR   database directory (default ./choam-data)\n  -c SQL      run the given SQL and exit\n  -f FILE     run the SQL in FILE and exit\n  (no -c/-f)  read SQL from stdin";

fn main() -> ExitCode {
    let mut dir = "./choam-data".to_string();
    let mut command: Option<String> = None;
    let mut file: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let slot = match a.as_str() {
            "--dir" => &mut dir,
            "-c" => command.get_or_insert_with(String::new),
            "-f" => file.get_or_insert_with(String::new),
            "-h" | "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("unknown argument {other:?}\n{USAGE}");
                return ExitCode::from(2);
            }
        };
        match args.next() {
            Some(v) => *slot = v,
            None => {
                eprintln!("{a} needs a value\n{USAGE}");
                return ExitCode::from(2);
            }
        }
    }

    let engine = match Engine::open(&dir) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("error: cannot open {dir:?}: {e}");
            return ExitCode::from(2);
        }
    };
    let mut session = engine.session();
    let (mut out, mut err) = (io::stdout().lock(), io::stderr().lock());

    let result = if let Some(sql) = command {
        choamc::run(
            &engine,
            &mut session,
            sql.as_bytes(),
            &mut out,
            &mut err,
            false,
        )
    } else if let Some(path) = file {
        match std::fs::File::open(&path) {
            Ok(f) => choamc::run(
                &engine,
                &mut session,
                BufReader::new(f),
                &mut out,
                &mut err,
                false,
            ),
            Err(e) => {
                eprintln!("error: cannot read {path:?}: {e}");
                return ExitCode::from(2);
            }
        }
    } else {
        let stdin = io::stdin();
        let interactive = stdin.is_terminal();
        choamc::run(
            &engine,
            &mut session,
            stdin.lock(),
            &mut out,
            &mut err,
            interactive,
        )
    };
    match result {
        Ok(false) => ExitCode::SUCCESS,
        Ok(true) => ExitCode::from(1),
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(2)
        }
    }
}
