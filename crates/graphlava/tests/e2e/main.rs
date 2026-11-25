//! End-to-end scenarios through the public API on a real database file
//! ([[21-e2e-tests]]). Every test creates its file in a fresh temporary
//! directory and closes and reopens it before its final assertion.

mod catalogue;
mod datalog_query;
mod fixture;
mod hyperedges;
mod named_graphs;
mod planner;
mod sparql_query;
mod statement_store;
mod term_identity;
mod transactions;
