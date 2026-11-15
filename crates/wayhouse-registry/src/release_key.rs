//! The public key the maintainer signs the release feed with (`release-feed.json` on each
//! GitHub release). A separate key from the sniffer and plugin registry key: that one signs
//! code that runs inside the proxy, this one only a list of version numbers.
//!
//! Only the public half lives here. The private half is the `RELEASE_MINISIGN_SECRET_KEY`
//! and `RELEASE_MINISIGN_PASSWORD` secrets of the release workflow.

/// Base64 minisign public key (the second line of the `.pub` file). Key id `433303EA43BF7163`.
pub const RELEASE_FEED_PUBKEY: &str = "RWRjcb9D6gMzQ9OZjXQAwiguEBMxBU2mn54kULUS+BPkny5+9laZNJqf";
