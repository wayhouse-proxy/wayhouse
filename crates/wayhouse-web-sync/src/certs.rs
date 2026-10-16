//! Operator-supplied certificates, read from disk into the document. Inline PEM (not
//! `load_files`) on purpose: Caddy does not watch the files and treats an identical
//! `POST /load` as a no-op, so only a changed document reloads a renewed certificate
//! (spike #355, item 5).

use std::collections::BTreeMap;

use wayhouse_config::WebView;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pem {
    pub certificate: String,
    pub key: String,
}

/// Certificates by site index.
pub type SiteCerts = BTreeMap<usize, Pem>;

#[derive(Debug, thiserror::Error)]
pub enum CertError {
    #[error("site {site}: cannot read {path}: {source}")]
    Read {
        site: usize,
        path: String,
        source: std::io::Error,
    },
    #[error("site {site}: {path} holds no PEM {what}")]
    NotPem {
        site: usize,
        path: String,
        what: &'static str,
    },
}

/// Reads every site's `tls` files. One unreadable or non-PEM file fails the lot: a
/// document missing a site's certificate would take that site down on load.
pub async fn load(web: &WebView) -> Result<SiteCerts, CertError> {
    let mut out = SiteCerts::new();
    for (i, s) in web.sites.iter().enumerate() {
        let Some(t) = &s.tls else { continue };
        let certificate = read(i, &t.cert, "-----BEGIN CERTIFICATE-----", "certificate").await?;
        let key = read(i, &t.key, "-----BEGIN ", "private key").await?;
        out.insert(i, Pem { certificate, key });
    }
    Ok(out)
}

async fn read(
    site: usize,
    path: &str,
    marker: &str,
    what: &'static str,
) -> Result<String, CertError> {
    let text = tokio::fs::read_to_string(path)
        .await
        .map_err(|source| CertError::Read {
            site,
            path: path.to_string(),
            source,
        })?;
    if !text.contains(marker) {
        return Err(CertError::NotPem {
            site,
            path: path.to_string(),
            what,
        });
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wayhouse_config::{WebSiteTlsView, WebSiteView};

    fn view(cert: &str, key: &str) -> WebView {
        WebView {
            pool: "web".into(),
            listen_http: "127.0.0.1:8080".into(),
            listen_https: "127.0.0.1:8443".into(),
            trusted_proxy: vec![],
            sites: vec![
                WebSiteView {
                    hosts: vec!["plain.example.com".into()],
                    upstreams: vec![],
                    tls: None,
                    upstream_tls: None,
                    health_path: None,
                },
                WebSiteView {
                    hosts: vec!["a.example.com".into()],
                    upstreams: vec![],
                    tls: Some(WebSiteTlsView {
                        cert: cert.into(),
                        key: key.into(),
                    }),
                    upstream_tls: None,
                    health_path: None,
                },
            ],
        }
    }

    #[tokio::test]
    async fn reads_inline_pem_by_site_index() {
        let d = tempfile::tempdir().unwrap();
        let (c, k) = (d.path().join("c.pem"), d.path().join("k.pem"));
        std::fs::write(
            &c,
            "-----BEGIN CERTIFICATE-----\nAAA\n-----END CERTIFICATE-----\n",
        )
        .unwrap();
        std::fs::write(
            &k,
            "-----BEGIN PRIVATE KEY-----\nBBB\n-----END PRIVATE KEY-----\n",
        )
        .unwrap();
        let got = load(&view(c.to_str().unwrap(), k.to_str().unwrap()))
            .await
            .unwrap();
        assert_eq!(got.len(), 1);
        assert!(got[&1].certificate.contains("AAA") && got[&1].key.contains("BBB"));
    }

    #[tokio::test]
    async fn a_missing_or_non_pem_file_fails_the_whole_load() {
        let d = tempfile::tempdir().unwrap();
        let junk = d.path().join("junk");
        std::fs::write(&junk, "hello").unwrap();
        let junk = junk.to_str().unwrap();
        let e = load(&view("/nonexistent/c.pem", junk)).await.unwrap_err();
        assert!(matches!(e, CertError::Read { site: 1, .. }), "{e}");
        let e = load(&view(junk, junk)).await.unwrap_err();
        assert!(
            matches!(
                e,
                CertError::NotPem {
                    what: "certificate",
                    ..
                }
            ),
            "{e}"
        );
    }
}
