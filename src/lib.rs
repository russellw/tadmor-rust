//! The Rust counterpart of tadmor: the same business management product,
//! specified by spec/ and checked by conformance/, on Axum, SQLx and Askama
//! (docs/stack.md).
//!
//! Business rules live in the service modules (auth, banking, users, master,
//! calendar, currency, documents, inventory, orders, payments, posting,
//! settlement, reporting, yearend, and those to come), shared by the JSON
//! API and the UI in http. Services return `error::Error`, which carries
//! the spec's HTTP status.

pub mod auth;
pub mod banking;
pub mod calendar;
pub mod config;
pub mod currency;
pub mod db;
pub mod documents;
pub mod error;
pub mod http;
pub mod inventory;
pub mod master;
pub mod orders;
pub mod payments;
pub mod posting;
pub mod reporting;
pub mod settlement;
pub mod users;
pub mod yearend;
