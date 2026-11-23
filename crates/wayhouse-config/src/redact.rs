//! Scrubbing plaintext secrets out of a stored config document (#474).
//!
//! A config revision is kept as the exact text that was submitted. Once a newer revision has
//! replaced it, nothing needs the plaintext `settings.admin.auth_token` and `settings.gossip.psk`
//! in it any more, so the controller rewrites them to [`REDACTED`]. The edit keeps comments and
//! layout when the value sits on the key's own line (every form an operator writes in practice);
//! any other layout falls back to re-serialising the document, and a last pass replaces whatever
//! copy of the value is still left. A value never survives.

use serde_norway::Value;

/// What a scrubbed secret reads as. It is shorter than [`crate::MIN_SECRET_LEN`], so the
/// controller refuses to submit a document that still carries it.
pub const REDACTED: &str = "[redacted]";

/// The two secret-valued settings, as key paths.
const PATHS: [[&str; 3]; 2] = [
    ["settings", "admin", "auth_token"],
    ["settings", "gossip", "psk"],
];

/// Values shorter than this are not searched for in the last pass: they would also match
/// unrelated text, and the controller has not accepted one this short since the strength rule.
const MIN_SWEEP: usize = 4;

/// A bare (non-string) token is swept only from this length on, and only as a whole token: a
/// secret passes the strength rule at 16 bytes, so a shorter token is not one, and a short one
/// (`true`, `8080`) would hit unrelated settings.
const MIN_TOKEN_SWEEP: usize = 8;

fn lookup<'a>(doc: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter().try_fold(doc, |v, k| v.get(*k))
}

/// A scalar's text: a string, a number or a bool, through any tag (`!!str`). A secret an
/// operator wrote as `auth_token: 12345678` is still a secret.
fn scalar_text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Tagged(t) => scalar_text(&t.value),
        _ => None,
    }
}

/// The plaintext values `doc` carries (a `{ secret: name }` reference is not one).
fn plaintext(doc: &Value) -> Vec<String> {
    PATHS
        .iter()
        .filter_map(|p| scalar_text(lookup(doc, p)?))
        .filter(|v| v != REDACTED)
        .collect()
}

/// What the last pass searches for besides the key's own line: the text of each string secret
/// (anywhere it repeats, a comment or an alias's copy). A secret that is not a string (a bare
/// number, say) has no text of its own to search for here: its normalized form (`1.5e3` is
/// `1500`) does not appear in the document, and a short one (`true`) would rewrite unrelated
/// matches. Those are swept by the raw token the line edit saw instead ([`edit_in_place`]).
fn sweep_values(doc: &Value) -> Vec<String> {
    PATHS
        .iter()
        .filter_map(|p| match lookup(doc, p)? {
            Value::String(s) if s != REDACTED => Some(s.clone()),
            _ => None,
        })
        .collect()
}

/// Whether either secret in `yaml` was scrubbed.
#[must_use]
pub fn has_redacted_secret(yaml: &str) -> bool {
    let Ok(doc) = serde_norway::from_str::<Value>(yaml) else {
        return false;
    };
    PATHS
        .iter()
        .any(|p| lookup(&doc, p).and_then(scalar_text).as_deref() == Some(REDACTED))
}

/// What [`redact`] found in a config text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Redaction {
    /// A document with no plaintext secret in it: nothing to hide.
    Clean,
    /// The text with its plaintext secrets replaced by [`REDACTED`].
    Scrubbed(String),
    /// Not a document, or one that could not be scrubbed: the text may still hold a secret, so
    /// a caller that must not leak one withholds it.
    Unreadable,
}

/// Looks for plaintext secrets in `yaml` and scrubs them; see [`Redaction`].
#[must_use]
pub fn redact(yaml: &str) -> Redaction {
    let Ok(doc) = serde_norway::from_str::<Value>(yaml) else {
        return Redaction::Unreadable;
    };
    if plaintext(&doc).is_empty() {
        return Redaction::Clean;
    }
    scrub(yaml, doc).map_or(Redaction::Unreadable, Redaction::Scrubbed)
}

/// `yaml` with its plaintext secrets replaced by [`REDACTED`], or `None` when it holds none (or is
/// not a document at all). Idempotent: its own output gives `None`.
#[must_use]
pub fn redact_secrets(yaml: &str) -> Option<String> {
    match redact(yaml) {
        Redaction::Scrubbed(out) => Some(out),
        Redaction::Clean | Redaction::Unreadable => None,
    }
}

fn scrub(yaml: &str, doc: Value) -> Option<String> {
    let mut values = sweep_values(&doc);
    let edited = edit_in_place(yaml).filter(|(t, _)| scrubbed(t));
    let mut raw_tokens = Vec::new();
    let mut out = edited.map(|(t, raw)| {
        raw_tokens = raw;
        t
    });
    let mut out = out.take().or_else(|| {
        let mut doc = doc;
        for p in PATHS {
            if let Some(v) = path_mut(&mut doc, &p) {
                if scalar_text(v).is_some() {
                    *v = Value::String(REDACTED.to_owned());
                }
            }
        }
        serde_norway::to_string(&doc).ok()
    })?;
    // Whatever else repeats the value (a comment, an anchor's copy) goes too.
    values.retain(|v| v.len() >= MIN_SWEEP);
    for v in &values {
        if out.contains(v.as_str()) {
            out = out.replace(v.as_str(), REDACTED);
        }
    }
    for token in raw_tokens.iter().filter(|t| t.len() >= MIN_TOKEN_SWEEP) {
        out = replace_token(&out, token);
    }
    Some(out)
}

/// `text` with each whole-token occurrence of `token` (not part of a longer word) replaced.
fn replace_token(text: &str, token: &str) -> String {
    let word = |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.');
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(token) {
        let before_ok = at == 0 || !word(rest.as_bytes()[at - 1]);
        let end = at + token.len();
        let after_ok = end == rest.len() || !word(rest.as_bytes()[end]);
        out.push_str(&rest[..at]);
        out.push_str(if before_ok && after_ok {
            REDACTED
        } else {
            token
        });
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

fn path_mut<'a>(doc: &'a mut Value, path: &[&str]) -> Option<&'a mut Value> {
    path.iter().try_fold(doc, |v, k| v.get_mut(*k))
}

/// Whether `text` parses and holds no plaintext secret.
fn scrubbed(text: &str) -> bool {
    serde_norway::from_str::<Value>(text).is_ok_and(|d| plaintext(&d).is_empty())
}

/// Replaces the value on each secret key's own line, keeping everything else byte for byte.
/// `None` when no line could be edited. Also returns the raw text of each bare value it replaced.
fn edit_in_place(yaml: &str) -> Option<(String, Vec<String>)> {
    let mut stack: Vec<(usize, String)> = Vec::new();
    let mut out = String::with_capacity(yaml.len());
    let mut edited = false;
    let mut raw = Vec::new();
    for line in yaml.split_inclusive('\n') {
        let body = line.trim_end_matches(['\n', '\r']);
        let trimmed = body.trim_start();
        let indent = body.len() - trimmed.len();
        let mut done = false;
        if let Some((key, rest)) = split_key(trimmed) {
            while stack.last().is_some_and(|(i, _)| *i >= indent) {
                stack.pop();
            }
            stack.push((indent, key.to_owned()));
            let path: Vec<&str> = stack.iter().map(|(_, k)| k.as_str()).collect();
            if PATHS.iter().any(|p| p[..] == path[..]) {
                if let Some((value_end, token)) = replace_scalar(rest) {
                    raw.extend(token);
                    let value_start = body.len() - rest.len();
                    out.push_str(&body[..value_start]);
                    out.push_str(&value_end);
                    out.push_str(&line[body.len()..]);
                    edited = true;
                    done = true;
                }
            }
        }
        if !done {
            out.push_str(line);
        }
    }
    edited.then_some((out, raw))
}

/// `key: rest` for a plain mapping key, else `None` (comments, list items, anything odd).
fn split_key(trimmed: &str) -> Option<(&str, &str)> {
    if trimmed.starts_with('#') || trimmed.starts_with("- ") {
        return None;
    }
    let colon = trimmed.find(':')?;
    let key = &trimmed[..colon];
    if key.is_empty()
        || !key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return None;
    }
    let rest = &trimmed[colon + 1..];
    (rest.is_empty() || rest.starts_with(' ')).then_some((key, rest))
}

/// For the text after `key:`, the same text with its scalar replaced; `None` when the value is
/// not a quoted or bare scalar on this line (a reference, a block scalar, a value on the next
/// line, an alias).
fn replace_scalar(rest: &str) -> Option<(String, Option<String>)> {
    let value = rest.trim_start();
    let lead = &rest[..rest.len() - value.len()];
    let first = value.chars().next()?;
    let end = match first {
        '"' => {
            let mut esc = false;
            value
                .char_indices()
                .skip(1)
                .find(|&(_, c)| {
                    let hit = c == '"' && !esc;
                    esc = c == '\\' && !esc;
                    hit
                })?
                .0
                + 1
        }
        '\'' => {
            let b = value.as_bytes();
            let mut i = 1;
            loop {
                let q = i + value[i..].find('\'')?;
                if b.get(q + 1) == Some(&b'\'') {
                    i = q + 2;
                } else {
                    break q + 1;
                }
            }
        }
        '#' | '|' | '>' | '{' | '[' | '&' | '*' | '!' | '%' | '@' | '`' => return None,
        _ => value.find(" #").unwrap_or(value.len()),
    };
    let tail = &value[end..];
    // Only a comment may follow the scalar.
    if !(tail.trim().is_empty() || tail.trim_start().starts_with('#')) {
        return None;
    }
    // A bare scalar's trailing spaces belong to the gap before a comment, keep them.
    let bare_trim = value[..end].trim_end().len();
    let (scalar_end, gap) = if first == '"' || first == '\'' {
        (end, "")
    } else {
        (bare_trim, &value[bare_trim..end])
    };
    let token = (first != '"' && first != '\'').then(|| value[..scalar_end].to_owned());
    Some((
        format!("{lead}\"{REDACTED}\"{gap}{}", &value[scalar_end.max(end)..]),
        token,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "s3cr3t-token-0123456789";

    fn doc(settings: &str) -> String {
        format!(
            "# fleet config\nsettings:\n{settings}pools:\n  - name: p\n    targets: [\"127.0.0.1:9\"]\n"
        )
    }

    fn leaks(out: &str) -> bool {
        out.contains(SECRET)
    }

    #[test]
    fn redact_tells_nothing_to_hide_from_text_it_could_not_read() {
        assert_eq!(redact("pools: []\n"), Redaction::Clean);
        assert_eq!(redact("settings: [auth_token: \"x"), Redaction::Unreadable);
        let leaky = doc(&format!("  admin:\n    auth_token: \"{SECRET}\"\n"));
        let Redaction::Scrubbed(out) = redact(&leaky) else {
            panic!("a plaintext token is scrubbed");
        };
        assert!(!out.contains(SECRET));
        assert_eq!(redact(&out), Redaction::Clean, "idempotent");
    }

    #[test]
    fn each_scalar_form_is_scrubbed_and_the_rest_of_the_text_is_kept() {
        for value in [
            format!("\"{SECRET}\""),
            format!("'{SECRET}'"),
            SECRET.to_owned(),
            format!("\"{SECRET}\"   # the admin token"),
            format!("{SECRET} # the admin token"),
        ] {
            let yaml = doc(&format!(
                "  admin:\n    listen: \"127.0.0.1:9900\"\n    auth_token: {value}\n"
            ));
            let out = redact_secrets(&yaml).expect(&value);
            assert!(!leaks(&out), "{value}: {out}");
            assert!(out.contains("# fleet config\n"), "{out}");
            assert!(out.contains("listen: \"127.0.0.1:9900\"\n"), "{out}");
            assert!(
                out.contains(&format!("auth_token: \"{REDACTED}\"")),
                "{out}"
            );
            assert_eq!(
                out.contains("# the admin token"),
                value.contains("# the"),
                "{out}"
            );
            assert!(has_redacted_secret(&out));
        }
    }

    #[test]
    fn a_quoted_value_with_a_hash_and_quotes_inside_is_scrubbed_whole() {
        let tricky = "pa#ss \"quoted\" 0123456789";
        let yaml = doc(&format!(
            "  admin:\n    auth_token: \"{}\"\n",
            tricky.replace('"', "\\\"")
        ));
        let out = redact_secrets(&yaml).unwrap();
        assert!(
            !out.contains("pa#ss") && !out.contains("0123456789"),
            "{out}"
        );
    }

    #[test]
    fn the_gossip_key_is_scrubbed_too_and_both_at_once() {
        let yaml = doc(&format!(
            "  admin:\n    auth_token: \"{SECRET}\"\n  gossip:\n    bind: \"10.0.0.1:7946\"\n    psk: \"another-psk-0123456789\"\n"
        ));
        let out = redact_secrets(&yaml).unwrap();
        assert!(!leaks(&out) && !out.contains("another-psk"), "{out}");
        assert_eq!(out.matches(REDACTED).count(), 2);
    }

    #[test]
    fn layouts_the_line_edit_cannot_place_still_never_leak() {
        for layout in [
            format!("  admin:\n    auth_token: |\n      {SECRET}\n"),
            format!("  admin:\n    auth_token:\n      \"{SECRET}\"\n"),
            format!("  admin: {{ auth_token: \"{SECRET}\" }}\n"),
            format!("  admin:\n    auth_token: \"{SECRET}\" # copy: {SECRET}\n"),
        ] {
            let out = redact_secrets(&doc(&layout)).unwrap_or_else(|| panic!("{layout}"));
            assert!(!leaks(&out), "{layout} -> {out}");
            assert!(has_redacted_secret(&out), "{layout} -> {out}");
        }
    }

    #[test]
    fn numbers_bools_and_tagged_scalars_are_secrets_too() {
        for value in [
            "1234567890123456",
            "!!str 1234567890123456",
            "!!str s3cr3t-0123456789",
            "\"1234567890123456\"",
            "true",
            "1.5e3",
        ] {
            let yaml = doc(&format!("  admin:\n    auth_token: {value}\n"));
            let out = redact_secrets(&yaml).unwrap_or_else(|| panic!("{value}"));
            assert!(
                !out.contains("1234567890123456") && !out.contains("s3cr3t-0123456789"),
                "{value} -> {out}"
            );
            assert!(has_redacted_secret(&out), "{value} -> {out}");
            assert_eq!(redact_secrets(&out), None, "{value}: idempotent");
        }
    }

    #[test]
    fn an_alias_copy_of_a_bare_number_is_swept_by_its_raw_text_and_short_tokens_are_not() {
        // A hex number normalises to something that is not in the text (`0x1234567890abcdef` is
        // `1311768467294899695`); the raw token is.
        let yaml = format!(
            "{}# was 0x1234567890abcdef\nlimits:\n  retries: true\n  note: 0x1234567890abcdefx\n",
            doc("  admin:\n    auth_token: 0x1234567890abcdef\n")
        );
        let out = redact_secrets(&yaml).unwrap();
        assert!(!out.contains("# was 0x1234567890abcdef"), "{out}");
        assert!(
            out.contains("0x1234567890abcdefx"),
            "a longer word is not the token: {out}"
        );
        let short = doc("  admin:\n    auth_token: true\n");
        let out = redact_secrets(&format!("{short}flag: true\n")).unwrap();
        assert!(
            out.contains("flag: true"),
            "a short token is not swept: {out}"
        );
    }

    #[test]
    fn a_comment_that_repeats_the_value_is_scrubbed_too() {
        let yaml = format!(
            "# old token was {SECRET}\n{}",
            doc(&format!("  admin:\n    auth_token: \"{SECRET}\"\n"))
        );
        let out = redact_secrets(&yaml).unwrap();
        assert!(!leaks(&out), "{out}");
    }

    #[test]
    fn a_reference_a_missing_secret_and_a_scrubbed_value_are_left_alone() {
        assert_eq!(
            redact_secrets(&doc("  admin:\n    auth_token: { secret: admin-token }\n")),
            None
        );
        assert_eq!(
            redact_secrets(&doc("  admin:\n    listen: \"127.0.0.1:1\"\n")),
            None
        );
        assert_eq!(redact_secrets("pools: []\n"), None);
        let once =
            redact_secrets(&doc(&format!("  admin:\n    auth_token: \"{SECRET}\"\n"))).unwrap();
        assert_eq!(redact_secrets(&once), None, "idempotent");
    }

    #[test]
    fn a_token_key_outside_settings_is_not_touched() {
        let yaml = format!(
            "settings:\n  admin:\n    auth_token: \"{SECRET}\"\npools:\n  - name: p\n    targets: [\"127.0.0.1:9\"]\n    auth_token: \"unrelated-0123456789\"\n"
        );
        let out = redact_secrets(&yaml).unwrap();
        assert!(out.contains("unrelated-0123456789"), "{out}");
    }

    #[test]
    fn a_redacted_document_no_longer_passes_the_strength_rule() {
        let out =
            redact_secrets(&doc(&format!("  admin:\n    auth_token: \"{SECRET}\"\n"))).unwrap();
        assert!(REDACTED.len() < crate::MIN_SECRET_LEN);
        assert!(has_redacted_secret(&out));
        assert!(!has_redacted_secret(&doc(
            "  admin:\n    auth_token: \"0123456789abcdef0123\"\n"
        )));
    }
}
