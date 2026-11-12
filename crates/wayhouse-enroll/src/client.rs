//! The redeem client. TLS is required except to a loopback host, and the server is pinned by a
//! SHA-256 of its leaf certificate or by a CA file, so the code is never sent to an unverified
//! peer. `http://` to a non-loopback host is refused before any connection.

use std::path::PathBuf;
use std::sync::Arc;

use tokio_rustls::rustls::client::danger::{HandshakeSignatureValid, ServerCertVerifier, ServerCertVerified};
use tokio_rustls::rustls::crypto::{ring, verify_tls12_signature, verify_tls13_signature, CryptoProvider};
use tokio_rustls::rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use tokio_rustls::rustls::{ClientConfig, DigitallySignedStruct, Error as TlsError, SignatureScheme};

use crate::{Code, RedeemRequest, RedeemResponse, Role};

#[derive(Debug, Clone)]
pub enum Pin {
    /// SHA-256 of the server's leaf certificate (DER).
    Fingerprint([u8; 32]),
    /// A PEM file of the CA(s) to trust instead of the built-in roots.
    CaFile(PathBuf),
}

#[derive(Debug, thiserror::Error)]
pub enum EnrollError {
    #[error("the controller URL must be https:// (http:// is only accepted for a loopback host)")]
    PlainHttp,
    #[error("not a valid controller URL")]
    BadUrl,
    #[error("not a SHA-256 fingerprint: expected 64 hex digits, optionally colon-separated")]
    BadFingerprint,
    #[error("--controller-ca: {0}")]
    Ca(String),
    #[error("could not reach the controller: {0}")]
    Connect(String),
    #[error("the controller refused the code (wrong, expired, already used or revoked)")]
    Refused,
    #[error("the controller answered {0}")]
    Status(u16),
    #[error("the controller sent an unusable answer")]
    BadAnswer,
}

/// Parses `sha256:ab:cd..`, `AB:CD..` or bare hex.
pub fn parse_fingerprint(s: &str) -> Result<[u8; 32], EnrollError> {
    let s = s.trim();
    let s = s.strip_prefix("sha256:").unwrap_or(s);
    let hex: String = s.chars().filter(|c| *c != ':').collect();
    if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(EnrollError::BadFingerprint);
    }
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).map_err(|_| EnrollError::BadFingerprint)?;
    }
    Ok(out)
}

fn is_loopback(host: &str) -> bool {
    let h = host.trim_start_matches('[').trim_end_matches(']');
    h.eq_ignore_ascii_case("localhost") || h.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// The URL's host when it may carry a code: `https`, or `http` to loopback.
fn check_url(url: &str) -> Result<(), EnrollError> {
    let u = reqwest::Url::parse(url).map_err(|_| EnrollError::BadUrl)?;
    match (u.scheme(), u.host_str()) {
        ("https", Some(_)) => Ok(()),
        ("http", Some(h)) if is_loopback(h) => Ok(()),
        ("http", Some(_)) => Err(EnrollError::PlainHttp),
        _ => Err(EnrollError::BadUrl),
    }
}

/// Accepts exactly one certificate, whatever its names or chain; handshake signatures are still
/// verified, so the peer must hold the key of the pinned certificate.
#[derive(Debug)]
struct PinVerifier {
    pin: [u8; 32],
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for PinVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        use sha2::{Digest, Sha256};
        let got: [u8; 32] = Sha256::digest(end_entity.as_ref()).into();
        // Public data, so a plain compare is fine.
        if got == self.pin {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(TlsError::General("the controller's certificate does not match the pinned fingerprint".into()))
        }
    }

    fn verify_tls12_signature(&self, m: &[u8], c: &CertificateDer<'_>, d: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, TlsError> {
        verify_tls12_signature(m, c, d, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(&self, m: &[u8], c: &CertificateDer<'_>, d: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, TlsError> {
        verify_tls13_signature(m, c, d, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

fn http_client(pin: &Pin) -> Result<reqwest::Client, EnrollError> {
    let b = match pin {
        Pin::Fingerprint(fp) => {
            let provider = Arc::new(ring::default_provider());
            let cfg = ClientConfig::builder_with_provider(provider.clone())
                .with_safe_default_protocol_versions()
                .map_err(|e| EnrollError::Connect(e.to_string()))?
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(PinVerifier { pin: *fp, provider }))
                .with_no_client_auth();
            reqwest::Client::builder().use_preconfigured_tls(cfg)
        }
        Pin::CaFile(path) => {
            let certs = wayhouse_http::load_ca_file(path).map_err(|e| EnrollError::Ca(e.to_string()))?;
            // Only this CA: a pinned controller must not also be reachable via a public root.
            certs
                .into_iter()
                .fold(reqwest::Client::builder().tls_built_in_root_certs(false), reqwest::ClientBuilder::add_root_certificate)
        }
    };
    b.timeout(std::time::Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| EnrollError::Connect(e.to_string()))
}

/// Redeems `code` as `role`. Nothing is sent unless the URL and pin are acceptable; a failed
/// pin check aborts the handshake before the request body (the code) leaves this process.
pub async fn redeem(controller_url: &str, pin: &Pin, code: &Code, role: Role) -> Result<RedeemResponse, EnrollError> {
    check_url(controller_url)?;
    let client = http_client(pin)?;
    let body = RedeemRequest { code: code.expose().to_string(), role };
    let resp = client
        .post(format!("{}/enroll", controller_url.trim_end_matches('/')))
        .json(&body)
        .send()
        .await
        .map_err(|e| EnrollError::Connect(wayhouse_http::error_chain(&e)))?;
    match resp.status().as_u16() {
        200 => resp.json().await.map_err(|_| EnrollError::BadAnswer),
        401 | 403 | 404 | 410 | 429 => Err(EnrollError::Refused),
        s => Err(EnrollError::Status(s)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_http_is_refused_except_to_loopback() {
        for ok in ["https://c.example", "http://127.0.0.1:9901", "http://localhost", "http://[::1]:1"] {
            assert!(check_url(ok).is_ok(), "{ok}");
        }
        for bad in ["http://c.example", "http://10.0.0.5:9901", "ftp://x", "nonsense"] {
            assert!(check_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn fingerprints_parse_in_the_usual_spellings() {
        let hex = "ab".repeat(32);
        let colons = vec!["AB"; 32].join(":");
        assert_eq!(parse_fingerprint(&hex).unwrap(), [0xab; 32]);
        assert_eq!(parse_fingerprint(&format!("sha256:{colons}")).unwrap(), [0xab; 32]);
        assert!(parse_fingerprint("abcd").is_err());
    }
}
