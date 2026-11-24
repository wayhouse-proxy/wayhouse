//! The one place a hop turns request-body bytes into a typed value.
//!
//! Same result as `serde_json::from_slice::<T>` (a trailing byte after the value is an error), but
//! behind one name: the typed target is the validation, and a body is never parsed anywhere else
//! by hand. Callers keep their own error mapping (status, redaction of the parser's message).

use serde::de::DeserializeOwned;

/// Parse `body` as exactly one JSON value of type `T`.
pub fn from_body<T: DeserializeOwned>(body: impl AsRef<[u8]>) -> Result<T, serde_json::Error> {
    let mut de = serde_json::Deserializer::from_slice(body.as_ref());
    let value = T::deserialize(&mut de)?;
    de.end()?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, serde::Deserialize)]
    struct Body {
        n: u32,
    }

    #[test]
    fn parses_bytes_and_strings() {
        assert_eq!(from_body::<Body>(br#"{"n":7}"#).unwrap(), Body { n: 7 });
        assert_eq!(
            from_body::<Body>(String::from(r#" {"n":7} "#)).unwrap(),
            Body { n: 7 }
        );
    }

    #[test]
    fn refuses_trailing_data_and_wrong_shapes() {
        assert!(from_body::<Body>(br#"{"n":7} x"#).is_err());
        assert!(from_body::<Body>(br#"{"n":"7"}"#).is_err());
        assert!(from_body::<Body>(b"").is_err());
    }
}
