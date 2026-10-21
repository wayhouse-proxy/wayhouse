#![allow(clippy::needless_pass_by_value, clippy::result_large_err)]

use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::Router;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine;
use instant_acme::HttpClient;
use rcgen::{
    BasicConstraints, CertificateParams, CertificateSigningRequestParams, IsCa, Issuer, KeyPair,
    SerialNumber,
};
use ring::signature::{UnparsedPublicKey, ECDSA_P256_SHA256_FIXED};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::dns::MemDns;

/// What the next challenge or order should do wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailKind {
    /// The next challenge is declared invalid (`urn:ietf:params:acme:error:dns`).
    Dns,
    /// The next `newOrder` answers `429 rateLimited`.
    RateLimited,
}

type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

struct AuthzRec {
    order: usize,
    host: String,
    wildcard: bool,
    token: String,
    status: &'static str,
}

struct OrderRec {
    ids: Vec<String>,
    authzs: Vec<usize>,
    status: &'static str,
    cert: Option<String>,
}

struct Account {
    thumbprint: String,
    key: Vec<u8>,
}

struct State {
    nonces: HashSet<String>,
    accounts: Vec<Account>,
    orders: Vec<OrderRec>,
    authzs: Vec<AuthzRec>,
    fail: Option<FailKind>,
    /// Sent as `Retry-After` (seconds) on a `rateLimited` problem.
    retry_after: Option<u64>,
    issued: usize,
}

struct Inner {
    base: String,
    dns: MemDns,
    clock: Clock,
    validity_secs: u64,
    ca: Issuer<'static, KeyPair>,
    ca_pem: String,
    state: Mutex<State>,
    /// When set, a new account must carry a valid external account binding for this
    /// `(key id, raw HMAC key)`.
    eab: Mutex<Option<(String, Vec<u8>)>>,
}

/// An in-process ACME server on a loopback port. Dropping it stops the server.
pub struct FakeCa {
    inner: Arc<Inner>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for FakeCa {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl FakeCa {
    /// Starts a CA whose certificates are valid for 90 days from the real clock.
    pub async fn start(dns: MemDns) -> Self {
        let clock: Clock = Arc::new(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs())
        });
        Self::start_with(dns, clock, 90 * 86_400).await
    }

    /// `clock` stamps certificates (tests share it with the code under test).
    pub async fn start_with(dns: MemDns, clock: Clock, validity_secs: u64) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        let key = KeyPair::generate().unwrap();
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, "wayhouse fake CA");
        let ca_pem = params.self_signed(&key).unwrap().pem();
        let inner = Arc::new(Inner {
            base: format!("http://{addr}"),
            dns,
            clock,
            validity_secs,
            ca: Issuer::new(params, key),
            ca_pem,
            state: Mutex::new(State {
                nonces: HashSet::new(),
                accounts: vec![],
                orders: vec![],
                authzs: vec![],
                fail: None,
                retry_after: None,
                issued: 0,
            }),
            eab: Mutex::new(None),
        });
        let svc = inner.clone();
        let app = Router::new().fallback(move |m: Method, u: Uri, b: Bytes| {
            let svc = svc.clone();
            async move { svc.handle(&m, u.path(), &b) }
        });
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Self { inner, task }
    }

    pub fn directory_url(&self) -> String {
        format!("{}/dir", self.inner.base)
    }

    /// Makes the next matching request fail.
    pub fn fail_next(&self, kind: FailKind) {
        self.inner.state.lock().unwrap().fail = Some(kind);
    }

    /// Makes the rate-limit answers carry `Retry-After: <secs>`.
    pub fn retry_after(&self, secs: u64) {
        self.inner.state.lock().unwrap().retry_after = Some(secs);
    }

    /// How many certificates were issued.
    pub fn require_eab(&self, kid: &str, hmac_key_b64url: &str) {
        let raw = B64.decode(hmac_key_b64url).expect("a base64url key");
        *self.inner.eab.lock().unwrap() = Some((kid.to_string(), raw));
    }

    pub fn issued(&self) -> usize {
        self.inner.state.lock().unwrap().issued
    }

    /// How many accounts were created.
    pub fn accounts(&self) -> usize {
        self.inner.state.lock().unwrap().accounts.len()
    }

    /// An HTTP client factory for [`crate::AcmeCa::with_http`] (plain HTTP to this server).
    pub fn http_factory() -> Box<dyn Fn() -> Box<dyn HttpClient> + Send + Sync> {
        Box::new(|| {
            let c =
                hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
                    .build(hyper_util::client::legacy::connect::HttpConnector::new());
            Box::new(c)
        })
    }
}

/// Checks the `externalAccountBinding` of a new-account payload: an HS256 JWS over the
/// account's own public key, naming `kid`.
fn verify_eab(
    base: &str,
    payload: &Value,
    kid: &str,
    key: &[u8],
    account_key: &[u8],
) -> Result<(), &'static str> {
    let eab = payload
        .get("externalAccountBinding")
        .ok_or("an external account binding is required")?;
    let field = |k: &str| eab[k].as_str().unwrap_or_default();
    let (protected, body, sig) = (field("protected"), field("payload"), field("signature"));
    let hdr: Value = B64
        .decode(protected)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .ok_or("bad binding header")?;
    if hdr["alg"] != "HS256" || hdr["kid"].as_str() != Some(kid) {
        return Err("unknown external account key id");
    }
    if hdr["url"].as_str() != Some(&format!("{base}/acct")) {
        return Err("the binding names another url");
    }
    let sig = B64.decode(sig).map_err(|_| "bad binding signature")?;
    let mac = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, key);
    ring::hmac::verify(&mac, format!("{protected}.{body}").as_bytes(), &sig)
        .map_err(|_| "the binding signature does not verify")?;
    let jwk: Value = B64
        .decode(body)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .ok_or("bad binding payload")?;
    let mut expect = vec![4u8];
    expect.extend(
        B64.decode(jwk["x"].as_str().unwrap_or_default())
            .unwrap_or_default(),
    );
    expect.extend(
        B64.decode(jwk["y"].as_str().unwrap_or_default())
            .unwrap_or_default(),
    );
    if expect != account_key {
        return Err("the binding is for another account key");
    }
    Ok(())
}

fn rand_token() -> String {
    use rand::RngCore;
    let mut b = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut b);
    B64.encode(b)
}

impl Inner {
    fn reply(
        &self,
        st: &mut State,
        status: StatusCode,
        loc: Option<String>,
        body: Value,
    ) -> Response {
        let nonce = rand_token();
        st.nonces.insert(nonce.clone());
        let mut h = HeaderMap::new();
        h.insert("replay-nonce", HeaderValue::from_str(&nonce).unwrap());
        if let Some(l) = loc {
            h.insert(header::LOCATION, HeaderValue::from_str(&l).unwrap());
        }
        h.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        (status, h, body.to_string()).into_response()
    }

    fn problem(&self, st: &mut State, status: StatusCode, kind: &str, detail: &str) -> Response {
        let retry_after = st.retry_after.filter(|_| kind == "rateLimited");
        let mut rsp = self.reply(
            st,
            status,
            None,
            json!({"type": format!("urn:ietf:params:acme:error:{kind}"), "detail": detail}),
        );
        if let Some(secs) = retry_after {
            rsp.headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from(secs));
        }
        rsp
    }

    fn handle(&self, method: &Method, path: &str, body: &Bytes) -> Response {
        let mut st = self.state.lock().unwrap();
        match (method.as_str(), path) {
            ("GET", "/dir") => {
                let b = &self.base;
                let d = json!({
                    "newNonce": format!("{b}/nonce"),
                    "newAccount": format!("{b}/acct"),
                    "newOrder": format!("{b}/order"),
                    "meta": {"termsOfService": format!("{b}/tos")},
                });
                self.reply(&mut st, StatusCode::OK, None, d)
            }
            ("HEAD" | "GET", "/nonce") => {
                let nonce = rand_token();
                st.nonces.insert(nonce.clone());
                let mut h = HeaderMap::new();
                h.insert("replay-nonce", HeaderValue::from_str(&nonce).unwrap());
                (StatusCode::OK, h).into_response()
            }
            ("POST", p) => self.post(&mut st, p, body),
            _ => self.problem(&mut st, StatusCode::NOT_FOUND, "malformed", "no such route"),
        }
    }

    /// Parses and verifies a JWS; returns `(payload, account index)`.
    fn jws(
        &self,
        st: &mut State,
        path: &str,
        body: &[u8],
    ) -> Result<(Value, usize, bool), Response> {
        let bad =
            |s: &mut State, m: &str| Err(self.problem(s, StatusCode::BAD_REQUEST, "malformed", m));
        let Ok(env) = serde_json::from_slice::<Value>(body) else {
            return bad(st, "not json");
        };
        let field = |k: &str| env[k].as_str().unwrap_or_default().to_string();
        let (protected, payload, sig) = (field("protected"), field("payload"), field("signature"));
        let Ok(hdr_bytes) = B64.decode(&protected) else {
            return bad(st, "bad protected");
        };
        let hdr: Value = serde_json::from_slice(&hdr_bytes).unwrap_or(Value::Null);
        let nonce = hdr["nonce"].as_str().unwrap_or_default();
        if !st.nonces.remove(nonce) {
            return Err(self.problem(st, StatusCode::BAD_REQUEST, "badNonce", "bad nonce"));
        }
        if hdr["url"].as_str() != Some(&format!("{}{path}", self.base)) {
            return bad(st, "url mismatch");
        }
        let (pubkey, acct_idx) = if let Some(jwk) = hdr.get("jwk") {
            let (x, y) = (
                B64.decode(jwk["x"].as_str().unwrap_or_default())
                    .unwrap_or_default(),
                B64.decode(jwk["y"].as_str().unwrap_or_default())
                    .unwrap_or_default(),
            );
            let mut k = vec![4u8];
            k.extend(x);
            k.extend(y);
            (k, None::<usize>)
        } else {
            let kid = hdr["kid"].as_str().unwrap_or_default();
            let Some(i) = kid
                .strip_prefix(&format!("{}/acct/", self.base))
                .and_then(|n| n.parse::<usize>().ok())
                .filter(|i| *i < st.accounts.len())
            else {
                return Err(self.problem(
                    st,
                    StatusCode::UNAUTHORIZED,
                    "accountDoesNotExist",
                    "no such account",
                ));
            };
            (st.accounts[i].key.clone(), Some(i))
        };
        let signed = format!("{protected}.{payload}");
        let sig = B64.decode(sig).unwrap_or_default();
        if UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, &pubkey)
            .verify(signed.as_bytes(), &sig)
            .is_err()
        {
            return Err(self.problem(
                st,
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "bad signature",
            ));
        }
        let payload = if payload.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&B64.decode(&payload).unwrap_or_default()).unwrap_or(Value::Null)
        };
        if acct_idx.is_none() {
            // New account: remember its key and thumbprint.
            let jwk = &hdr["jwk"];
            let canon = format!(
                r#"{{"crv":"{}","kty":"{}","x":"{}","y":"{}"}}"#,
                jwk["crv"].as_str().unwrap_or_default(),
                jwk["kty"].as_str().unwrap_or_default(),
                jwk["x"].as_str().unwrap_or_default(),
                jwk["y"].as_str().unwrap_or_default()
            );
            let thumbprint = B64.encode(Sha256::digest(canon.as_bytes()));
            st.accounts.push(Account {
                thumbprint,
                key: pubkey,
            });
            return Ok((payload, st.accounts.len() - 1, true));
        }
        Ok((payload, acct_idx.unwrap_or(0), false))
    }

    fn order_json(&self, st: &State, i: usize) -> Value {
        let o = &st.orders[i];
        let b = &self.base;
        let mut v = json!({
            "status": o.status,
            "identifiers": o.ids.iter().map(|n| json!({"type":"dns","value":n})).collect::<Vec<_>>(),
            "authorizations": o.authzs.iter().map(|a| format!("{b}/authz/{a}")).collect::<Vec<_>>(),
            "finalize": format!("{b}/finalize/{i}"),
        });
        if o.cert.is_some() && o.status == "valid" {
            v["certificate"] = json!(format!("{b}/cert/{i}"));
        }
        v
    }

    fn authz_json(&self, st: &State, a: usize) -> Value {
        let z = &st.authzs[a];
        let mut ch = json!({
            "type": "dns-01",
            "url": format!("{}/chall/{a}", self.base),
            "status": z.status,
            "token": z.token,
        });
        if z.status == "invalid" {
            ch["error"] =
                json!({"type": "urn:ietf:params:acme:error:dns", "detail": "no TXT record found"});
        }
        let mut v = json!({
            "status": z.status,
            "identifier": {"type": "dns", "value": z.host},
            "challenges": [ch],
        });
        if z.wildcard {
            v["wildcard"] = json!(true);
        }
        v
    }

    fn refresh_order(&self, st: &mut State, i: usize) {
        let statuses: Vec<&str> = st.orders[i]
            .authzs
            .iter()
            .map(|a| st.authzs[*a].status)
            .collect();
        if st.orders[i].status == "pending" {
            if statuses.contains(&"invalid") {
                st.orders[i].status = "invalid";
            } else if statuses.iter().all(|s| *s == "valid") {
                st.orders[i].status = "ready";
            }
        }
    }

    fn post(&self, st: &mut State, path: &str, body: &[u8]) -> Response {
        let (payload, acct, is_new) = match self.jws(st, path, body) {
            Ok(v) => v,
            Err(r) => return r,
        };
        let b = self.base.clone();
        let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
        let idx = parts.get(1).and_then(|s| s.parse::<usize>().ok());
        match (parts[0], idx) {
            ("acct", None) if is_new => {
                let i = acct;
                if let Some((kid, key)) = self.eab.lock().unwrap().clone() {
                    if let Err(why) =
                        verify_eab(&self.base, &payload, &kid, &key, &st.accounts[i].key)
                    {
                        st.accounts.pop();
                        return self.problem(
                            st,
                            StatusCode::UNAUTHORIZED,
                            "externalAccountRequired",
                            why,
                        );
                    }
                }
                if payload["termsOfServiceAgreed"] != json!(true) {
                    st.accounts.pop();
                    return self.problem(
                        st,
                        StatusCode::FORBIDDEN,
                        "userActionRequired",
                        "terms not agreed",
                    );
                }
                self.reply(
                    st,
                    StatusCode::CREATED,
                    Some(format!("{b}/acct/{i}")),
                    json!({"status":"valid"}),
                )
            }
            ("order", None) => {
                if st.fail == Some(FailKind::RateLimited) {
                    st.fail = None;
                    return self.problem(
                        st,
                        StatusCode::TOO_MANY_REQUESTS,
                        "rateLimited",
                        "too many certificates",
                    );
                }
                let ids: Vec<String> = payload["identifiers"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|i| i["value"].as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                let oi = st.orders.len();
                let mut authzs = vec![];
                for n in &ids {
                    let (host, wildcard) = n
                        .strip_prefix("*.")
                        .map_or((n.as_str(), false), |h| (h, true));
                    authzs.push(st.authzs.len());
                    st.authzs.push(AuthzRec {
                        order: oi,
                        host: host.to_string(),
                        wildcard,
                        token: rand_token(),
                        status: "pending",
                    });
                }
                st.orders.push(OrderRec {
                    ids,
                    authzs,
                    status: "pending",
                    cert: None,
                });
                let v = self.order_json(st, oi);
                self.reply(st, StatusCode::CREATED, Some(format!("{b}/order/{oi}")), v)
            }
            ("order", Some(i)) if i < st.orders.len() => {
                self.refresh_order(st, i);
                let v = self.order_json(st, i);
                self.reply(st, StatusCode::OK, None, v)
            }
            ("authz", Some(a)) if a < st.authzs.len() => {
                let v = self.authz_json(st, a);
                self.reply(st, StatusCode::OK, None, v)
            }
            ("chall", Some(a)) if a < st.authzs.len() => {
                let thumb = st.accounts[acct].thumbprint.clone();
                let z = &st.authzs[a];
                let expected =
                    B64.encode(Sha256::digest(format!("{}.{thumb}", z.token).as_bytes()));
                let present = self
                    .dns
                    .has(&format!("_acme-challenge.{}", z.host), &expected);
                let forced = st.fail == Some(FailKind::Dns);
                if forced {
                    st.fail = None;
                }
                let ok = present && !forced;
                st.authzs[a].status = if ok { "valid" } else { "invalid" };
                let order = st.authzs[a].order;
                self.refresh_order(st, order);
                let v = self.authz_json(st, a);
                self.reply(st, StatusCode::OK, None, v["challenges"][0].clone())
            }
            ("finalize", Some(i)) if i < st.orders.len() => {
                if st.orders[i].status != "ready" {
                    return self.problem(
                        st,
                        StatusCode::FORBIDDEN,
                        "orderNotReady",
                        "order is not ready",
                    );
                }
                let der = B64
                    .decode(payload["csr"].as_str().unwrap_or_default())
                    .unwrap_or_default();
                let der = rustls_pki_types::CertificateSigningRequestDer::from(der);
                let Ok(mut csr) = CertificateSigningRequestParams::from_der(&der) else {
                    return self.problem(st, StatusCode::BAD_REQUEST, "badCSR", "unparsable CSR");
                };
                let now = (self.clock)();
                csr.params.not_before =
                    time::OffsetDateTime::from_unix_timestamp(now as i64).unwrap();
                csr.params.not_after =
                    time::OffsetDateTime::from_unix_timestamp((now + self.validity_secs) as i64)
                        .unwrap();
                st.issued += 1;
                csr.params.serial_number = Some(SerialNumber::from(st.issued as u64 + 1000));
                let cert = csr.signed_by(&self.ca).unwrap();
                st.orders[i].cert = Some(format!("{}{}", cert.pem(), self.ca_pem));
                st.orders[i].status = "valid";
                let v = self.order_json(st, i);
                self.reply(st, StatusCode::OK, None, v)
            }
            ("cert", Some(i)) if i < st.orders.len() && st.orders[i].cert.is_some() => {
                let pem = st.orders[i].cert.clone().unwrap();
                let nonce = rand_token();
                st.nonces.insert(nonce.clone());
                let mut h = HeaderMap::new();
                h.insert("replay-nonce", HeaderValue::from_str(&nonce).unwrap());
                h.insert(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("application/pem-certificate-chain"),
                );
                (StatusCode::OK, h, pem).into_response()
            }
            _ => self.problem(st, StatusCode::NOT_FOUND, "malformed", "no such resource"),
        }
    }
}
