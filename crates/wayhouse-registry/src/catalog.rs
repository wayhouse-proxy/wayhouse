//! The catalog fields shared by sniffer and plugin registries (index schema 2): categories,
//! tags, the newest version's summary and where the full version list lives.

use crate::index::check_https_url;

pub const MAX_CATEGORIES: usize = 3;
pub const MAX_TAGS: usize = 8;
const LABEL_LEN: std::ops::RangeInclusive<usize> = 2..=24;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CatalogProblem {
    #[error("{0:?} is not a valid category or tag: use 2 to 24 of a-z, 0-9 and '-'")]
    BadLabel(String),
    #[error("more than {MAX_CATEGORIES} categories")]
    TooManyCategories,
    #[error("more than {MAX_TAGS} tags")]
    TooManyTags,
    #[error("{0:?} is listed twice")]
    Duplicate(String),
    #[error("updated {0:?} is not a YYYY-MM-DD date")]
    BadDate(String),
    #[error("detail_url must be an https:// URL with a host and no user info, got {0:?}")]
    BadDetailUrl(String),
    #[error("list either `versions` or a `detail_url`, not both and not neither")]
    VersionsOrDetail,
    #[error("`latest`, `abi` and `signed` must describe the newest listed version")]
    LatestMismatch,
    #[error("unknown category {0:?} (the allowed list is categories.toml)")]
    UnknownCategory(String),
}

/// 2 to 24 characters of `a-z`, `0-9` and `-`.
pub fn valid_label(s: &str) -> bool {
    LABEL_LEN.contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Tags are written by people: trim, lowercase and drop repeats (first spelling wins).
pub fn normalize_tags(tags: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in tags {
        let t = t.trim().to_ascii_lowercase();
        if !out.contains(&t) {
            out.push(t);
        }
    }
    out
}

/// Syntax only: an external registry may use categories of its own.
pub fn check_labels(categories: &[String], tags: &[String]) -> Result<(), CatalogProblem> {
    if categories.len() > MAX_CATEGORIES {
        return Err(CatalogProblem::TooManyCategories);
    }
    if tags.len() > MAX_TAGS {
        return Err(CatalogProblem::TooManyTags);
    }
    for list in [categories, tags] {
        for (i, label) in list.iter().enumerate() {
            if !valid_label(label) {
                return Err(CatalogProblem::BadLabel(label.clone()));
            }
            if list[..i].contains(label) {
                return Err(CatalogProblem::Duplicate(label.clone()));
            }
        }
    }
    Ok(())
}

/// `YYYY-MM-DD`; the day is only checked as 1..=31 (it is a label, not a calendar).
pub fn valid_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| matches!(i, 4 | 7) || c.is_ascii_digit())
        && matches!(s[5..7].parse::<u8>(), Ok(1..=12))
        && matches!(s[8..10].parse::<u8>(), Ok(1..=31))
}

/// The newest inline version, as the catalog fields must describe it.
pub struct Newest<'a> {
    pub version: &'a semver::Version,
    pub abi: &'a str,
    pub signed: bool,
}

/// Everything about one catalog entry that does not depend on the kind of registry.
pub struct Catalog<'a> {
    pub categories: &'a [String],
    pub tags: &'a [String],
    pub updated: &'a str,
    pub detail_url: Option<&'a str>,
    pub latest: &'a semver::Version,
    pub abi: &'a str,
    pub signed: bool,
    /// `None` when the versions are not inline.
    pub newest: Option<Newest<'a>>,
}

impl Catalog<'_> {
    pub fn check(&self) -> Result<(), CatalogProblem> {
        check_labels(self.categories, self.tags)?;
        if !valid_date(self.updated) {
            return Err(CatalogProblem::BadDate(self.updated.to_owned()));
        }
        match (self.detail_url, &self.newest) {
            (Some(url), None) => {
                check_https_url(url).map_err(|_| CatalogProblem::BadDetailUrl(url.to_owned()))
            }
            (None, Some(n)) => {
                if n.version == self.latest && n.abi == self.abi && n.signed == self.signed {
                    Ok(())
                } else {
                    Err(CatalogProblem::LatestMismatch)
                }
            }
            _ => Err(CatalogProblem::VersionsOrDetail),
        }
    }
}

/// Parse `categories.toml`: `categories = ["game", "voip"]`.
pub fn parse_categories(text: &str) -> Result<Vec<String>, String> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct File {
        categories: Vec<String>,
    }
    let file: File = toml::from_str(text).map_err(|e| e.to_string())?;
    for (i, c) in file.categories.iter().enumerate() {
        if !valid_label(c) {
            return Err(CatalogProblem::BadLabel(c.clone()).to_string());
        }
        if file.categories[..i].contains(c) {
            return Err(CatalogProblem::Duplicate(c.clone()).to_string());
        }
    }
    Ok(file.categories)
}

/// Every category must be on the allowed list.
pub fn check_allowed(categories: &[String], allowed: &[String]) -> Result<(), CatalogProblem> {
    match categories.iter().find(|c| !allowed.contains(c)) {
        Some(c) => Err(CatalogProblem::UnknownCategory(c.clone())),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn labels_are_short_lowercase_slugs() {
        for ok in ["ab", "java-edition", "a2s", &"x".repeat(24)] {
            assert!(valid_label(ok), "{ok}");
        }
        for bad in ["a", "", "Game", "has space", "a_b", "ünï", &"x".repeat(25)] {
            assert!(!valid_label(bad), "{bad}");
        }
    }

    #[test]
    fn tags_are_trimmed_lowercased_and_deduplicated() {
        assert_eq!(
            normalize_tags(&s(&[" Minecraft ", "minecraft", "Java-Edition"])),
            s(&["minecraft", "java-edition"])
        );
    }

    #[test]
    fn label_limits_and_duplicates() {
        assert_eq!(
            check_labels(&s(&["a1", "b1", "c1", "d1"]), &[]),
            Err(CatalogProblem::TooManyCategories)
        );
        let nine: Vec<String> = (0..9).map(|i| format!("t{i}")).collect();
        assert_eq!(check_labels(&[], &nine), Err(CatalogProblem::TooManyTags));
        assert_eq!(
            check_labels(&[], &s(&["aa", "aa"])),
            Err(CatalogProblem::Duplicate("aa".into()))
        );
        assert_eq!(
            check_labels(&s(&["Game"]), &[]),
            Err(CatalogProblem::BadLabel("Game".into()))
        );
    }

    #[test]
    fn dates() {
        assert!(valid_date("2026-11-12"));
        for bad in [
            "2026-13-01",
            "2026-00-10",
            "2026-11-32",
            "26-11-12",
            "2026/11/12",
            "2026-11-1x",
        ] {
            assert!(!valid_date(bad), "{bad}");
        }
    }

    #[test]
    fn categories_file() {
        assert_eq!(
            parse_categories("categories = [\"game\", \"voip\"]").unwrap(),
            s(&["game", "voip"])
        );
        assert!(parse_categories("categories = [\"Game\"]").is_err());
        assert!(parse_categories("categories = [\"game\", \"game\"]").is_err());
        assert!(parse_categories("cats = []").is_err());
        assert_eq!(
            check_allowed(&s(&["game", "x1"]), &s(&["game"])),
            Err(CatalogProblem::UnknownCategory("x1".into()))
        );
    }
}
