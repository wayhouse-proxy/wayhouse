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

fn lookup<'a>(doc: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter().try_fold(doc, |v, k| v.get(*k))
}

/// The plaintext values `doc` carries (a `{ secret: name }` reference is not one).
fn plaintext(doc: &Value) -> Vec<String> {
    PATHS
        .iter()
        .filter_map(|p| lookup(doc, p)?.as_str())
        .filter(|v| *v != REDACTED)
        .map(str::to_owned)
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
        .any(|p| lookup(&doc, p).and_then(Value::as_str) == Some(REDACTED))
}

/// `yaml` with its plaintext secrets replaced by [`REDACTED`], or `None` when it holds none (or is
/// not a document at all). Idempotent: its own output gives `None`.
#[must_use]
pub fn redact_secrets(yaml: &str) -> Option<String> {
    let doc: Value = serde_norway::from_str(yaml).ok()?;
    let values = plaintext(&doc);
    if values.is_empty() {
        return None;
    }
    let mut out = edit_in_place(yaml).filter(|t| scrubbed(t)).or_else(|| {
        let mut doc = doc;
        for p in PATHS {
            if let Some(v) = path_mut(&mut doc, &p) {
                if v.is_string() {
                    *v = Value::String(REDACTED.to_owned());
                }
            }
        }
        serde_norway::to_string(&doc).ok()
    })?;
    // Whatever else repeats the value (a comment, an anchor's copy) goes too.
    for v in values.iter().filter(|v| v.len() >= MIN_SWEEP) {
        if out.contains(v.as_str()) {
            out = out.replace(v.as_str(), REDACTED);
        }
    }
    Some(out)
}

fn path_mut<'a>(doc: &'a mut Value, path: &[&str]) -> Option<&'a mut Value> {
    path.iter().try_fold(doc, |v, k| v.get_mut(*k))
}

/// Whether `text` parses and holds no plaintext secret.
fn scrubbed(text: &str) -> bool {
    serde_norway::from_str::<Value>(text).is_ok_and(|d| plaintext(&d).is_empty())
}

/// Replaces the value on each secret key's own line, keeping everything else byte for byte.
/// `None` when no line could be edited.
fn edit_in_place(yaml: &str) -> Option<String> {
    let mut stack: Vec<(usize, String)> = Vec::new();
    let mut out = String::with_capacity(yaml.len());
    let mut edited = false;
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
                if let Some(value_end) = replace_scalar(rest) {
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
    edited.then_some(out)
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
fn replace_scalar(rest: &str) -> Option<String> {
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
    Some(format!(
        "{lead}\"{REDACTED}\"{gap}{}",
        &value[scalar_end.max(end)..]
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
