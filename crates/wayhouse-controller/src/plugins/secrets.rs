//! The sealed-secret store primitives moved to `wayhouse-secrets` (shared with the proxy, which
//! unseals cluster secrets); this module keeps the old path for the plugin code.
pub use wayhouse_secrets::*;
