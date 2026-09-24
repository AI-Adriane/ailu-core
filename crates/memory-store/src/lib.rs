//! Rust port of `@ailu/memory-store`.
//!
//! A namespaced key/value store for agent memory. The public surface mirrors the
//! TypeScript package:
//!
//! - [`MemoryNamespace`], [`MemoryKey`], [`MemoryItem`] — the data shapes
//!   (serde camelCase, wire-compatible with the TS model).
//! - [`BaseStore`] — the async store contract (`get` / `put` / `delete` /
//!   `search` / `list`) plus its [`MemoryError`] type.
//! - [`InMemoryStore`] — the in-memory implementation, faithful to the TS
//!   `InMemoryStore` (insertion-ordered, substring `search`, prefix `list`,
//!   `createdAt` preserved across overwrites).
//!
//! ## Deferred: `PgStore`
//!
//! The TypeScript package also ships a Postgres-backed `PgStore` (its methods
//! currently throw "not implemented", with `search` reserved for pgvector). The
//! Rust port intentionally **does not** include `PgStore`: it is deferred to a
//! later wave so that this crate carries no database dependency. When it lands,
//! it will live in a `pg_store` module behind a feature flag and implement the
//! same [`BaseStore`] trait.

#![forbid(unsafe_code)]

mod in_memory_store;
mod interfaces;
mod types;

pub use in_memory_store::InMemoryStore;
pub use interfaces::{BaseStore, MemoryError};
pub use types::{MemoryItem, MemoryKey, MemoryNamespace};
