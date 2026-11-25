//! `graphlava`: the command-line tool of the graphlava metagraph database.
//!
//! Query and update with SPARQL 1.2 or Datalog, import and export RDF
//! (N-Triples, N-Quads, Turtle, TriG, RDF 1.2 included), run an interactive
//! shell, and inspect or maintain a database file.

mod admin;
mod cli;
mod error;
mod rdfio;
mod render;
mod run;
mod shell;

use std::io::{IsTerminal, Write};
use std::process::ExitCode;

use clap::Parser;

use crate::cli::Cli;

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(c) => c,
        Err(e) => {
            // clap prints help and version on stdout with exit code 0, and
            // usage errors on stderr with exit code 2.
            let _ = e.print();
            return ExitCode::from(if e.use_stderr() { 2 } else { 0 });
        }
    };
    match run::dispatch(cli) {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            let color = std::io::stdout().is_terminal()
                && std::io::stderr().is_terminal()
                && std::env::var_os("NO_COLOR").is_none();
            let mut err = std::io::stderr().lock();
            if color {
                let _ = writeln!(err, "\x1b[1;31merror:\x1b[0m {e}");
            } else {
                let _ = writeln!(err, "error: {e}");
            }
            ExitCode::from(e.exit_code())
        }
    }
}
