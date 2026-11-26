//! What protocol minor each peer speaks, so a sender can gate new optional
//! fields on the receiver (#185, the "gating rule" in
//! `docs/deep-dive/topics/http-toolkit.md`).
//!
//! Server side: [`gate`](crate::protocol::gate) records the caller's version as a
//! [`PeerProtocol`] request extension; a handler or SSE stream asks
//! [`receiver_supports`] once, on connect. There is no client-side memory of what a
//! peer last answered with: nothing sends an optional field to a peer yet, so one
//! was removed rather than kept unused (#262). When a first call site needs it, the
//! response header (`HEADER`) is what to read.

use crate::protocol::{PeerProtocol, ProtocolVersion};

/// Does the caller described by `ext` understand protocol minor `minor` (of
/// our major)? A caller that sent no header is treated as baseline.
pub fn receiver_supports(ext: &PeerProtocol, minor: u16) -> bool {
    ext.0.is_some_and(|v| supports(v, minor))
}

fn supports(v: ProtocolVersion, minor: u16) -> bool {
    v.major == ProtocolVersion::current().major && v.minor >= minor
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(major: u16, minor: u16) -> ProtocolVersion {
        ProtocolVersion { major, minor }
    }

    #[test]
    fn receiver_supports_reads_the_request_extension() {
        assert!(receiver_supports(&PeerProtocol(Some(v(1, 4))), 4));
        assert!(!receiver_supports(&PeerProtocol(Some(v(1, 4))), 5));
    }

    #[test]
    fn missing_extension_means_baseline() {
        assert!(!receiver_supports(&PeerProtocol(None), 0));
    }
}
