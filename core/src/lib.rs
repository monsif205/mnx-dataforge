//! MNX DataForge core engine. Pure std, no dependencies. Most ops are pure JSON-in/JSON-out; the file ops
//! (hash, archives, streaming readers) read files by path with fixed memory limits.
//! The Android UI talks to it through one JSON-in / JSON-out entry point: `api::call`.

pub mod api;
pub mod archive;
pub mod csv;
pub mod docx;
pub mod pdf;
pub mod dedup;
pub mod export;
pub mod hash;
pub mod html;
pub mod inflate;
pub mod intel;
pub mod json;
pub mod leak;
pub mod license;
pub mod nlp;
pub mod parquet;
pub mod reader;
pub mod script;
pub mod split;
pub mod stats;
pub mod unorm;
pub mod unorm_data;
pub mod text;
pub mod tools;
pub mod validate;
pub mod web;
pub mod zstd;
