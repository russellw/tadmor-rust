//! The Rust counterpart of tadmor: the same business management product,
//! specified by spec/ and checked by conformance/, on Axum, SQLx and Askama
//! (docs/stack.md).
//!
//! Business rules live in the service modules (auth, users, and those to
//! come), shared by the JSON API and the UI in http. Services return
//! `error::Error`, which carries the spec's HTTP status.

pub mod auth;
pub mod config;
pub mod db;
pub mod error;
pub mod http;
pub mod users;
