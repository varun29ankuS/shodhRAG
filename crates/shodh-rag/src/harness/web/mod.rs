//! Reaching the public web from agent tools: SSRF-safe HTTP, page text
//! extraction, web search providers and scholarly search.
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

pub mod client;
pub mod html;
pub mod papers;
pub mod search;
pub mod ssrf;

pub use client::{SafeClient, WebError};
