//! The command tree (clap derive).

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

const ABOUT: &str = "Command-line tool for graphlava, the embedded metagraph database: \
query, update, import, export and inspect a database file.";

const AFTER_HELP: &str = "\
Examples:
  graphlava import memory.glv people.ttl
  graphlava query memory.glv 'SELECT ?s ?o WHERE { ?s <http://example.org/knows> ?o }'
  graphlava update memory.glv 'INSERT DATA { <http://example.org/a> <http://example.org/p> 1 }'
  graphlava datalog memory.glv reach.dl
  graphlava export memory.glv -o dump.nq
  graphlava shell memory.glv

Exit codes: 0 success, 1 query/update/check failure, 2 usage error,
3 database locked, unreadable or I/O error.

The file format is not frozen before 1.0: files written by this version may
not open in the next one. Documentation: https://graphlava.org";

#[derive(Debug, Parser)]
#[command(
    name = "graphlava",
    version = crate::run::version_line(),
    about = ABOUT,
    after_help = AFTER_HELP,
    propagate_version = true,
    disable_help_subcommand = true
)]
pub struct Cli {
    /// Print no progress or summary lines on stderr.
    #[arg(short, long, global = true)]
    pub quiet: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run a SPARQL 1.2 query (or a Datalog program with --datalog).
    #[command(after_help = "\
Examples:
  graphlava query db.glv 'SELECT * WHERE { ?s ?p ?o } LIMIT 10'
  graphlava query db.glv -f report.rq --format csv > report.csv
  echo 'ASK { ?s ?p ?o }' | graphlava query db.glv -
  graphlava query db.glv --datalog -f reach.dl

SELECT and ASK results print as an aligned table by default, or as SPARQL 1.1
Query Results JSON, CSV or TSV. CONSTRUCT and DESCRIBE print N-Triples.
Statement reifiers print as stmt:<id> in tables; that is
<https://graphlava.org/ns/stmt/<id>>, which you can use in later queries.")]
    Query(QueryArgs),

    /// Run a Datalog program; its ?- goal is the result.
    #[command(after_help = "\
Examples:
  graphlava datalog db.glv reach.dl
  graphlava datalog db.glv - --format json < reach.dl
  graphlava datalog db.glv --update derive.dl   # @assert heads write statements

Rules see triple(s, p, o[, id]), prop, role, in and in_star, as in SPARQL.")]
    Datalog(DatalogArgs),

    /// Run a SPARQL 1.2 update as one transaction.
    #[command(after_help = "\
Examples:
  graphlava update db.glv 'PREFIX : <http://example.org/>
    INSERT DATA { :alice :worksAt :acme {| :confidence 0.9 |} }'
  graphlava update db.glv -f changes.ru --durability sync
  cat changes.ru | graphlava update db.glv -")]
    Update(UpdateArgs),

    /// Load RDF (N-Triples, N-Quads, Turtle, TriG; RDF 1.2 included).
    #[command(after_help = "\
Examples:
  graphlava import db.glv people.ttl
  graphlava import db.glv dump.nq
  graphlava import db.glv episode.ttl --graph http://example.org/episode1
  curl -s https://example.org/data.nt | graphlava import db.glv - --format nt

Named graphs in N-Quads and TriG become graphs. RDF 1.2 reifiers and
annotations ({| ... |}) attach to the statement they reify: a blank-node
reifier is the statement's ID, an IRI reifier becomes an alias of it.
The import runs in relaxed batches and ends with one durable commit.")]
    Import(ImportArgs),

    /// Write the database (or one graph) as RDF.
    #[command(after_help = "\
Examples:
  graphlava export db.glv                       # N-Quads on stdout
  graphlava export db.glv -o dump.trig          # format from the extension
  graphlava export db.glv --graph http://example.org/episode1 --format ttl

Annotated statements export as RDF 1.2 reifiers (_:s<id> rdf:reifies <<( s p o )>>),
statements in object position as triple terms, graph nesting as
<child> <https://graphlava.org/ns#in> <parent> triples. N-Triples and Turtle
merge all graphs into one. Hyperedges export as their type and role triples.")]
    Export(ExportArgs),

    /// Start an interactive shell (SPARQL, Datalog or update mode).
    #[command(after_help = "\
Statements end with ';' at the end of a line (outside braces) or with an
empty line. Dot commands: .help .mode .format .prefix .prefixes .timing .quit

Examples:
  graphlava shell db.glv
  graphlava shell db.glv --mode datalog
  graphlava shell db.glv < script.txt")]
    Shell(ShellArgs),

    /// Show the file, its format version and feature flags.
    Info(DbArg),

    /// Count statements per predicate, graphs and nesting, hyperedge types.
    Stats(DbArg),

    /// Check the cross-table invariants; exit code 1 on a violation.
    Check(DbArg),

    /// Mark and sweep unreferenced dictionary terms.
    Sweep(SweepArgs),

    /// Build an on-demand ordering (pso, sop or ops).
    #[command(name = "build-ordering")]
    BuildOrdering(BuildOrderingArgs),

    /// Build a value index on a predicate, for range filters and ORDER BY.
    #[command(name = "build-index")]
    BuildIndex(BuildIndexArgs),
}

#[derive(Debug, Args)]
pub struct DbArg {
    /// The database file.
    pub file: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutFormat {
    /// Aligned columns for a terminal.
    Table,
    /// SPARQL 1.1 Query Results JSON.
    Json,
    /// SPARQL 1.1 Query Results CSV.
    Csv,
    /// SPARQL 1.1 Query Results TSV.
    Tsv,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum DurabilityArg {
    /// On disk when the command returns; its own commit.
    Sync,
    /// On disk when the command returns; shares a group commit.
    Ordered,
    /// Applied in memory; durable with a later durable commit.
    Relaxed,
}

impl From<DurabilityArg> for graphlava::Durability {
    fn from(d: DurabilityArg) -> Self {
        match d {
            DurabilityArg::Sync => graphlava::Durability::Sync,
            DurabilityArg::Ordered => graphlava::Durability::Ordered,
            DurabilityArg::Relaxed => graphlava::Durability::Relaxed,
        }
    }
}

#[derive(Debug, Args)]
pub struct TextInput {
    /// Inline text, or '-' to read stdin. Read from stdin when absent and
    /// no --file is given.
    pub text: Option<String>,
    /// Read the text from a file.
    #[arg(
        id = "input_file",
        short = 'f',
        long = "file",
        value_name = "PATH",
        conflicts_with = "text"
    )]
    pub file: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct QueryOpts {
    /// Output format.
    #[arg(long, value_enum, default_value_t = OutFormat::Table)]
    pub format: OutFormat,
    /// SPARQL 1.1 dataset semantics (GRAPH is direct, the default graph is
    /// only the file's default graph).
    #[arg(long)]
    pub standard: bool,
    /// Stop the query after this long: 500ms, 5s, 2m or plain seconds.
    #[arg(long, value_name = "DURATION", value_parser = crate::run::parse_duration)]
    pub timeout: Option<std::time::Duration>,
    /// Row budget: a SPARQL query fails when it produces more rows than
    /// this; Datalog output is cut to N rows.
    #[arg(long, value_name = "N")]
    pub max_rows: Option<usize>,
}

#[derive(Debug, Args)]
pub struct QueryArgs {
    /// The database file.
    pub file: PathBuf,
    #[command(flatten)]
    pub input: TextInput,
    /// The text is a Datalog program, not SPARQL.
    #[arg(long)]
    pub datalog: bool,
    #[command(flatten)]
    pub opts: QueryOpts,
}

#[derive(Debug, Args)]
pub struct DatalogArgs {
    /// The database file.
    pub file: PathBuf,
    /// The program file, or '-' for stdin.
    pub program: Option<PathBuf>,
    /// Write the facts of @assert heads (a write transaction).
    #[arg(long)]
    pub update: bool,
    /// Durability of the write with --update.
    #[arg(long, value_enum, default_value_t = DurabilityArg::Ordered)]
    pub durability: DurabilityArg,
    /// Output format.
    #[arg(long, value_enum, default_value_t = OutFormat::Table)]
    pub format: OutFormat,
    /// Print at most this many rows.
    #[arg(long, value_name = "N")]
    pub max_rows: Option<usize>,
}

#[derive(Debug, Args)]
pub struct UpdateArgs {
    /// The database file (created when missing).
    pub file: PathBuf,
    #[command(flatten)]
    pub input: TextInput,
    /// When the update counts as committed.
    #[arg(long, value_enum, default_value_t = DurabilityArg::Ordered)]
    pub durability: DurabilityArg,
    /// SPARQL 1.1 semantics (DELETE DATA without GRAPH touches only the
    /// default graph).
    #[arg(long)]
    pub standard: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum RdfFormat {
    /// N-Triples
    Nt,
    /// N-Quads
    Nq,
    /// Turtle
    Ttl,
    /// TriG
    Trig,
}

#[derive(Debug, Args)]
pub struct ImportArgs {
    /// The database file (created when missing).
    pub file: PathBuf,
    /// The RDF file, or '-' for stdin (then --format is required).
    pub data: PathBuf,
    /// Put default-graph triples into this named graph.
    #[arg(long, value_name = "IRI")]
    pub graph: Option<String>,
    /// Input format; by default from the file extension.
    #[arg(long, value_enum)]
    pub format: Option<RdfFormat>,
    /// Base IRI for relative IRIs (Turtle and TriG).
    #[arg(long, value_name = "IRI")]
    pub base: Option<String>,
    /// Statements per write batch.
    #[arg(long, default_value_t = 10_000, value_name = "N")]
    pub batch: usize,
}

#[derive(Debug, Args)]
pub struct ExportArgs {
    /// The database file.
    pub file: PathBuf,
    /// Export only this graph and the graphs nested under it.
    #[arg(long, value_name = "IRI")]
    pub graph: Option<String>,
    /// With --graph: only the graph itself, not the graphs nested under it.
    #[arg(long, requires = "graph")]
    pub direct: bool,
    /// Output format; by default from the -o extension, else N-Quads.
    #[arg(long, value_enum)]
    pub format: Option<RdfFormat>,
    /// Write to this file instead of stdout.
    #[arg(short, long, value_name = "PATH")]
    pub output: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ShellMode {
    Sparql,
    Datalog,
    Update,
}

#[derive(Debug, Args)]
pub struct ShellArgs {
    /// The database file (created when missing).
    pub file: PathBuf,
    /// Starting mode.
    #[arg(long, value_enum, default_value_t = ShellMode::Sparql)]
    pub mode: ShellMode,
    /// Starting output format.
    #[arg(long, value_enum, default_value_t = OutFormat::Table)]
    pub format: OutFormat,
    /// History file (default: ~/.graphlava_history).
    #[arg(long, value_name = "PATH")]
    pub history: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct SweepArgs {
    /// The database file.
    pub file: PathBuf,
    /// Keep terms interned less than this long ago.
    #[arg(long, default_value = "0s", value_parser = crate::run::parse_duration)]
    pub window: std::time::Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OrderingArg {
    Pso,
    Sop,
    Ops,
}

#[derive(Debug, Args)]
pub struct BuildOrderingArgs {
    /// The database file.
    pub file: PathBuf,
    /// The ordering to build.
    #[arg(value_enum)]
    pub ordering: OrderingArg,
}

#[derive(Debug, Args)]
pub struct BuildIndexArgs {
    /// The database file.
    pub file: PathBuf,
    /// The predicate IRI, bare or in <angle brackets>.
    pub predicate: String,
}
