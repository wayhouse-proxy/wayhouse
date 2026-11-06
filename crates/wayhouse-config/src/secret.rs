//! A config value that is either plaintext or a reference to a sealed cluster secret
//! (`K1`, #474): `psk: "text"` or `psk: { secret: gossip-psk }`.
//!
//! The config crate only parses the reference. Turning it into plaintext is the
//! caller's job through a [`SecretResolver`], and a reference that cannot be turned
//! into a non-empty value is an error here, never an empty token.

use std::fmt;

use serde::de::{self, Deserializer};
use serde::Deserialize;

/// What can go wrong turning a reference into a value. Every message names the secret and
/// the missing piece, and never contains a value.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SecretError {
    #[error("secret `{0}` is referenced, but this process has no secret store to read it from (a proxy without a controller cannot resolve `secret:` values yet; write the value in plaintext or connect the proxy to a controller)")]
    NoStore(String),
    #[error("secret `{0}` is not in the secret store")]
    Missing(String),
    #[error("secret `{name}` cannot be opened: {why}")]
    Unreadable { name: String, why: String },
    #[error("secret `{0}` is empty")]
    Empty(String),
}

/// Looks a cluster secret up by name. Implemented in the binary that holds the keyring.
pub trait SecretResolver {
    /// The plaintext of `name`.
    fn resolve(&self, name: &str) -> Result<String, SecretError>;
}

/// A plaintext value or a `{ secret: name }` reference.
#[derive(Clone, PartialEq, Eq)]
pub enum SecretOr {
    Plain(String),
    Ref(String),
}

impl SecretOr {
    #[must_use]
    pub fn plain(value: impl Into<String>) -> Self {
        Self::Plain(value.into())
    }

    /// True when the value sits in the config as plaintext.
    #[must_use]
    pub fn is_plain(&self) -> bool {
        matches!(self, Self::Plain(_))
    }

    /// The referenced secret's name, if this is a reference.
    #[must_use]
    pub fn secret_name(&self) -> Option<&str> {
        match self {
            Self::Ref(n) => Some(n),
            Self::Plain(_) => None,
        }
    }

    /// The plaintext, or an error for a reference nobody resolved. Fail closed: no caller
    /// can get an empty value out of an unresolved reference.
    pub fn resolve(&self, resolver: Option<&dyn SecretResolver>) -> Result<String, SecretError> {
        match self {
            Self::Plain(v) => Ok(v.clone()),
            Self::Ref(name) => {
                let value = resolver
                    .ok_or_else(|| SecretError::NoStore(name.clone()))?
                    .resolve(name)?;
                if value.is_empty() {
                    return Err(SecretError::Empty(name.clone()));
                }
                Ok(value)
            }
        }
    }
}

impl From<&str> for SecretOr {
    fn from(v: &str) -> Self {
        Self::Plain(v.to_owned())
    }
}

/// Never prints a plaintext value.
impl fmt::Debug for SecretOr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Plain(_) => f.write_str("Plain(<redacted>)"),
            Self::Ref(n) => write!(f, "Ref({n})"),
        }
    }
}

impl<'de> Deserialize<'de> for SecretOr {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(Visit)
    }
}

struct Visit;

/// A number or boolean is refused with an instruction instead of being turned into text: the
/// parser has already read `0123456789012345` as the number 123456789012345 and `1.5e3` as 1500,
/// so the value would silently differ from what was written.
fn unquoted<E: de::Error>(what: &str) -> E {
    de::Error::custom(format!(
        "a {what} here must be quoted so it is read as written: write it as a string, e.g. \"...\""
    ))
}

impl<'de> de::Visitor<'de> for Visit {
    type Value = SecretOr;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a string, or a mapping `{ secret: <name> }`")
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<SecretOr, E> {
        Ok(SecretOr::Plain(v.to_owned()))
    }

    fn visit_string<E: de::Error>(self, v: String) -> Result<SecretOr, E> {
        Ok(SecretOr::Plain(v))
    }

    fn visit_bool<E: de::Error>(self, _: bool) -> Result<SecretOr, E> {
        Err(unquoted("boolean"))
    }

    fn visit_i64<E: de::Error>(self, _: i64) -> Result<SecretOr, E> {
        Err(unquoted("number"))
    }

    fn visit_u64<E: de::Error>(self, _: u64) -> Result<SecretOr, E> {
        Err(unquoted("number"))
    }

    fn visit_f64<E: de::Error>(self, _: f64) -> Result<SecretOr, E> {
        Err(unquoted("number"))
    }

    fn visit_map<A: de::MapAccess<'de>>(self, mut m: A) -> Result<SecretOr, A::Error> {
        let bad = || de::Error::custom("a secret reference is exactly `{ secret: <name> }`");
        let Some((key, name)) = m.next_entry::<String, String>()? else {
            return Err(bad());
        };
        if key != "secret" || m.next_key::<String>()?.is_some() {
            return Err(bad());
        }
        if !wayhouse_secrets::valid_cluster_name(&name) {
            return Err(de::Error::custom(format!(
                "secret name {name:?} is not valid: use 1 to 63 of a-z, 0-9 and `-`, not starting or ending with `-`"
            )));
        }
        Ok(SecretOr::Ref(name))
    }
}

#[cfg(feature = "json-schema")]
impl schemars::JsonSchema for SecretOr {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "SecretOr".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "description": "A plaintext string (warned about) or a reference to a sealed cluster secret.",
            "anyOf": [
                { "type": "string" },
                {
                    "type": "object",
                    "properties": { "secret": { "type": "string", "pattern": "^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$" } },
                    "required": ["secret"],
                    "additionalProperties": false
                }
            ]
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(yaml: &str) -> Result<SecretOr, serde_norway::Error> {
        serde_norway::from_str(yaml)
    }

    struct Fixed(&'static str);
    impl SecretResolver for Fixed {
        fn resolve(&self, name: &str) -> Result<String, SecretError> {
            if name == "known" {
                Ok(self.0.to_owned())
            } else {
                Err(SecretError::Missing(name.to_owned()))
            }
        }
    }

    #[test]
    fn a_string_is_plaintext_and_a_mapping_is_a_reference() {
        assert_eq!(
            parse("hunter2hunter2").unwrap(),
            SecretOr::plain("hunter2hunter2")
        );
        assert_eq!(
            parse("{ secret: gossip-psk }").unwrap(),
            SecretOr::Ref("gossip-psk".into())
        );
    }

    #[test]
    fn a_reference_with_a_bad_name_or_extra_keys_is_refused() {
        for bad in [
            "{ secret: Gossip }",
            "{ secret: '' }",
            "{ secret: -a }",
            "{ secret: a, other: b }",
            "{ other: a }",
            "{}",
            "[a]",
            "5",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn an_unquoted_number_or_boolean_is_refused_with_an_instruction_to_quote_it() {
        for v in ["1234567890123456", "true", "1.5e3"] {
            let e = parse(v).unwrap_err().to_string();
            assert!(e.contains("must be quoted"), "{v}: {e}");
        }
        assert_eq!(
            parse("\"1234567890123456\"").unwrap(),
            SecretOr::plain("1234567890123456")
        );
        // Not a number to the parser, so it is read as written.
        assert_eq!(
            parse("0123456789012345").unwrap(),
            SecretOr::plain("0123456789012345")
        );
    }

    #[test]
    fn debug_never_shows_a_plaintext_value() {
        assert!(!format!("{:?}", SecretOr::plain("topsecretvalue")).contains("topsecret"));
        assert!(format!("{:?}", SecretOr::Ref("k".into())).contains('k'));
    }

    #[test]
    fn a_reference_without_a_resolver_fails_closed_and_names_the_secret() {
        let e = SecretOr::Ref("known".into()).resolve(None).unwrap_err();
        assert_eq!(e, SecretError::NoStore("known".into()));
        assert!(e.to_string().contains("`known`"));
    }

    #[test]
    fn a_missing_or_empty_secret_never_becomes_an_empty_value() {
        let r: &dyn SecretResolver = &Fixed("value-of-it");
        assert_eq!(
            SecretOr::Ref("known".into()).resolve(Some(r)).unwrap(),
            "value-of-it"
        );
        assert_eq!(
            SecretOr::Ref("other".into()).resolve(Some(r)).unwrap_err(),
            SecretError::Missing("other".into())
        );
        let empty: &dyn SecretResolver = &Fixed("");
        assert_eq!(
            SecretOr::Ref("known".into())
                .resolve(Some(empty))
                .unwrap_err(),
            SecretError::Empty("known".into())
        );
    }

    #[test]
    fn plaintext_resolves_to_itself_with_or_without_a_resolver() {
        assert_eq!(SecretOr::plain("abc").resolve(None).unwrap(), "abc");
        assert!(SecretOr::plain("abc").is_plain());
        assert_eq!(SecretOr::Ref("n".into()).secret_name(), Some("n"));
    }
}

#[cfg(test)]
mod in_config {
    use crate::{parse_str, SecretOr};

    const BODY: &str = "pools:\n  - name: mc\n    targets: [\"127.0.0.1:25566\"]\nlisteners:\n  - name: main\n    bind: \"0.0.0.0:25565\"\n    protocol: tcp\n    pool: mc\n";

    fn doc(version: &str, settings: &str) -> String {
        format!("{version}settings:\n{settings}{BODY}")
    }

    #[test]
    fn the_admin_token_and_gossip_psk_take_a_reference_in_schema_4() {
        let cfg = parse_str(&doc(
            "schema_version: 4\n",
            "  admin:\n    auth_token: { secret: admin-token }\n  failure_domain: eu\n  gossip:\n    bind: \"10.0.0.1:7946\"\n    psk: { secret: gossip-psk }\n",
        ))
        .unwrap();
        assert_eq!(
            cfg.admin_auth_token,
            Some(SecretOr::Ref("admin-token".into()))
        );
        assert_eq!(cfg.gossip.unwrap().psk, SecretOr::Ref("gossip-psk".into()));
    }

    #[test]
    fn plaintext_still_parses_at_any_schema() {
        let cfg = parse_str(&doc("", "  admin:\n    auth_token: a-long-enough-token\n")).unwrap();
        assert_eq!(
            cfg.admin_auth_token,
            Some(SecretOr::plain("a-long-enough-token"))
        );
    }

    #[test]
    fn a_reference_below_schema_4_is_refused_and_says_what_to_set() {
        for (s, path) in [
            ("  admin:\n    auth_token: { secret: t }\n", "settings.admin.auth_token"),
            (
                "  failure_domain: eu\n  gossip:\n    bind: \"10.0.0.1:7946\"\n    psk: { secret: t }\n",
                "settings.gossip.psk",
            ),
        ] {
            let e = parse_str(&doc("schema_version: 3\n", s)).unwrap_err().to_string();
            assert!(e.contains(path) && e.contains("schema_version 4"), "{e}");
        }
    }

    #[test]
    fn a_bad_secret_name_is_refused_with_the_rule() {
        let e = parse_str(&doc(
            "schema_version: 4\n",
            "  admin:\n    auth_token: { secret: Admin_Token }\n",
        ))
        .unwrap_err()
        .to_string();
        assert!(e.contains("not valid") && e.contains("a-z"), "{e}");
    }

    #[test]
    fn an_empty_plaintext_psk_is_still_refused() {
        let e = parse_str(&doc(
            "schema_version: 4\n",
            "  failure_domain: eu\n  gossip:\n    bind: \"10.0.0.1:7946\"\n    psk: \"\"\n",
        ))
        .unwrap_err()
        .to_string();
        assert!(e.contains("must not be empty"), "{e}");
    }

    struct Store;
    impl crate::SecretResolver for Store {
        fn resolve(&self, name: &str) -> Result<String, crate::SecretError> {
            Ok(format!("value-of-{name}-value"))
        }
    }

    const GOSSIP: &str = "  failure_domain: eu\n  gossip:\n    bind: \"10.0.0.1:7946\"\n    psk: ";

    #[test]
    fn plaintext_secrets_are_warned_about_and_references_are_not() {
        let plain = parse_str(&doc(
            "",
            &format!("  admin:\n    auth_token: a-long-enough-token\n{GOSSIP}a-long-enough-psk\n"),
        ))
        .unwrap();
        let w = plain.warnings().join("\n");
        assert!(
            w.contains("settings.admin.auth_token")
                && w.contains("settings.gossip.psk")
                && w.contains("secret:"),
            "{w}"
        );
        assert!(!w.contains("a-long-enough"), "{w}");
        let sealed = parse_str(&doc(
            "schema_version: 4\n",
            &format!("  admin:\n    auth_token: {{ secret: t }}\n{GOSSIP}{{ secret: p }}\n"),
        ))
        .unwrap();
        assert!(sealed.warnings().is_empty(), "{:?}", sealed.warnings());
    }

    #[test]
    fn the_secret_names_of_a_config_are_listed_once_and_sorted() {
        let both = parse_str(&doc(
            "schema_version: 4\n",
            &format!("  admin:\n    auth_token: {{ secret: zeta }}\n{GOSSIP}{{ secret: alpha }}\n"),
        ))
        .unwrap();
        assert_eq!(both.secret_names(), ["alpha", "zeta"]);
        let same = parse_str(&doc(
            "schema_version: 4\n",
            &format!("  admin:\n    auth_token: {{ secret: one }}\n{GOSSIP}{{ secret: one }}\n"),
        ))
        .unwrap();
        assert_eq!(same.secret_names(), ["one"]);
        assert!(parse_str(&doc("", "")).unwrap().secret_names().is_empty());
    }

    #[test]
    fn resolving_replaces_references_with_values_and_fails_without_a_store() {
        let cfg = parse_str(&doc(
            "schema_version: 4\n",
            &format!("  admin:\n    auth_token: {{ secret: t }}\n{GOSSIP}{{ secret: p }}\n"),
        ))
        .unwrap();
        let e = cfg.clone().resolve_secrets(None).unwrap_err();
        assert_eq!(e, crate::SecretError::NoStore("t".into()));
        let done = cfg.resolve_secrets(Some(&Store)).unwrap();
        assert_eq!(
            done.admin_auth_token,
            Some(SecretOr::plain("value-of-t-value"))
        );
        assert_eq!(
            done.gossip.unwrap().psk,
            SecretOr::plain("value-of-p-value")
        );
    }
}
