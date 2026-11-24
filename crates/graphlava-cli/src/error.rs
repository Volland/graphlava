//! Errors and exit codes.
//!
//! | code | meaning |
//! | --- | --- |
//! | 0 | success |
//! | 1 | the query, update, import or check failed |
//! | 2 | usage error (bad arguments) |
//! | 3 | the database could not be opened or written: locked, I/O, format |

use std::fmt;

/// An error that ends a command.
#[derive(Debug)]
pub enum CliError {
    /// Bad arguments that clap could not catch (exit code 2).
    Usage(String),
    /// A query, update, parse or check failure (exit code 1).
    Failed(String),
    /// An error from the database.
    Db(graphlava::Error),
    /// An I/O error outside the database (exit code 3).
    Io(String, std::io::Error),
}

impl CliError {
    pub fn exit_code(&self) -> u8 {
        use graphlava::Error as E;
        match self {
            CliError::Usage(_) => 2,
            CliError::Failed(_) => 1,
            CliError::Io(..) => 3,
            CliError::Db(e) => match e {
                E::Io(_)
                | E::Locked
                | E::Corrupted(_)
                | E::FormatMismatch(_)
                | E::Full
                | E::ReadOnly
                | E::Busy(_) => 3,
                _ => 1,
            },
        }
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CliError::Usage(m) | CliError::Failed(m) => f.write_str(m),
            CliError::Db(graphlava::Error::Locked) => f.write_str(
                "database file is locked by another process (graphlava allows one owner process per file)",
            ),
            CliError::Db(e) => write!(f, "{e}"),
            CliError::Io(ctx, e) => write!(f, "{ctx}: {e}"),
        }
    }
}

impl From<graphlava::Error> for CliError {
    fn from(e: graphlava::Error) -> Self {
        CliError::Db(e)
    }
}

pub type CliResult<T> = Result<T, CliError>;

/// Wraps an I/O error with what was being done.
pub fn io_ctx(ctx: impl Into<String>) -> impl FnOnce(std::io::Error) -> CliError {
    let ctx = ctx.into();
    move |e| CliError::Io(ctx, e)
}
