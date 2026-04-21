//! Context Focusing — file-relationship graph for guiding Claude on what to read.
//!
//! Ported from atlas: builds a SQLite graph of files, their roles, co-change relationships,
//! and semantic embeddings to guide the agent toward relevant files and away from noise.

pub mod graph;
pub mod indexer;
pub mod query;
pub mod symbols;
pub mod assembler;

pub use graph::{open_readwrite, open_readonly, is_valid as graph_is_valid, update_embedding};
pub use indexer::{run_index, Meta};
pub use query::{query, query_with_read_boosts, query_hybrid, RankedFile};
pub use assembler::{assemble, GuidanceOutput, FileEntry};
pub use symbols::apply_structural;
