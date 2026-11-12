//! Resolves `{ secret: name }` config values (K1): the proxy fetches each secret's ciphertext from
//! the controller and opens it with its own key file. The controller never sends plaintext, and
//! a proxy without the key cannot read what it is sent.

use std::collections::HashMap;
use std::path::Path;

use wayhouse_config::{SecretError, SecretResolver};
use wayhouse_secrets::{Keyring, OpenError, Sealed};

/// The secrets a config names, already opened.
#[derive(Default)]
pub struct Resolved(HashMap<String, String>);

impl Drop for Resolved {
    fn drop(&mut self) {
        for v in self.0.values_mut() {
            zeroize::Zeroize::zeroize(v);
        }
    }
}

impl std::fmt::Debug for Resolved {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Resolved({} secrets, values redacted)", self.0.len())
    }
}

impl SecretResolver for Resolved {
    fn resolve(&self, name: &str) -> Result<String, SecretError> {
        self.0
            .get(name)
            .cloned()
            .ok_or_else(|| SecretError::Missing(name.to_owned()))
    }
}

/// What the controller answered for one secret.
pub enum Fetched {
    Sealed(Sealed),
    Absent,
}

/// Opens every named secret. `fetch` is the transport (an HTTP GET in the binary). The first
/// problem stops the start and names the secret and the missing piece; nothing is half-resolved.
pub fn open_all(
    names: &[String],
    keyring: Option<&Keyring>,
    mut fetch: impl FnMut(&str) -> Result<Fetched, SecretError>,
) -> Result<Resolved, SecretError> {
    let mut out = HashMap::new();
    for name in names {
        let Some(keyring) = keyring else {
            return Err(SecretError::Unreadable {
                name: name.clone(),
                why: "this proxy has no secret key (pass --secret-key-file)".into(),
            });
        };
        let sealed = match fetch(name)? {
            Fetched::Sealed(s) => s,
            Fetched::Absent => return Err(SecretError::Missing(name.clone())),
        };
        let value = keyring
            .open_cluster(name, &sealed)
            .map_err(|e| SecretError::Unreadable {
                name: name.clone(),
                why: match e {
                    OpenError::KeyMissing(id) => format!(
                        "it is sealed under key {id}, which is not in this proxy's key file"
                    ),
                    OpenError::Auth => {
                        "the stored value does not authenticate (wrong key, or it was changed)"
                            .into()
                    }
                    OpenError::Malformed | OpenError::Name => {
                        "the stored value is malformed".into()
                    }
                },
            })?;
        let text = String::from_utf8(zeroize::Zeroizing::new(value.expose().to_vec()).to_vec())
            .map_err(|_| SecretError::Unreadable {
                name: name.clone(),
                why: "the value is not valid UTF-8 text".into(),
            })?;
        out.insert(name.clone(), text);
    }
    Ok(Resolved(out))
}

/// Loads the proxy's key file (or the key from the environment); `None` when neither is given.
pub fn load_keyring(file: Option<&Path>) -> anyhow::Result<Option<std::sync::Arc<Keyring>>> {
    let handle = wayhouse_secrets::load(file).map_err(|e| anyhow::anyhow!("secret key: {e}"))?;
    Ok(handle.map(|h| h.current()))
}

/// Where a proxy finds sealed secrets: the controller, or its own file (`--secrets-file`).
pub enum Store<'a> {
    None,
    Controller(&'a str, Option<&'a str>),
    File(&'a Path),
}

/// Turns the config's `{ secret: name }` values into their values, once, at start. With no
/// reference in the config nothing is fetched and no key is needed. A reference needs a store
/// (the controller, or the proxy's own file) and a key file to open it with; with neither the
/// start stops with the message that says so.
pub async fn resolve_config(
    cfg: wayhouse_config::Config,
    store: Store<'_>,
    key_file: Option<&Path>,
) -> anyhow::Result<wayhouse_config::Config> {
    let names = cfg.secret_names();
    let resolver = if names.is_empty() || matches!(store, Store::None) {
        None
    } else {
        let keyring = load_keyring(key_file)?;
        let mut fetched = HashMap::new();
        if keyring.is_some() {
            let local = match &store {
                Store::File(path) => Some(crate::secret_store::read(path)?),
                _ => None,
            };
            for name in &names {
                let got = match (&store, &local) {
                    (Store::Controller(url, token), _) => fetch_sealed(url, *token, name)
                        .await
                        .map_err(|e| anyhow::anyhow!("{e}"))?,
                    (_, Some(map)) => map
                        .get(name)
                        .cloned()
                        .map_or(Fetched::Absent, Fetched::Sealed),
                    _ => Fetched::Absent,
                };
                fetched.insert(name.clone(), got);
            }
        }
        let r = open_all(&names, keyring.as_deref(), |n| {
            Ok(fetched.remove(n).unwrap_or(Fetched::Absent))
        })
        .map_err(|e| anyhow::anyhow!("{e}"))?;
        Some(r)
    };
    cfg.resolve_secrets(resolver.as_ref().map(|r| r as &dyn SecretResolver))
        .map_err(|e| anyhow::anyhow!("{e}"))
}

/// `GET {controller}/secrets/{name}/sealed` for each name; run before the config is resolved.
pub async fn fetch_sealed(
    base_url: &str,
    token: Option<&str>,
    name: &str,
) -> Result<Fetched, SecretError> {
    let unreadable = |why: String| SecretError::Unreadable {
        name: name.to_owned(),
        why,
    };
    wayhouse_http::policy::check_bearer_transport_global(
        "the controller URL",
        base_url,
        token.is_some(),
    )
    .map_err(unreadable)?;
    let mut req = wayhouse_http::client().get(format!("{base_url}/secrets/{name}/sealed"));
    if let Some(token) = token {
        req = req.bearer_auth(token);
    }
    let resp = req.send().await.map_err(|e| {
        unreadable(format!(
            "the controller could not be reached: {}",
            wayhouse_http::error_chain(&e)
        ))
    })?;
    match resp.status() {
        s if s.is_success() => resp
            .json::<Sealed>()
            .await
            .map(Fetched::Sealed)
            .map_err(|_| unreadable("the controller's answer was not understood".into())),
        reqwest::StatusCode::NOT_FOUND => Ok(Fetched::Absent),
        s => Err(unreadable(format!("the controller answered {s}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_bearer_token_is_never_sent_over_plain_http_beyond_loopback() {
        for url in ["http://controller:9901", "http://10.0.0.5:9901"] {
            let e = fetch_sealed(url, Some("tok"), "gossip-psk")
                .await
                .err()
                .unwrap();
            assert!(e.to_string().contains("https"), "{url}: {e}");
        }
    }

    const KEY_A: &str = "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=";
    const KEY_B: &str = "AgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgI=";
    const VALUE: &str = "correct-horse-battery-staple";

    fn ring(k: &str) -> Keyring {
        Keyring::parse(k).unwrap()
    }

    fn names(n: &[&str]) -> Vec<String> {
        n.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn a_secret_sealed_under_the_proxys_key_opens_and_resolves() {
        let sealed = ring(KEY_A)
            .seal_cluster("gossip-psk", VALUE.as_bytes())
            .unwrap();
        let r = open_all(&names(&["gossip-psk"]), Some(&ring(KEY_A)), |_| {
            Ok(Fetched::Sealed(sealed.clone()))
        })
        .unwrap();
        assert_eq!(r.resolve("gossip-psk").unwrap(), VALUE);
        assert_eq!(
            r.resolve("other").unwrap_err(),
            SecretError::Missing("other".into())
        );
    }

    #[test]
    fn nothing_to_open_needs_no_key_and_no_fetch() {
        let r = open_all(&[], None, |_| panic!("no fetch")).unwrap();
        assert!(r.0.is_empty());
    }

    #[test]
    fn no_key_file_stops_the_start_and_names_the_secret_and_the_flag() {
        let e = open_all(&names(&["admin-token"]), None, |_| {
            panic!("no fetch without a key")
        })
        .unwrap_err()
        .to_string();
        assert!(
            e.contains("`admin-token`") && e.contains("--secret-key-file"),
            "{e}"
        );
    }

    #[test]
    fn an_absent_secret_a_wrong_key_and_a_swapped_name_each_fail_with_a_reason() {
        let sealed = ring(KEY_A)
            .seal_cluster("gossip-psk", VALUE.as_bytes())
            .unwrap();
        let absent = open_all(&names(&["gossip-psk"]), Some(&ring(KEY_A)), |_| {
            Ok(Fetched::Absent)
        })
        .unwrap_err();
        assert_eq!(absent, SecretError::Missing("gossip-psk".into()));
        let wrong = open_all(&names(&["gossip-psk"]), Some(&ring(KEY_B)), |_| {
            Ok(Fetched::Sealed(sealed.clone()))
        })
        .unwrap_err()
        .to_string();
        assert!(
            wrong.contains("`gossip-psk`") && wrong.contains("key file"),
            "{wrong}"
        );
        // A ciphertext moved to another name does not open there.
        let swapped = open_all(&names(&["admin-token"]), Some(&ring(KEY_A)), |_| {
            Ok(Fetched::Sealed(sealed.clone()))
        })
        .unwrap_err()
        .to_string();
        assert!(swapped.contains("`admin-token`"), "{swapped}");
        assert!(!wrong.contains(VALUE) && !swapped.contains(VALUE));
    }

    #[test]
    fn a_transport_error_is_passed_on_unchanged() {
        let e = open_all(&names(&["x1"]), Some(&ring(KEY_A)), |n| {
            Err(SecretError::Unreadable {
                name: n.into(),
                why: "the controller could not be reached".into(),
            })
        })
        .unwrap_err();
        assert!(e.to_string().contains("could not be reached"));
    }

    /// A controller stub that serves what it was given and checks the bearer token.
    async fn stub(sealed: HashMap<String, Sealed>) -> String {
        use axum::extract::{Path, State};
        use axum::http::{HeaderMap, StatusCode};
        use axum::response::{IntoResponse, Response};
        async fn get(
            State(m): State<std::sync::Arc<HashMap<String, Sealed>>>,
            Path(name): Path<String>,
            headers: HeaderMap,
        ) -> Response {
            if headers.get("authorization").and_then(|v| v.to_str().ok())
                != Some("Bearer ctl-token")
            {
                return StatusCode::UNAUTHORIZED.into_response();
            }
            match m.get(&name) {
                Some(s) => axum::Json(s.clone()).into_response(),
                None => StatusCode::NOT_FOUND.into_response(),
            }
        }
        let app = axum::Router::new()
            .route("/secrets/{name}/sealed", axum::routing::get(get))
            .with_state(std::sync::Arc::new(sealed));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        url
    }

    fn sealed_cfg() -> wayhouse_config::Config {
        wayhouse_config::parse_str(
            "schema_version: 4\nsettings:\n  admin:\n    auth_token: { secret: admin-token }\n  failure_domain: eu\n  gossip:\n    bind: \"10.0.0.1:7946\"\n    psk: { secret: gossip-psk }\n\
             pools:\n  - name: p\n    targets: [\"127.0.0.1:1\"]\nlisteners:\n  - name: l\n    bind: \"0.0.0.0:7777\"\n    pool: p\n",
        )
        .unwrap()
    }

    fn key_file(contents: &str) -> tempfile::NamedTempFile {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(f, "{contents}").unwrap();
        std::fs::set_permissions(f.path(), std::fs::Permissions::from_mode(0o600)).unwrap();
        f
    }

    #[tokio::test]
    async fn the_config_is_resolved_from_a_controller_with_the_proxys_key_file() {
        let ring = ring(KEY_A);
        let url = stub(HashMap::from([
            (
                "admin-token".to_owned(),
                ring.seal_cluster("admin-token", b"the-admin-token-value")
                    .unwrap(),
            ),
            (
                "gossip-psk".to_owned(),
                ring.seal_cluster("gossip-psk", VALUE.as_bytes()).unwrap(),
            ),
        ]))
        .await;
        let keys = key_file(KEY_A);
        let cfg = resolve_config(
            sealed_cfg(),
            Store::Controller(&url, Some("ctl-token")),
            Some(keys.path()),
        )
        .await
        .unwrap();
        assert_eq!(
            cfg.admin_auth_token,
            Some(wayhouse_config::SecretOr::plain("the-admin-token-value"))
        );
        assert_eq!(
            cfg.gossip.unwrap().psk,
            wayhouse_config::SecretOr::plain(VALUE)
        );
    }

    #[tokio::test]
    async fn every_way_to_miss_stops_the_start_and_none_prints_a_value() {
        let ring = ring(KEY_A);
        let both = HashMap::from([
            (
                "admin-token".to_owned(),
                ring.seal_cluster("admin-token", b"the-admin-token-value")
                    .unwrap(),
            ),
            (
                "gossip-psk".to_owned(),
                ring.seal_cluster("gossip-psk", VALUE.as_bytes()).unwrap(),
            ),
        ]);
        let url = stub(both.clone()).await;
        let good = key_file(KEY_A);
        let other = key_file(KEY_B);
        let run = |url: String, token: &'static str, keys: Option<std::path::PathBuf>| async move {
            resolve_config(
                sealed_cfg(),
                Store::Controller(&url, Some(token)),
                keys.as_deref(),
            )
            .await
            .unwrap_err()
            .to_string()
        };
        // No key file: the flag is named.
        std::env::remove_var(wayhouse_secrets::KEY_ENV);
        assert!(run(url.clone(), "ctl-token", None)
            .await
            .contains("--secret-key-file"));
        // A key that cannot open them.
        let e = run(url.clone(), "ctl-token", Some(other.path().to_path_buf())).await;
        assert!(e.contains("key file"), "{e}");
        // Wrong controller token.
        assert!(run(url.clone(), "nope", Some(good.path().to_path_buf()))
            .await
            .contains("401"));
        // A secret the controller does not hold.
        let mut one = both;
        one.remove("gossip-psk");
        let url2 = stub(one).await;
        let e = run(url2, "ctl-token", Some(good.path().to_path_buf())).await;
        assert!(
            e.contains("`gossip-psk`") && e.contains("not in the secret store"),
            "{e}"
        );
        // No controller: a file-config proxy.
        let e = resolve_config(sealed_cfg(), Store::None, Some(good.path()))
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("no secret store"), "{e}");
        assert!(!e.contains(VALUE) && !e.contains("the-admin-token-value"));
    }

    #[tokio::test]
    async fn a_file_config_proxy_resolves_from_its_own_store() {
        let keys = key_file(KEY_A);
        let ring = ring(KEY_A);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secrets.json");
        crate::secret_store::set(&path, &ring, "admin-token", b"the-admin-token-value").unwrap();
        // One of the two is missing: named, and no value printed.
        let e = resolve_config(sealed_cfg(), Store::File(&path), Some(keys.path()))
            .await
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("`gossip-psk`") && !e.contains("the-admin-token-value"),
            "{e}"
        );
        crate::secret_store::set(&path, &ring, "gossip-psk", VALUE.as_bytes()).unwrap();
        let cfg = resolve_config(sealed_cfg(), Store::File(&path), Some(keys.path()))
            .await
            .unwrap();
        assert_eq!(
            cfg.admin_auth_token,
            Some(wayhouse_config::SecretOr::plain("the-admin-token-value"))
        );
    }
}
