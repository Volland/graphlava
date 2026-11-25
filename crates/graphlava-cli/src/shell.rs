//! `graphlava shell`: an interactive read-eval-print loop.
//!
//! A statement ends with `;` at the end of a line (outside braces, brackets
//! and parentheses) or with an empty line. Lines starting with `.` are shell
//! commands. With stdin not a terminal, the shell reads statements from it
//! without prompts and exits with code 1 if any statement failed.

use std::io::{BufRead, IsTerminal};
use std::path::PathBuf;
use std::time::Instant;

use graphlava::{Database, Durability, QueryResult};

use crate::cli::{OutFormat, ShellArgs, ShellMode};
use crate::error::CliResult;
use crate::render::{DEFAULT_PREFIXES, Prefixes, write_result};
use crate::run::{Ctx, ms, open_or_create};

const HELP: &str = "\
Statements end with ';' at the end of a line, or with an empty line.

  .mode [sparql|datalog|update]   show or switch the language
  .format [table|json|csv|tsv]    show or switch the output format
  .prefix NAME: <IRI>             declare a prefix for every later statement
  .prefixes                       list the declared prefixes
  .timing on|off                  print row counts and timings
  .durability sync|ordered|relaxed  durability of updates (default ordered)
  .help                           this text
  .quit                           leave (also Ctrl-D)

Prefixes rdf: rdfs: xsd: owl: meta: and stmt: are always declared.";

struct Shell<'d> {
    db: &'d Database,
    ctx: Ctx,
    mode: ShellMode,
    format: OutFormat,
    /// Prefixes declared with `.prefix`.
    user: Vec<(String, String)>,
    timing: bool,
    durability: Durability,
    failed: bool,
}

/// Bracket depth of `text`, skipping strings, IRIs and comments.
fn depth(text: &str) -> i64 {
    let b = text.as_bytes();
    let mut d = 0i64;
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            q @ (b'"' | b'\'') => {
                i += 1;
                while i < b.len() && b[i] != q {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
            }
            b'<' => {
                if let Some(end) = text[i..].find(|c: char| c == '>' || c.is_whitespace()) {
                    if text[i + end..].starts_with('>') {
                        i += end;
                    }
                }
            }
            b'#' => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'{' | b'(' | b'[' => d += 1,
            b'}' | b')' | b']' => d -= 1,
            _ => {}
        }
        i += 1;
    }
    d
}

fn mode_name(m: ShellMode) -> &'static str {
    match m {
        ShellMode::Sparql => "sparql",
        ShellMode::Datalog => "datalog",
        ShellMode::Update => "update",
    }
}

fn format_name(f: OutFormat) -> &'static str {
    match f {
        OutFormat::Table => "table",
        OutFormat::Json => "json",
        OutFormat::Csv => "csv",
        OutFormat::Tsv => "tsv",
    }
}

impl Shell<'_> {
    fn prompt(&self, continuation: bool) -> String {
        let m = mode_name(self.mode);
        if continuation {
            format!(
                "{}> ",
                " ".repeat(m.len().saturating_sub(1)).replace(' ', ".") + "."
            )
        } else {
            format!("{m}> ")
        }
    }

    fn error(&mut self, msg: impl std::fmt::Display) {
        self.failed = true;
        let color = std::io::stderr().is_terminal()
            && std::io::stdout().is_terminal()
            && std::env::var_os("NO_COLOR").is_none();
        if color {
            eprintln!("\x1b[1;31merror:\x1b[0m {msg}");
        } else {
            eprintln!("error: {msg}");
        }
    }

    /// Runs a dot command; returns false to quit.
    fn command(&mut self, line: &str) -> bool {
        let line = line.trim().trim_end_matches(';').trim();
        let (cmd, arg) = match line.split_once(char::is_whitespace) {
            Some((c, a)) => (c, a.trim()),
            None => (line, ""),
        };
        match cmd {
            ".quit" | ".exit" | ".q" => return false,
            ".help" | ".h" => println!("{HELP}"),
            ".mode" => match arg {
                "" => println!("{}", mode_name(self.mode)),
                "sparql" | "query" => self.mode = ShellMode::Sparql,
                "datalog" => self.mode = ShellMode::Datalog,
                "update" => self.mode = ShellMode::Update,
                other => self.error(format!(
                    "unknown mode '{other}' (sparql, datalog or update)"
                )),
            },
            ".format" => match arg {
                "" => println!("{}", format_name(self.format)),
                "table" => self.format = OutFormat::Table,
                "json" => self.format = OutFormat::Json,
                "csv" => self.format = OutFormat::Csv,
                "tsv" => self.format = OutFormat::Tsv,
                other => self.error(format!(
                    "unknown format '{other}' (table, json, csv or tsv)"
                )),
            },
            ".prefix" if arg.is_empty() => self.list_prefixes(),
            ".prefix" => {
                let found = crate::render::declared_prefixes(&format!("prefix {arg}"));
                match found.first() {
                    Some((p, i)) => {
                        self.user.retain(|(q, _)| q != p);
                        self.user.push((p.clone(), i.clone()));
                    }
                    None => self.error("usage: .prefix NAME: <IRI>"),
                }
            }
            ".prefixes" => self.list_prefixes(),
            ".timing" => match arg {
                "on" => self.timing = true,
                "off" => self.timing = false,
                "" => println!("{}", if self.timing { "on" } else { "off" }),
                _ => self.error("usage: .timing on|off"),
            },
            ".durability" => match arg {
                "sync" => self.durability = Durability::Sync,
                "ordered" => self.durability = Durability::Ordered,
                "relaxed" => self.durability = Durability::Relaxed,
                "" => println!("{:?}", self.durability),
                _ => self.error("usage: .durability sync|ordered|relaxed"),
            },
            other => self.error(format!("unknown command '{other}'; try .help")),
        }
        true
    }

    fn list_prefixes(&self) {
        for (p, i) in &self.user {
            println!("{p}: <{i}>");
        }
        for (p, i) in DEFAULT_PREFIXES {
            if !self.user.iter().any(|(q, _)| q == p) {
                println!("{p}: <{i}>  (built in)");
            }
        }
    }

    /// The statement with the declared prefixes in front.
    fn with_prefixes(&self, text: &str) -> String {
        let mut head = String::new();
        let all = DEFAULT_PREFIXES
            .iter()
            .map(|(p, i)| (p.to_string(), i.to_string()))
            .filter(|(p, _)| !self.user.iter().any(|(q, _)| q == p))
            .chain(self.user.iter().cloned());
        for (p, i) in all {
            match self.mode {
                ShellMode::Datalog => head.push_str(&format!("@prefix {p}: <{i}> .\n")),
                _ => head.push_str(&format!("PREFIX {p}: <{i}>\n")),
            }
        }
        head.push_str(text);
        head
    }

    fn run(&mut self, text: &str) {
        let full = self.with_prefixes(text);
        let mut px = Prefixes::with_defaults();
        for (p, i) in &self.user {
            px.set(p, i);
        }
        px.add_declared(text);
        let t0 = Instant::now();
        let res: graphlava::Result<Option<QueryResult>> = match self.mode {
            ShellMode::Sparql => self.db.sparql(&full).map(Some),
            ShellMode::Datalog => self.db.datalog(&full).map(Some),
            ShellMode::Update => self.db.sparql_update(&full, self.durability).map(|rep| {
                if self.timing && !self.ctx.quiet {
                    eprintln!(
                        "inserted {}, deleted {}, updated in place {} ({})",
                        rep.inserted,
                        rep.deleted,
                        rep.updated_in_place,
                        ms(t0.elapsed())
                    );
                }
                None
            }),
        };
        let took = t0.elapsed();
        match res {
            Ok(Some(r)) => {
                let mut out = std::io::stdout().lock();
                match write_result(&mut out, &r, self.format, &px) {
                    Ok(p) => {
                        if self.timing && !self.ctx.quiet {
                            eprintln!("({}, {})", p.summary(), ms(took));
                        }
                    }
                    Err(e) => self.error(e),
                }
            }
            Ok(None) => {}
            Err(e) => {
                let lines = full.lines().count() - text.lines().count();
                self.error(shift_lines(&e.to_string(), lines));
            }
        }
    }
}

/// Rewrites `at L:C` positions in a parser message so they count from the
/// statement the user typed, not from the injected prefix lines.
fn shift_lines(msg: &str, by: usize) -> String {
    let Some(k) = msg.find("error at ") else {
        return msg.to_string();
    };
    let start = k + "error at ".len();
    let rest = &msg[start..];
    let digits = rest.chars().take_while(|c| c.is_ascii_digit()).count();
    match (
        rest[..digits].parse::<usize>(),
        rest[digits..].starts_with(':'),
    ) {
        (Ok(line), true) if line > by => {
            format!("{}{}{}", &msg[..start], line - by, &rest[digits..])
        }
        _ => msg.to_string(),
    }
}

enum Input {
    Line(String),
    Interrupted,
    Eof,
}

trait LineSource {
    fn read(&mut self, prompt: &str) -> Input;
    fn remember(&mut self, _entry: &str) {}
    fn save(&mut self, _path: &std::path::Path) {}
}

struct Piped(std::io::StdinLock<'static>);

impl LineSource for Piped {
    fn read(&mut self, _prompt: &str) -> Input {
        let mut s = String::new();
        match self.0.read_line(&mut s) {
            Ok(0) | Err(_) => Input::Eof,
            Ok(_) => Input::Line(s.trim_end_matches(['\n', '\r']).to_string()),
        }
    }
}

struct Interactive(rustyline::DefaultEditor);

impl LineSource for Interactive {
    fn read(&mut self, prompt: &str) -> Input {
        use rustyline::error::ReadlineError;
        match self.0.readline(prompt) {
            Ok(l) => Input::Line(l),
            Err(ReadlineError::Interrupted) => Input::Interrupted,
            Err(_) => Input::Eof,
        }
    }
    fn remember(&mut self, entry: &str) {
        let _ = self.0.add_history_entry(entry);
    }
    fn save(&mut self, path: &std::path::Path) {
        let _ = self.0.save_history(path);
    }
}

fn history_path(arg: Option<PathBuf>) -> Option<PathBuf> {
    arg.or_else(|| {
        std::env::var_os("GRAPHLAVA_HISTORY")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".graphlava_history"))
            })
    })
}

pub fn shell_cmd(ctx: Ctx, a: ShellArgs) -> CliResult<u8> {
    let db = open_or_create(&a.file)?;
    let interactive = std::io::stdin().is_terminal();
    let mut sh = Shell {
        db: &db,
        ctx,
        mode: a.mode,
        format: a.format,
        user: Vec::new(),
        timing: true,
        durability: Durability::Ordered,
        failed: false,
    };
    let history = history_path(a.history);
    let mut src: Box<dyn LineSource> = if interactive {
        let mut ed = rustyline::DefaultEditor::new().map_err(|e| {
            crate::error::CliError::Io(
                "cannot start the line editor".into(),
                std::io::Error::other(e.to_string()),
            )
        })?;
        if let Some(h) = &history {
            let _ = ed.load_history(h);
        }
        if !ctx.quiet {
            println!(
                "graphlava {} shell on {}. Statements end with ';' or an empty line; .help for commands.",
                crate::run::version_line(),
                a.file.display()
            );
        }
        Box::new(Interactive(ed))
    } else {
        Box::new(Piped(std::io::stdin().lock()))
    };
    let mut buf = String::new();
    loop {
        let prompt = sh.prompt(!buf.is_empty());
        let line = match src.read(&prompt) {
            Input::Line(l) => l,
            Input::Interrupted => {
                buf.clear();
                continue;
            }
            Input::Eof => break,
        };
        if buf.is_empty() {
            let t = line.trim();
            if t.is_empty() {
                continue;
            }
            if t.starts_with('.') {
                src.remember(t);
                if !sh.command(t) {
                    break;
                }
                continue;
            }
        }
        if line.trim().is_empty() {
            let stmt = std::mem::take(&mut buf);
            src.remember(stmt.trim_end());
            sh.run(&stmt);
            continue;
        }
        buf.push_str(&line);
        buf.push('\n');
        let t = line.trim_end();
        if t.ends_with(';') && depth(&buf) <= 0 {
            let stmt = std::mem::take(&mut buf);
            src.remember(stmt.trim_end());
            let s = stmt.trim_end();
            sh.run(&s[..s.len() - 1]);
        }
    }
    if !buf.trim().is_empty() {
        let stmt = std::mem::take(&mut buf);
        sh.run(&stmt);
    }
    if let Some(h) = &history {
        src.save(h);
    }
    let failed = sh.failed;
    drop(src);
    db.close()?;
    Ok(if failed && !interactive { 1 } else { 0 })
}

#[cfg(test)]
mod tests {
    use super::depth;

    #[test]
    fn shifts_positions() {
        assert_eq!(
            super::shift_lines("SPARQL syntax error: error at 8:16: expected X", 7),
            "SPARQL syntax error: error at 1:16: expected X"
        );
        assert_eq!(super::shift_lines("other", 3), "other");
    }

    #[test]
    fn bracket_depth() {
        assert_eq!(depth("SELECT * WHERE { ?s ?p ?o ;"), 1);
        assert_eq!(depth("SELECT * WHERE { ?s a <http://x#T> }"), 0);
        assert_eq!(depth("INSERT DATA { :a :b \"}\" }"), 0);
        assert_eq!(depth("# { comment\nASK {}"), 0);
        assert_eq!(depth("FILTER(?x < 3) {"), 1);
    }
}
