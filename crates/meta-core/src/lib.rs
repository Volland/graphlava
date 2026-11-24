//! meta-core: a mutable, single-file metagraph database for agent memory.
//!
//! The design lives in `tg.md/` (start at `tg.md/00-overview.md`); this crate is
//! the embeddable core. It is a placeholder until the engine bake-off lands.

/// The crate version, as set in `Cargo.toml`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_set() {
        assert!(!VERSION.is_empty());
    }
}
