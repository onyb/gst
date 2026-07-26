//! Core engine for preparing Indian GST returns offline.
//!
//! A pure library — importers, validation, summary computation, and portal
//! JSON generation — driven entirely by the machine-readable `spec/` files
//! embedded at build time. No I/O beyond what callers hand in, no network.

pub mod date;
pub mod generate;
pub mod gstin;
pub mod import;
pub mod masters;
pub mod payload;
pub mod record;
pub mod spec;
pub mod validate;
