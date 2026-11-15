//! MNX DataForge core engine. Pure std, no dependencies, no I/O.
//! The Android UI talks to it through one JSON-in / JSON-out entry point: `api::call`.

pub mod api;
pub mod csv;
pub mod dedup;
pub mod export;
pub mod json;
pub mod leak;
pub mod script;
pub mod split;
pub mod stats;
pub mod text;
pub mod tools;
pub mod validate;
