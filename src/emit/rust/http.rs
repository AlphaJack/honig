//! `operations.rs`, `client.rs`, `server.rs` and `documented_examples.rs` built
//! as syntax trees.
//!
//! Generated code must stay clean under strict clippy and rustdoc lint sets
//! without any `allow`. So there are no unused bindings or imports. Owned values
//! are moved and not cloned. Large variants are boxed.

mod client;
mod codec;
mod examples;
mod operations;
mod paths;
mod security;
mod server;
mod shared;

pub(super) use client::client;
pub(super) use examples::tests;
pub(super) use operations::operations;
pub(super) use server::{server, unset_error_headers};
pub(super) use shared::{has_hooks, is_path, listed};
