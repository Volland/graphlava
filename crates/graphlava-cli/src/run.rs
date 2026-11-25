//! Command dispatch and the query, update and Datalog commands.

use std::io::{IsTerminal, Read, Write};
use std::path::Path;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use graphlava::{Database, OpenOptions, QueryOptions, QueryResult};

use crate::cli::{Cli, Command, OutFormat, QueryArgs, QueryOpts, TextInput, UpdateArgs};
use crate::error::{CliError, CliResult, io_ctx};
use crate::render::{Prefixes, write_result};

/// `graphlava --version`: the CLI version, the core version and the file
/// format version this build reads and writes.
pub fn version_line() -> &'static str {
    static LINE: OnceLock<String> = OnceLock::new();
    LINE.get_or_init(|| {
        format!(
            "{} (graphlava core {}, file format {})",
            env!("CARGO_PKG_VERSION"),
            graphlava::VERSION,
            graphlava::tables::FORMAT_VERSION
        )
    })
}

/// Global settings every command sees.
#[derive(Debug, Clone, Copy)]
pub struct Ctx {
    pub quiet: bool,
}

impl Ctx {
    /// A progress or summary line on stderr, unless `--quiet`.
    pub fn note(&self, msg: impl AsRef<str>) {
        if !self.quiet {
            eprintln!("{}", msg.as_ref());
        }
    }
}

pub fn dispatch(cli: Cli) -> CliResult<u8> {
    let ctx = Ctx { quiet: cli.quiet };
    match cli.command {
        Command::Query(a) => query_cmd(ctx, a),
        Command::Datalog(a) => crate::run::datalog_cmd(ctx, a),
        Command::Update(a) => update_cmd(ctx, a),
        Command::Import(a) => crate::rdfio::import_cmd(ctx, a),
        Command::Export(a) => crate::rdfio::export_cmd(ctx, a),
        Command::Shell(a) => crate::shell::shell_cmd(ctx, a),
        Command::Info(a) => crate::admin::info_cmd(ctx, &a.file),
        Command::Stats(a) => crate::admin::stats_cmd(ctx, &a.file),
        Command::Check(a) => crate::admin::check_cmd(ctx, &a.file),
        Command::Sweep(a) => crate::admin::sweep_cmd(ctx, a),
        Command::BuildOrdering(a) => crate::admin::build_ordering_cmd(ctx, a),
        Command::BuildIndex(a) => crate::admin::build_index_cmd(ctx, a),
    }
}

// ---- opening ----------------------------------------------------------------

/// Opens an existing database; a missing file is an error instead of a new,
/// empty database.
pub fn open_existing(path: &Path) -> CliResult<Database> {
    if !path.exists() {
        return Err(CliError::Io(
            format!("cannot open {}", path.display()),
            std::io::Error::new(std::io::ErrorKind::NotFound, "no such database file"),
        ));
    }
    open_or_create(path)
}

/// Opens a database, creating the file when it is missing.
pub fn open_or_create(path: &Path) -> CliResult<Database> {
    Ok(Database::open(path, OpenOptions::default())?)
}

// ---- input ------------------------------------------------------------------

/// Reads a file, or stdin for `-`.
pub fn read_path(p: &Path) -> CliResult<String> {
    if p.as_os_str() == "-" {
        return read_stdin();
    }
    std::fs::read_to_string(p).map_err(io_ctx(format!("cannot read {}", p.display())))
}

fn read_stdin() -> CliResult<String> {
    let mut s = String::new();
    std::io::stdin()
        .read_to_string(&mut s)
        .map_err(io_ctx("cannot read stdin"))?;
    Ok(s)
}

/// The text of a query or update: inline, `-f file`, or stdin.
pub fn read_text(input: &TextInput, what: &str) -> CliResult<String> {
    let text = match (&input.text, &input.file) {
        (Some(t), _) if t == "-" => read_stdin()?,
        (Some(t), _) => t.clone(),
        (None, Some(f)) => read_path(f)?,
        (None, None) => {
            if std::io::stdin().is_terminal() {
                return Err(CliError::Usage(format!(
                    "no {what} given: pass it as an argument, with -f FILE, or on stdin"
                )));
            }
            read_stdin()?
        }
    };
    if text.trim().is_empty() {
        return Err(CliError::Usage(format!("the {what} is empty")));
    }
    Ok(text)
}

/// Parses `500ms`, `5s`, `2m`, `1h` or plain (fractional) seconds.
pub fn parse_duration(s: &str) -> Result<Duration, String> {
    let s = s.trim();
    let (num, unit) = match s.find(|c: char| c.is_ascii_alphabetic()) {
        Some(k) => (&s[..k], &s[k..]),
        None => (s, "s"),
    };
    let v: f64 = num
        .trim()
        .parse()
        .map_err(|_| format!("invalid duration '{s}' (try 500ms, 5s or 2m)"))?;
    if !v.is_finite() || v < 0.0 {
        return Err(format!("invalid duration '{s}'"));
    }
    let secs = match unit {
        "ms" => v / 1000.0,
        "s" => v,
        "m" | "min" => v * 60.0,
        "h" => v * 3600.0,
        _ => return Err(format!("unknown unit in '{s}' (use ms, s, m or h)")),
    };
    Ok(Duration::from_secs_f64(secs))
}

pub fn ms(d: Duration) -> String {
    let ms = d.as_secs_f64() * 1000.0;
    if ms < 10.0 {
        format!("{ms:.2} ms")
    } else if ms < 10_000.0 {
        format!("{ms:.0} ms")
    } else {
        format!("{:.1} s", ms / 1000.0)
    }
}

pub fn query_options(o: &QueryOpts) -> QueryOptions {
    let mut q = if o.standard {
        QueryOptions::standard()
    } else {
        QueryOptions::default()
    };
    q.timeout = o.timeout;
    q.max_rows = o.max_rows;
    q
}

/// Cuts a Datalog result to `max` rows (Datalog has no row budget option).
pub fn truncate(res: &mut QueryResult, max: Option<usize>) {
    if let (Some(n), QueryResult::Solutions { rows, .. }) = (max, res) {
        rows.truncate(n);
    }
}

/// Prints a result on stdout and the summary on stderr.
pub fn print_result(
    ctx: Ctx,
    res: &QueryResult,
    format: OutFormat,
    px: &Prefixes,
    took: Duration,
) -> CliResult<()> {
    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());
    let printed = write_result(&mut out, res, format, px).map_err(io_ctx("cannot write output"))?;
    out.flush().map_err(io_ctx("cannot write output"))?;
    ctx.note(format!("{} ({})", printed.summary(), ms(took)));
    Ok(())
}

fn query_failed(e: graphlava::Error) -> CliError {
    CliError::Db(e)
}

// ---- commands ---------------------------------------------------------------

fn query_cmd(ctx: Ctx, a: QueryArgs) -> CliResult<u8> {
    let text = read_text(&a.input, if a.datalog { "program" } else { "query" })?;
    let db = open_existing(&a.file)?;
    let mut px = Prefixes::with_defaults();
    px.add_declared(&text);
    let t0 = Instant::now();
    let mut res = if a.datalog {
        db.datalog(&text).map_err(query_failed)?
    } else {
        db.sparql_with(&text, &query_options(&a.opts))
            .map_err(query_failed)?
    };
    if a.datalog {
        truncate(&mut res, a.opts.max_rows);
    }
    let took = t0.elapsed();
    print_result(ctx, &res, a.opts.format, &px, took)?;
    db.close()?;
    Ok(0)
}

pub fn datalog_cmd(ctx: Ctx, a: crate::cli::DatalogArgs) -> CliResult<u8> {
    let text = match &a.program {
        Some(p) => read_path(p)?,
        None => read_text(
            &TextInput {
                text: None,
                file: None,
            },
            "program",
        )?,
    };
    let db = if a.update {
        open_or_create(&a.file)?
    } else {
        open_existing(&a.file)?
    };
    let mut px = Prefixes::with_defaults();
    px.add_declared(&text);
    let t0 = Instant::now();
    let mut res = if a.update {
        db.datalog_update(&text, a.durability.into())?
    } else {
        db.datalog(&text)?
    };
    truncate(&mut res, a.max_rows);
    print_result(ctx, &res, a.format, &px, t0.elapsed())?;
    db.close()?;
    Ok(0)
}

fn update_cmd(ctx: Ctx, a: UpdateArgs) -> CliResult<u8> {
    let text = read_text(&a.input, "update")?;
    let db = open_or_create(&a.file)?;
    let t0 = Instant::now();
    let opts = if a.standard {
        QueryOptions::standard()
    } else {
        QueryOptions::default()
    };
    let rep = db.sparql_update_with(&text, a.durability.into(), &opts)?;
    ctx.note(format!(
        "inserted {}, deleted {}, updated in place {} ({})",
        rep.inserted,
        rep.deleted,
        rep.updated_in_place,
        ms(t0.elapsed())
    ));
    db.close()?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(parse_duration("500ms").unwrap(), Duration::from_millis(500));
        assert_eq!(parse_duration("2").unwrap(), Duration::from_secs(2));
        assert_eq!(parse_duration("1.5s").unwrap(), Duration::from_millis(1500));
        assert_eq!(parse_duration("2m").unwrap(), Duration::from_secs(120));
        assert!(parse_duration("abc").is_err());
        assert!(parse_duration("5y").is_err());
    }
}
