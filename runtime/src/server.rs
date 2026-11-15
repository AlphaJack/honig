//! The half of the generated server: the tower `Service`, routing, credential checks, serving an operation,
//! conditional and range requests and the wide event.

pub(crate) mod adapter;
pub(crate) mod auth;
pub(crate) mod conditional;
pub(crate) mod event;
pub(crate) mod operation;
pub(crate) mod range;
pub(crate) mod route;
pub(crate) mod serve;
pub(crate) mod service;
