//! The labels an instance reports about itself (`--region`, `--tag key=value`): one definition
//! of what is allowed, used by the proxy at startup and by the aggregator on every push.

use std::collections::BTreeMap;

/// Most tags one instance may report.
pub const MAX_TAGS: usize = 16;
const MAX_KEY: usize = 32;
const MAX_VALUE: usize = 64;
const MAX_REGION: usize = 64;
pub fn key_ok(key: &str) -> bool {
    let mut chars = key.chars();
    key.len() <= MAX_KEY
        && chars
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'))
}

fn value_ok(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value.chars().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ':' | '/' | '@' | '+')
        })
}

/// `Err` names the first label that breaks the rules: keys are lowercase letters, digits, `.`,
/// `_` and `-` (at most 32); values and the region use letters, digits and `. _ - : / @ +` (at
/// most 64, not empty); at most 16 tags.
pub fn check_labels(region: Option<&str>, tags: &BTreeMap<String, String>) -> Result<(), String> {
    if let Some(r) = region {
        if !value_ok(r, MAX_REGION) {
            return Err(format!(
                "region {r:?} is not 1 to {MAX_REGION} of A-Z a-z 0-9 . _ - : / @ +"
            ));
        }
    }
    if tags.len() > MAX_TAGS {
        return Err(format!("more than {MAX_TAGS} tags"));
    }
    for (k, v) in tags {
        if !key_ok(k) {
            return Err(format!(
                "tag key {k:?} is not 1 to {MAX_KEY} of a-z 0-9 . _ - starting with a letter or digit"
            ));
        }
        if !value_ok(v, MAX_VALUE) {
            return Err(format!(
                "value of tag {k:?} is not 1 to {MAX_VALUE} of A-Z a-z 0-9 . _ - : / @ +"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_checked() {
        let tags = |k: &str, v: &str| BTreeMap::from([(k.to_string(), v.to_string())]);
        assert!(check_labels(Some("eu-west-1"), &tags("env", "prod")).is_ok());
        assert!(check_labels(None, &BTreeMap::new()).is_ok());
        assert!(check_labels(Some(""), &BTreeMap::new()).is_err());
        assert!(check_labels(Some("a b"), &BTreeMap::new()).is_err());
        assert!(check_labels(None, &tags("Env", "prod")).is_err());
        assert!(check_labels(None, &tags("env", "")).is_err());
        assert!(check_labels(None, &tags("env", "pr od")).is_err());
        assert!(check_labels(None, &tags(&"k".repeat(33), "v")).is_err());
        assert!(check_labels(None, &tags("k", &"v".repeat(65))).is_err());
        let many: BTreeMap<_, _> = (0..17)
            .map(|i| (format!("k{i}"), "v".to_string()))
            .collect();
        assert!(check_labels(None, &many).is_err());
    }
}
