//! The release feed: a small signed JSON file that says which wayhouse versions exist, so a
//! controller or the UI can show "a newer release is available". It never installs anything.
//!
//! `release.yml` builds it from the GitHub releases list ([`from_github_releases`]), signs it
//! with the release key and attaches it, and its `.minisig`, to the release. Readers fetch
//! `releases/latest/download/release-feed.json` and call [`verify_release_feed`].

use semver::Version;
use serde::{Deserialize, Serialize};

/// The only feed schema this crate understands.
pub const FEED_SCHEMA: u32 = 1;
/// Largest feed accepted (256 KiB), signature included in nothing: the bytes alone.
pub const MAX_FEED_BYTES: usize = 256 * 1024;
/// Most releases a feed lists (newest first).
pub const MAX_RELEASES: usize = 500;
/// The trusted comment every release feed signature carries, so a signature made for
/// anything else (a module, an index) cannot be replayed as a feed.
pub const FEED_CLAIM: &str = "wayhouse-v1 release-feed";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseFeed {
    pub schema: u32,
    /// Newest first.
    pub releases: Vec<ReleaseEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseEntry {
    pub version: Version,
    /// `YYYY-MM-DD` of the publish.
    pub date: String,
    /// Where the release notes are (https). Shown as a link, never fetched.
    pub notes_url: String,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum FeedError {
    #[error("the release feed is {0} bytes, over the {MAX_FEED_BYTES} byte limit")]
    TooLarge(usize),
    #[error("the release feed is not valid JSON of the expected shape: {0}")]
    Malformed(String),
    #[error("release feed schema {0} is not supported (this build reads {FEED_SCHEMA})")]
    Schema(u32),
    #[error("the release feed lists more than {MAX_RELEASES} releases")]
    TooMany,
    #[error("release {version}: {why}")]
    Entry { version: Version, why: &'static str },
    #[error("the release feed signature is not valid: {0}")]
    Signature(String),
}

/// Parse and validate a feed (no signature check: see [`verify_release_feed`]).
///
/// # Errors
/// Too large, not the expected JSON, an unknown schema, or an entry with a bad date or a
/// notes URL that is not plain `https://`.
pub fn parse_release_feed(bytes: &[u8]) -> Result<ReleaseFeed, FeedError> {
    if bytes.len() > MAX_FEED_BYTES {
        return Err(FeedError::TooLarge(bytes.len()));
    }
    let feed: ReleaseFeed =
        serde_json::from_slice(bytes).map_err(|e| FeedError::Malformed(e.to_string()))?;
    if feed.schema != FEED_SCHEMA {
        return Err(FeedError::Schema(feed.schema));
    }
    if feed.releases.len() > MAX_RELEASES {
        return Err(FeedError::TooMany);
    }
    for r in &feed.releases {
        let entry = |why| FeedError::Entry {
            version: r.version.clone(),
            why,
        };
        if !valid_date(&r.date) {
            return Err(entry("the date is not YYYY-MM-DD"));
        }
        if !valid_https(&r.notes_url) {
            return Err(entry("notes_url must be a plain https:// URL"));
        }
    }
    Ok(feed)
}

/// Check `sig` (minisign text) over `bytes` with `key_b64`, then parse. The signature's
/// trusted comment must be [`FEED_CLAIM`].
///
/// # Errors
/// A bad key or signature, a signature for something else, or anything [`parse_release_feed`]
/// refuses.
pub fn verify_release_feed(
    bytes: &[u8],
    sig: &[u8],
    key_b64: &str,
) -> Result<ReleaseFeed, FeedError> {
    let bad = |e: String| FeedError::Signature(e);
    let key = minisign_verify::PublicKey::from_base64(key_b64).map_err(|e| bad(e.to_string()))?;
    crate::verify::verify_claimed(&key, sig, bytes, FEED_CLAIM).map_err(bad)?;
    parse_release_feed(bytes)
}

/// The newest stable release above `current`, if any.
#[must_use]
pub fn newer_than<'a>(feed: &'a ReleaseFeed, current: &Version) -> Option<&'a ReleaseEntry> {
    feed.releases
        .iter()
        .filter(|r| r.version.pre.is_empty() && r.version > *current)
        .max_by(|a, b| a.version.cmp(&b.version))
}

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    html_url: String,
    published_at: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

/// Build a feed from the JSON `gh api repos/<owner>/<repo>/releases --paginate` prints (an
/// array, or several arrays one after another when paginated): stable releases whose tag is
/// `v<semver>`, newest first. Drafts, prereleases and other tags are skipped.
///
/// # Errors
/// The input is not release JSON, or a kept release has no usable date or URL.
pub fn from_github_releases(json: &str) -> Result<ReleaseFeed, FeedError> {
    let mut all: Vec<GithubRelease> = Vec::new();
    for page in serde_json::Deserializer::from_str(json).into_iter::<Vec<GithubRelease>>() {
        all.extend(page.map_err(|e| FeedError::Malformed(e.to_string()))?);
    }
    let mut releases = Vec::new();
    for r in all {
        let Some(version) = r
            .tag_name
            .strip_prefix('v')
            .and_then(|t| Version::parse(t).ok())
        else {
            continue;
        };
        if r.draft || r.prerelease || !version.pre.is_empty() {
            continue;
        }
        let date = r
            .published_at
            .as_deref()
            .and_then(|d| d.get(..10))
            .unwrap_or_default()
            .to_string();
        releases.push(ReleaseEntry {
            version,
            date,
            notes_url: r.html_url,
        });
    }
    releases.sort_by(|a, b| b.version.cmp(&a.version));
    releases.truncate(MAX_RELEASES);
    let feed = ReleaseFeed {
        schema: FEED_SCHEMA,
        releases,
    };
    // The same checks a reader applies, so the workflow fails instead of publishing a feed
    // every client would refuse.
    parse_release_feed(&to_json(&feed)?)?;
    Ok(feed)
}

/// The feed as pretty JSON with a trailing newline.
///
/// # Errors
/// Serialisation failed (not expected for these types).
pub fn to_json(feed: &ReleaseFeed) -> Result<Vec<u8>, FeedError> {
    let mut out =
        serde_json::to_vec_pretty(feed).map_err(|e| FeedError::Malformed(e.to_string()))?;
    out.push(b'\n');
    Ok(out)
}

fn valid_date(d: &str) -> bool {
    let b = d.as_bytes();
    b.len() == 10
        && b.iter().enumerate().all(|(i, c)| {
            if i == 4 || i == 7 {
                *c == b'-'
            } else {
                c.is_ascii_digit()
            }
        })
}

fn valid_https(u: &str) -> bool {
    u.strip_prefix("https://").is_some_and(|rest| {
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        !authority.is_empty()
            && !authority.contains('@')
            && u.len() <= 512
            && u.bytes().all(|c| c.is_ascii_graphic())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    fn feed() -> Vec<u8> {
        let f = ReleaseFeed {
            schema: FEED_SCHEMA,
            releases: vec![
                ReleaseEntry {
                    version: v("0.4.1"),
                    date: "2026-12-01".into(),
                    notes_url: "https://github.com/wayhouse-proxy/wayhouse/releases/tag/v0.4.1"
                        .into(),
                },
                ReleaseEntry {
                    version: v("0.4.0"),
                    date: "2026-11-20".into(),
                    notes_url: "https://github.com/wayhouse-proxy/wayhouse/releases/tag/v0.4.0"
                        .into(),
                },
            ],
        };
        to_json(&f).unwrap()
    }

    /// A throwaway key pair, its public half in the base64 form the constant uses, and the
    /// signature of `data` with `claim` as the trusted comment.
    fn sign(data: &[u8], claim: &str) -> (String, Vec<u8>) {
        let kp = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let sig =
            minisign::sign(Some(&kp.pk), &kp.sk, Cursor::new(data), Some(claim), None).unwrap();
        let boxed = kp.pk.to_box().unwrap().to_string();
        let b64 = boxed.lines().nth(1).unwrap().to_string();
        (b64, sig.to_string().into_bytes())
    }

    #[test]
    fn the_shipped_key_parses() {
        assert!(
            minisign_verify::PublicKey::from_base64(crate::release_key::RELEASE_FEED_PUBKEY)
                .is_ok()
        );
    }

    #[test]
    fn a_feed_signed_with_the_key_verifies_and_anything_else_does_not() {
        let bytes = feed();
        let (key, sig) = sign(&bytes, FEED_CLAIM);
        assert_eq!(
            verify_release_feed(&bytes, &sig, &key)
                .unwrap()
                .releases
                .len(),
            2
        );
        // Altered bytes.
        let mut altered = bytes.clone();
        altered[20] ^= 1;
        assert!(matches!(
            verify_release_feed(&altered, &sig, &key),
            Err(FeedError::Signature(_))
        ));
        // Another key.
        let (other, _) = sign(&bytes, FEED_CLAIM);
        assert!(matches!(
            verify_release_feed(&bytes, &sig, &other),
            Err(FeedError::Signature(_))
        ));
        // A signature made for something else (a module claim) is not a feed signature.
        let (key2, other_sig) = sign(&bytes, "wayhouse-v1 sniffer x 1.0.0 a b");
        assert!(matches!(
            verify_release_feed(&bytes, &other_sig, &key2),
            Err(FeedError::Signature(_))
        ));
    }

    #[test]
    fn a_feed_is_validated_before_anyone_trusts_it() {
        let bad = |s: &str| parse_release_feed(s.as_bytes()).unwrap_err();
        assert!(matches!(
            bad(r#"{"schema":2,"releases":[]}"#),
            FeedError::Schema(2)
        ));
        assert!(matches!(bad("not json"), FeedError::Malformed(_)));
        assert!(matches!(
            bad(r#"{"schema":1,"releases":[],"extra":1}"#),
            FeedError::Malformed(_)
        ));
        let entry = |date: &str, url: &str| {
            format!(
                r#"{{"schema":1,"releases":[{{"version":"0.4.0","date":"{date}","notes_url":"{url}"}}]}}"#
            )
        };
        assert!(
            parse_release_feed(entry("2026-11-20", "https://example.com/n").as_bytes()).is_ok()
        );
        for (d, u) in [
            ("20261120", "https://example.com/n"),
            ("2026-11-20", "http://example.com/n"),
            ("2026-11-20", "https://u:p@example.com/n"),
            ("2026-11-20", "javascript:alert(1)"),
            ("2026-11-20", "https://"),
        ] {
            assert!(
                matches!(bad(&entry(d, u)), FeedError::Entry { .. }),
                "{d} {u}"
            );
        }
        assert!(matches!(
            parse_release_feed(&vec![b' '; MAX_FEED_BYTES + 1]),
            Err(FeedError::TooLarge(_))
        ));
    }

    #[test]
    fn only_a_newer_stable_release_counts() {
        let f = parse_release_feed(&feed()).unwrap();
        assert_eq!(newer_than(&f, &v("0.4.0")).unwrap().version, v("0.4.1"));
        assert!(newer_than(&f, &v("0.4.1")).is_none());
        assert!(newer_than(&f, &v("0.5.0")).is_none());
        // A running prerelease of 0.4.1 is older than the stable 0.4.1.
        assert_eq!(
            newer_than(&f, &v("0.4.1-rc.1")).unwrap().version,
            v("0.4.1")
        );
    }

    #[test]
    fn the_feed_is_built_from_github_releases_stable_only_newest_first() {
        let json = r#"[
          {"tag_name":"v0.4.0","html_url":"https://github.com/o/r/releases/tag/v0.4.0","published_at":"2026-11-20T10:00:00Z","draft":false,"prerelease":false},
          {"tag_name":"v0.5.0-rc.1","html_url":"https://github.com/o/r/releases/tag/v0.5.0-rc.1","published_at":"2026-12-01T10:00:00Z","draft":false,"prerelease":true}
        ]
        [
          {"tag_name":"v0.3.1","html_url":"https://github.com/o/r/releases/tag/v0.3.1","published_at":"2026-10-01T10:00:00Z","draft":false,"prerelease":false},
          {"tag_name":"v0.9.9","html_url":"https://github.com/o/r/releases/tag/v0.9.9","published_at":"2026-12-02T10:00:00Z","draft":true,"prerelease":false},
          {"tag_name":"nightly","html_url":"https://github.com/o/r/releases/tag/nightly","published_at":"2026-12-02T10:00:00Z"}
        ]"#;
        let f = from_github_releases(json).unwrap();
        let got: Vec<_> = f
            .releases
            .iter()
            .map(|r| (r.version.to_string(), r.date.clone()))
            .collect();
        assert_eq!(
            got,
            [
                ("0.4.0".into(), "2026-11-20".into()),
                ("0.3.1".into(), "2026-10-01".into())
            ]
        );
        // The result is a feed every reader accepts.
        assert_eq!(parse_release_feed(&to_json(&f).unwrap()).unwrap(), f);
    }

    #[test]
    fn a_release_without_a_date_stops_the_workflow_instead_of_publishing() {
        let json = r#"[{"tag_name":"v0.4.0","html_url":"https://github.com/o/r/releases/tag/v0.4.0","published_at":null}]"#;
        assert!(matches!(
            from_github_releases(json),
            Err(FeedError::Entry { .. })
        ));
    }
}
