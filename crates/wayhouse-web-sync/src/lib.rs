//! `wayhouse-web-sync`: renders the `web:` section of a local `wayhouse` into one Caddy
//! JSON document and loads it through Caddy's admin API (design: the Caddy front-door
//! model, #297; spike findings #355).
//!
//! It is a separate process on purpose: a bug or hang here cannot touch game traffic.
//! The renderer ([`render`]) is a pure function; everything that does I/O sits behind
//! [`sync::Source`] and [`sync::Loader`] so the cycle logic is tested without sockets.

pub mod acme_forward;
pub mod admin_addr;
pub mod caddy;
pub mod certs;
pub mod metrics_defs;
pub mod pools;
pub mod render;
pub mod source;
pub mod sync;
