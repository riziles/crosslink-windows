//! HTTP server module for `crosslink serve`.
//!
//! Provides the axum-based REST API and WebSocket hub for the web dashboard.
//! The server module is structured as:
//!
//! - `types` — serializable request/response types for the full API surface
//! - Future modules: `state`, `routes`, `handlers/`, `ws`, `watcher`

pub mod types;
