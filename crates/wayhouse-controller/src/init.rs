//! `wayhouse-controller init`: everything a first controller needs, in one command.
//!
//! It generates the random tokens (admin, proxy, agent, aggregator, ingest), the UI password
//! and a private certificate authority with the leaf certificates the control-plane servers
//! present, so a bundle is **https from the first start** and every client can trust the CA
//! (`--ca-file`) or pin the leaf (`sha256:` fingerprint, `wayhouse-agent enroll --pin`). It then
//! writes them where the chosen `--target` reads them:
//!
//! - `systemd`: `<dir>/credentials/*` (the `LoadCredential=` sources of `deploy/systemd/`) and
//!   `<dir>/tls/*`;
//! - `compose`: an env file (`deploy/compose/.env` by default) and `<dir>/tls/*`; the sealed-secret
//!   keyring is made by the bundle's `keys` one-shot, as before;
//! - `k8s-secret`: Secret manifests (tokens, keyring, TLS) on stdout or `--output`, for
//!   `kubectl apply -f -`.
//!
//! Rules, the same as `deploy/solo/init.sh` had: nothing that exists is overwritten (the whole
//! plan is checked first, so a refusal changes nothing), files are created `0600` from the first
//! byte, and **no secret value reaches the terminal**: the summary names files and prints the
//! public controller-certificate fingerprint, nothing else. `k8s-secret` is the one target whose output is the
//! secrets, so it refuses to print to a terminal.

use std::collections::BTreeSet;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

use base64::Engine;
use clap::{Parser, ValueEnum};
use rand::RngCore;
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa,
    Issuer, KeyPair, KeyUsagePurpose,
};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

/// Bytes of randomness in a token (48 hex characters) and in the UI password (24).
const TOKEN_BYTES: usize = 24;
const PASSWORD_BYTES: usize = 12;
/// How long the CA and its leaves are valid. There is no renewal command yet; a leaf is replaced
/// by issuing a new one under the same CA (the CA key stays in the `tls` directory).
const CA_VALID_DAYS: i64 = 3650;
const LEAF_VALID_DAYS: i64 = 1825;

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum Target {
    /// Credential files for `LoadCredential=` plus the certificates (needs no Docker).
    Systemd,
    /// An env file for `docker compose` plus the certificates.
    Compose,
    /// Secret manifests on stdout (or `--output`).
    #[value(name = "k8s-secret")]
    K8sSecret,
}

#[derive(Parser, Debug)]
#[command(
    name = "wayhouse-controller init",
    about = "Generate the tokens, UI password and TLS certificates of a first controller (nothing secret is printed, nothing is overwritten)"
)]
pub struct InitArgs {
    /// Where the secrets go: `systemd` (credential files), `compose` (an env file) or
    /// `k8s-secret` (Secret manifests).
    #[arg(long, value_enum)]
    pub target: Target,

    /// Base directory. systemd: `/etc/wayhouse` (default), holding `credentials/` and `tls/`;
    /// compose: the current directory (default), holding `.env` and `tls/`.
    #[arg(long, value_name = "DIR")]
    pub dir: Option<PathBuf>,

    /// compose: the env file to write instead of `<dir>/.env`.
    #[arg(long, value_name = "FILE")]
    pub env_file: Option<PathBuf>,

    /// systemd: the credentials directory instead of `<dir>/credentials`.
    #[arg(long, value_name = "DIR")]
    pub credentials_dir: Option<PathBuf>,

    /// The certificate directory instead of `<dir>/tls` (systemd, compose).
    #[arg(long, value_name = "DIR")]
    pub tls_dir: Option<PathBuf>,

    /// An extra DNS name or IP the certificates must be valid for (repeatable): the name other
    /// hosts reach this controller by. `localhost`, `127.0.0.1`, `::1` and the target's own
    /// service names are always included.
    #[arg(long = "san", value_name = "NAME")]
    pub sans: Vec<String>,

    /// k8s-secret: the namespace of the Secrets, and of the service names in the certificates.
    #[arg(long, default_value = "wayhouse")]
    pub namespace: String,

    /// systemd, compose: the layout for `wayhouse-control` (controller, aggregator and UI in one
    /// process): one certificate, `control.pem`, valid for all three, instead of `controller.pem`
    /// (and, for compose, `aggregator.pem`).
    #[arg(long)]
    pub control: bool,

    /// k8s-secret: write the manifests to this file (mode 0600, never overwritten) instead of
    /// stdout.
    #[arg(long, value_name = "FILE")]
    pub output: Option<PathBuf>,
}

#[derive(Debug, thiserror::Error)]
pub enum InitError {
    #[error("{0}: already exists; not touching it. Delete it first to start over")]
    Exists(String),
    #[error("{what} {name:?} is not a valid DNS name or IP address")]
    BadName { what: &'static str, name: String },
    #[error(
        "refusing to print Secret manifests to a terminal (they hold every token and key): pipe them, as in \
         `wayhouse-controller init --target k8s-secret | kubectl apply -f -`, or use --output FILE"
    )]
    Terminal,
    #[error("{0} only applies to the {1} target")]
    WrongTarget(&'static str, &'static str),
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("generating a certificate: {0}")]
    Cert(#[from] rcgen::Error),
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> InitError + '_ {
    move |source| InitError::Io {
        path: path.to_owned(),
        source,
    }
}

// --- generated material -------------------------------------------------------------------------

fn random_hex(bytes: usize) -> Zeroizing<String> {
    let mut raw = Zeroizing::new(vec![0u8; bytes]);
    rand::rngs::OsRng.fill_bytes(&mut raw);
    Zeroizing::new(raw.iter().map(|b| format!("{b:02x}")).collect())
}

/// The shared secrets. Every token is 48 hex characters and the five controller-side ones are
/// pairwise different (`scoped_auth` refuses equal ones).
pub struct Secrets {
    pub controller_token: Zeroizing<String>,
    pub proxy_token: Zeroizing<String>,
    pub agent_token: Zeroizing<String>,
    pub aggregator_token: Zeroizing<String>,
    pub ingest_token: Zeroizing<String>,
    pub ui_password: Zeroizing<String>,
}

impl Secrets {
    pub fn generate() -> Self {
        Self {
            controller_token: random_hex(TOKEN_BYTES),
            proxy_token: random_hex(TOKEN_BYTES),
            agent_token: random_hex(TOKEN_BYTES),
            aggregator_token: random_hex(TOKEN_BYTES),
            ingest_token: random_hex(TOKEN_BYTES),
            ui_password: random_hex(PASSWORD_BYTES),
        }
    }
}

/// A leaf certificate and its key, PEM.
pub struct Leaf {
    pub cert_pem: String,
    pub key_pem: Zeroizing<String>,
    /// `sha256:<hex>` of the DER certificate: what `wayhouse-agent enroll --pin` takes.
    pub fingerprint: String,
}

/// The private CA and the leaves it signed.
pub struct Tls {
    pub ca_pem: String,
    pub ca_key_pem: Zeroizing<String>,
    pub controller: Leaf,
    pub aggregator: Leaf,
}

fn validity(params: &mut CertificateParams, days: i64) {
    let now = time::OffsetDateTime::now_utc();
    // A day of slack for a node whose clock runs behind.
    params.not_before = now - time::Duration::days(1);
    params.not_after = now + time::Duration::days(days);
}

fn name(cn: &str) -> DistinguishedName {
    let mut dn = DistinguishedName::new();
    dn.push(DnType::OrganizationName, "wayhouse");
    dn.push(DnType::CommonName, cn);
    dn
}

fn leaf(
    cn: &str,
    sans: &[String],
    ca: &Issuer<'_, &KeyPair>,
    what: &'static str,
) -> Result<Leaf, InitError> {
    for s in sans {
        if !valid_name(s) {
            return Err(InitError::BadName {
                what,
                name: s.clone(),
            });
        }
    }
    let mut params = CertificateParams::new(sans.to_vec())?;
    params.distinguished_name = name(cn);
    params.is_ca = IsCa::NoCa;
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    validity(&mut params, LEAF_VALID_DAYS);
    let key = KeyPair::generate()?;
    let cert = params.signed_by(&key, ca)?;
    Ok(Leaf {
        cert_pem: cert.pem(),
        key_pem: Zeroizing::new(key.serialize_pem()),
        fingerprint: format!("sha256:{}", hex(&Sha256::digest(cert.der().as_ref()))),
    })
}

/// An IP literal or a DNS name (labels of letters, digits and `-`; a leading `*.` wildcard).
/// rcgen itself would accept any ASCII, spaces included.
fn valid_name(s: &str) -> bool {
    if s.parse::<std::net::IpAddr>().is_ok() {
        return true;
    }
    let host = s.strip_prefix("*.").unwrap_or(s);
    !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|label| {
            (1..=63).contains(&label.len())
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

impl Tls {
    /// A fresh CA, and one leaf for the controller and one for the aggregator (each valid for
    /// its own names plus `localhost`, `127.0.0.1` and `::1`).
    pub fn generate(
        controller_names: &[String],
        aggregator_names: &[String],
    ) -> Result<Self, InitError> {
        let ca_key = KeyPair::generate()?;
        let mut ca_params = CertificateParams::new(Vec::<String>::new())?;
        ca_params.distinguished_name = name("wayhouse CA");
        // Path length 0: this CA signs leaves, never another CA.
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
        ca_params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        validity(&mut ca_params, CA_VALID_DAYS);
        let ca_cert = ca_params.self_signed(&ca_key)?;
        let issuer = Issuer::from_params(&ca_params, &ca_key);
        Ok(Self {
            ca_pem: ca_cert.pem(),
            ca_key_pem: Zeroizing::new(ca_key.serialize_pem()),
            controller: leaf("wayhouse-controller", controller_names, &issuer, "--san")?,
            aggregator: leaf("wayhouse-aggregator", aggregator_names, &issuer, "--san")?,
        })
    }
}

/// `localhost`, the loopback addresses and `own`, then the operator's `--san`s, without repeats.
fn names(own: &[String], extra: &[String]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    ["localhost", "127.0.0.1", "::1"]
        .into_iter()
        .map(str::to_owned)
        .chain(own.iter().cloned())
        .chain(extra.iter().map(|s| s.trim().to_owned()))
        .filter(|s| !s.is_empty() && seen.insert(s.to_ascii_lowercase()))
        .collect()
}

fn k8s_service_names(service: &str, namespace: &str) -> Vec<String> {
    let mut names = vec![
        service.to_owned(),
        format!("{service}.{namespace}"),
        format!("{service}.{namespace}.svc"),
        format!("{service}.{namespace}.svc.cluster.local"),
    ];
    // The StatefulSet pods of the HA overlay (`<pod>.<service>-peers`): one wildcard label each.
    for suffix in ["", ".{ns}", ".{ns}.svc", ".{ns}.svc.cluster.local"] {
        names.push(format!(
            "*.{service}-peers{}",
            suffix.replace("{ns}", namespace)
        ));
    }
    names
}

// --- files --------------------------------------------------------------------------------------

struct Planned {
    path: PathBuf,
    mode: u32,
    data: Zeroizing<Vec<u8>>,
}

fn plan(path: PathBuf, mode: u32, data: impl AsRef<[u8]>) -> Planned {
    Planned {
        path,
        mode,
        data: Zeroizing::new(data.as_ref().to_vec()),
    }
}

/// Writes every planned file, creating a missing parent directory. Nothing is written when any
/// path already exists; a failure part-way removes what this call created.
fn write_all(files: &[Planned], dir_modes: &[(&Path, u32)]) -> Result<(), InitError> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    for f in files {
        if std::fs::symlink_metadata(&f.path).is_ok() {
            return Err(InitError::Exists(f.path.display().to_string()));
        }
    }
    for (dir, mode) in dir_modes {
        // Only a directory this run makes gets the mode: an existing `/etc/wayhouse` stays as is.
        if !dir.exists() {
            std::fs::create_dir_all(dir).map_err(io(dir))?;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(*mode))
                .map_err(io(dir))?;
        }
    }
    let mut written: Vec<&Path> = Vec::new();
    for f in files {
        let result = (|| {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(f.mode)
                .open(&f.path)
                .map_err(io(&f.path))?;
            written.push(&f.path);
            file.write_all(&f.data).map_err(io(&f.path))
        })();
        if let Err(e) = result {
            for p in written {
                let _ = std::fs::remove_file(p);
            }
            return Err(e);
        }
    }
    Ok(())
}

/// One process serves all three listeners with one certificate: `controller.*` becomes `control.*`.
fn rename_to_control(files: &mut [Planned]) {
    for f in files {
        if let Some(n) = f.path.file_name().and_then(|n| n.to_str()) {
            if let Some(rest) = n.strip_prefix("controller.") {
                f.path.set_file_name(format!("control.{rest}"));
            }
        }
    }
}

fn tls_files(dir: &Path, tls: &Tls, with_aggregator: bool) -> Vec<Planned> {
    let mut files = vec![
        plan(dir.join("ca.pem"), 0o644, &tls.ca_pem),
        // Kept so another replica's or node's leaf can be issued later; it is never mounted.
        plan(dir.join("ca.key"), 0o600, tls.ca_key_pem.as_bytes()),
        plan(dir.join("controller.pem"), 0o644, &tls.controller.cert_pem),
        plan(
            dir.join("controller.key"),
            0o600,
            tls.controller.key_pem.as_bytes(),
        ),
    ];
    if with_aggregator {
        files.push(plan(
            dir.join("aggregator.pem"),
            0o644,
            &tls.aggregator.cert_pem,
        ));
        files.push(plan(
            dir.join("aggregator.key"),
            0o600,
            tls.aggregator.key_pem.as_bytes(),
        ));
    }
    files
}

fn summary_tls(out: &mut dyn Write, dir: &Path, tls: &Tls) -> std::io::Result<()> {
    writeln!(
        out,
        "wrote the CA and the controller certificate under {} (the CA key ca.key never leaves it).",
        dir.display()
    )?;
    writeln!(
        out,
        "controller certificate fingerprint (public; `wayhouse-agent enroll --pin`): {}",
        tls.controller.fingerprint
    )
}

// --- targets ------------------------------------------------------------------------------------

/// Runs the command. `out` receives the Secret manifests (`k8s-secret` only); `summary` gets the
/// human-readable result, which never contains a secret value.
pub fn run(
    args: &InitArgs,
    out: &mut dyn Write,
    out_is_terminal: bool,
    summary: &mut dyn Write,
) -> Result<(), InitError> {
    if args.target != Target::K8sSecret && args.output.is_some() {
        return Err(InitError::WrongTarget("--output", "k8s-secret"));
    }
    if args.target == Target::K8sSecret && args.control {
        return Err(InitError::WrongTarget("--control", "systemd or compose"));
    }
    if args.target != Target::Compose && args.env_file.is_some() {
        return Err(InitError::WrongTarget("--env-file", "compose"));
    }
    if args.target != Target::Systemd && args.credentials_dir.is_some() {
        return Err(InitError::WrongTarget("--credentials-dir", "systemd"));
    }
    match args.target {
        Target::Systemd => systemd(args, summary),
        Target::Compose => compose(args, summary),
        Target::K8sSecret => k8s_secret(args, out, out_is_terminal, summary),
    }
}

fn systemd(args: &InitArgs, summary: &mut dyn Write) -> Result<(), InitError> {
    let base = args
        .dir
        .clone()
        .unwrap_or_else(|| PathBuf::from("/etc/wayhouse"));
    let creds = args
        .credentials_dir
        .clone()
        .unwrap_or_else(|| base.join("credentials"));
    let tls_dir = args.tls_dir.clone().unwrap_or_else(|| base.join("tls"));
    let s = Secrets::generate();
    // The split solo units dial the controller on loopback, so only its certificate is used and the
    // aggregator's loopback listener stays plain http (the guard exempts loopback). With
    // `--control` the one certificate serves the controller and the aggregator.
    let tls = Tls::generate(&names(&[], &args.sans), &names(&[], &args.sans))?;
    let mut files = vec![
        plan(
            creds.join("controller-token"),
            0o600,
            s.controller_token.as_bytes(),
        ),
        plan(creds.join("proxy-token"), 0o600, s.proxy_token.as_bytes()),
        plan(creds.join("agent-token"), 0o600, s.agent_token.as_bytes()),
        plan(
            creds.join("aggregator-token"),
            0o600,
            s.aggregator_token.as_bytes(),
        ),
        plan(creds.join("ingest-token"), 0o600, s.ingest_token.as_bytes()),
        plan(creds.join("ui-password"), 0o600, s.ui_password.as_bytes()),
    ];
    let mut tls_planned = tls_files(&tls_dir, &tls, false);
    if args.control {
        rename_to_control(&mut tls_planned);
    }
    files.extend(tls_planned);
    write_all(&files, &[(&creds, 0o700), (&tls_dir, 0o755)])?;
    let io_err = |e| InitError::Io {
        path: base.clone(),
        source: e,
    };
    writeln!(
        summary,
        "wrote {}/{{controller-token,proxy-token,agent-token,aggregator-token,ingest-token,ui-password}} (mode 0600). The values stay out of your terminal.",
        creds.display()
    )
    .map_err(io_err)?;
    summary_tls(summary, &tls_dir, &tls).map_err(io_err)?;
    let (units, target) = if args.control {
        (
            "wayhouse-control.service and wayhouse-solo-control.target, plus \
             wayhouse.service with wayhouse.service.d/control.conf (the proxy dials the aggregator over https \
             and must not pull in the split units, which would fight wayhouse-control for 9901/9902)",
            "wayhouse-solo-control.target",
        )
    } else {
        (
            "deploy/systemd/*.service and wayhouse-solo.target",
            "wayhouse-solo.target",
        )
    };
    writeln!(
        summary,
        "Next: install {units} into /etc/systemd/system, \
         `systemctl daemon-reload`, `systemctl enable --now {target}`.\n\
         The UI is at http://127.0.0.1:9903; its password is in {}/ui-password.",
        creds.display()
    )
    .map_err(io_err)
}

fn compose(args: &InitArgs, summary: &mut dyn Write) -> Result<(), InitError> {
    let base = args.dir.clone().unwrap_or_else(|| PathBuf::from("."));
    let env_file = args.env_file.clone().unwrap_or_else(|| base.join(".env"));
    let tls_dir = args.tls_dir.clone().unwrap_or_else(|| base.join("tls"));
    // The env file records where the certificates are; compose resolves relative paths against
    // the compose file, not against this env file, so record an absolute one.
    let abs_tls = std::path::absolute(&tls_dir).map_err(io(&tls_dir))?;
    let s = Secrets::generate();
    // `--control`: the one certificate serves the `controller` service name (the seed step dials it);
    // the aggregator is dialled on loopback, which the certificate covers, so its own leaf is not written.
    let tls = Tls::generate(
        &names(&["controller".to_owned()], &args.sans),
        &names(&["aggregator".to_owned()], &args.sans),
    )?;
    let env = Zeroizing::new(format!(
        "WAYHOUSE_CONTROLLER_TOKEN={}\nWAYHOUSE_PROXY_TOKEN={}\nWAYHOUSE_AGENT_TOKEN={}\n\
         WAYHOUSE_AGGREGATOR_TOKEN={}\nWAYHOUSE_INGEST_TOKEN={}\nWAYHOUSE_UI_PASSWORD={}\n\
         WAYHOUSE_TLS_DIR={}\n",
        *s.controller_token,
        *s.proxy_token,
        *s.agent_token,
        *s.aggregator_token,
        *s.ingest_token,
        *s.ui_password,
        abs_tls.display()
    ));
    let mut files = vec![plan(env_file.clone(), 0o600, env.as_bytes())];
    let mut tls_planned = tls_files(&tls_dir, &tls, !args.control);
    if args.control {
        rename_to_control(&mut tls_planned);
    }
    files.extend(tls_planned);
    let env_dir = env_file
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_owned);
    let mut dirs: Vec<(&Path, u32)> = vec![(&tls_dir, 0o755)];
    if let Some(d) = &env_dir {
        dirs.insert(0, (d, 0o755));
    }
    write_all(&files, &dirs)?;
    let io_err = |e| InitError::Io {
        path: env_file.clone(),
        source: e,
    };
    writeln!(
        summary,
        "wrote {} (mode 0600). The UI password is WAYHOUSE_UI_PASSWORD in it; the tokens stay out of your terminal and your shell history.",
        env_file.display()
    )
    .map_err(io_err)?;
    summary_tls(summary, &tls_dir, &tls).map_err(io_err)?;
    writeln!(
        summary,
        "Next: docker compose --env-file {} -f docker-compose.yml -f compose.solo.yml{} up -d --build\n\
         The UI is at {}://127.0.0.1:9903.{}",
        env_file.display(),
        if args.control { " -f compose.solo-control.yml" } else { "" },
        if args.control { "https" } else { "http" },
        if args.control { " It serves the init certificate, which a browser does not know: accept the warning (the CA has no name constraints, so do not add ca.pem to a trust store, and keep ca.key out of backups)." } else { "" }
    )
    .map_err(io_err)
}

fn b64(data: impl AsRef<[u8]>) -> Zeroizing<String> {
    Zeroizing::new(base64::engine::general_purpose::STANDARD.encode(data))
}

fn secret_manifest(name: &str, namespace: &str, data: &[(&str, &[u8])]) -> Zeroizing<String> {
    let mut y = format!(
        "apiVersion: v1\nkind: Secret\nmetadata:\n  name: {name}\n  namespace: {namespace}\ntype: Opaque\ndata:\n"
    );
    for (key, value) in data {
        y.push_str(&format!("  {key}: {}\n", *b64(value)));
    }
    Zeroizing::new(y)
}

fn k8s_secret(
    args: &InitArgs,
    out: &mut dyn Write,
    out_is_terminal: bool,
    summary: &mut dyn Write,
) -> Result<(), InitError> {
    if args.output.is_none() && out_is_terminal {
        return Err(InitError::Terminal);
    }
    let ns = args.namespace.as_str();
    let s = Secrets::generate();
    // The example Secret's other two keys: the proxy's admin token (the aggregator presents it as
    // its --instance-token) and the controller replicas' peer token.
    let admin_token = random_hex(TOKEN_BYTES);
    let ha_token = random_hex(TOKEN_BYTES);
    let tls = Tls::generate(
        &names(&k8s_service_names("wayhouse-controller", ns), &args.sans),
        &names(&k8s_service_names("wayhouse-aggregator", ns), &args.sans),
    )?;
    let mut keyring = Zeroizing::new([0u8; 32]);
    rand::rngs::OsRng.fill_bytes(&mut keyring[..]);
    let keyring_line = Zeroizing::new(format!("{}\n", *b64(&keyring[..])));

    let docs = [
        secret_manifest(
            "wayhouse-tokens",
            ns,
            &[
                ("controller-token", s.controller_token.as_bytes()),
                ("proxy-token", s.proxy_token.as_bytes()),
                ("agent-token", s.agent_token.as_bytes()),
                ("aggregator-token", s.aggregator_token.as_bytes()),
                ("ingest-token", s.ingest_token.as_bytes()),
                ("admin-token", admin_token.as_bytes()),
                ("ha-token", ha_token.as_bytes()),
                ("ui-password", s.ui_password.as_bytes()),
            ],
        ),
        secret_manifest(
            "wayhouse-keyring",
            ns,
            &[("secret.keys", keyring_line.as_bytes())],
        ),
        // The CA key is not here: a Secret is no place for the key that can mint more
        // certificates. Replicas share the leaf (their service names are in it).
        secret_manifest(
            "wayhouse-tls",
            ns,
            &[
                ("ca.crt", tls.ca_pem.as_bytes()),
                ("controller.crt", tls.controller.cert_pem.as_bytes()),
                ("controller.key", tls.controller.key_pem.as_bytes()),
                ("aggregator.crt", tls.aggregator.cert_pem.as_bytes()),
                ("aggregator.key", tls.aggregator.key_pem.as_bytes()),
            ],
        ),
    ];
    let manifests = Zeroizing::new(
        docs.iter()
            .map(|d| d.as_str())
            .collect::<Vec<_>>()
            .join("---\n"),
    );
    match &args.output {
        Some(path) => write_all(&[plan(path.clone(), 0o600, manifests.as_bytes())], &[])?,
        None => out
            .write_all(manifests.as_bytes())
            .and_then(|()| out.flush())
            .map_err(io(Path::new("stdout")))?,
    }
    let io_err = |e| InitError::Io {
        path: PathBuf::from("stderr"),
        source: e,
    };
    writeln!(
        summary,
        "generated the Secrets wayhouse-tokens, wayhouse-keyring and wayhouse-tls for namespace {ns}.\n\
         The CA key was not kept (it is not in any Secret). controller certificate fingerprint \
         (public; `wayhouse-agent enroll --pin`): {}\n\
         admin-token must equal the proxy's admin auth_token (wayhouse-config).",
        tls.controller.fingerprint
    )
    .map_err(io_err)
}

/// The `init` command line: `args` start after the program name and the `init` word.
pub fn main(args: impl IntoIterator<Item = std::ffi::OsString>) -> anyhow::Result<()> {
    let parsed = InitArgs::parse_from(
        std::iter::once(std::ffi::OsString::from("wayhouse-controller init")).chain(args),
    );
    let stdout = std::io::stdout();
    let is_terminal = stdout.is_terminal();
    let mut lock = stdout.lock();
    // `k8s-secret` writes the manifests to stdout, so its summary goes to stderr.
    let result = if parsed.target == Target::K8sSecret {
        run(&parsed, &mut lock, is_terminal, &mut std::io::stderr())
    } else {
        let mut sink = Vec::new();
        run(&parsed, &mut sink, false, &mut lock)
    };
    result.map_err(anyhow::Error::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn args(target: Target, dir: &Path) -> InitArgs {
        InitArgs {
            target,
            dir: Some(dir.to_owned()),
            env_file: None,
            credentials_dir: None,
            tls_dir: None,
            sans: vec![],
            namespace: "wayhouse".into(),
            output: None,
            control: false,
        }
    }

    fn mode(p: &Path) -> u32 {
        std::fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    /// Every file under `dir` with its contents, to prove a refused run changed nothing.
    fn snapshot(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        let mut all = Vec::new();
        let mut stack = vec![dir.to_owned()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(d).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    all.push((p.clone(), std::fs::read(&p).unwrap()));
                }
            }
        }
        all.sort();
        all
    }

    fn run_to_strings(a: &InitArgs) -> Result<(String, String), InitError> {
        let (mut out, mut summary) = (Vec::new(), Vec::new());
        run(a, &mut out, false, &mut summary)?;
        Ok((
            String::from_utf8(out).unwrap(),
            String::from_utf8(summary).unwrap(),
        ))
    }

    /// The tokens, password and every PEM body in `dir`: what must never reach a terminal.
    fn secret_values(dir: &Path) -> Vec<String> {
        let mut v = Vec::new();
        for (p, data) in snapshot(dir) {
            let text = String::from_utf8(data).unwrap();
            if p.ends_with("ca.pem")
                || p.to_string_lossy().ends_with(".pem")
                || p.to_string_lossy().ends_with(".key")
            {
                v.extend(
                    text.lines()
                        .filter(|l| !l.starts_with("-----") && !l.is_empty())
                        .map(str::to_owned),
                );
            } else {
                v.extend(
                    text.lines()
                        .filter(|l| !l.starts_with("WAYHOUSE_TLS_DIR"))
                        .flat_map(|l| {
                            l.rsplit('=')
                                .next()
                                .map(str::to_owned)
                                .filter(|x| x.len() >= 16)
                        }),
                );
            }
        }
        assert!(!v.is_empty());
        v
    }

    #[test]
    fn generated_tokens_pass_the_startup_policy_and_differ() {
        let s = Secrets::generate();
        let all = [
            &s.controller_token,
            &s.proxy_token,
            &s.agent_token,
            &s.aggregator_token,
            &s.ingest_token,
            &s.ui_password,
        ];
        for t in all {
            wayhouse_http::policy::check_secret("token", t).unwrap();
        }
        let unique: BTreeSet<&str> = all.iter().map(|t| t.as_str()).collect();
        assert_eq!(unique.len(), all.len(), "two generated secrets are equal");
        // And the controller accepts the three scoped ones together.
        crate::scoped_auth::check(
            Some(&s.controller_token),
            Some(&s.proxy_token),
            Some(&s.agent_token),
        )
        .unwrap();
    }

    #[test]
    fn systemd_writes_private_credentials_and_certificates_and_prints_no_value() {
        let dir = tempfile::tempdir().unwrap();
        let (out, summary) = run_to_strings(&args(Target::Systemd, dir.path())).unwrap();
        assert!(out.is_empty());
        let creds = dir.path().join("credentials");
        for f in [
            "controller-token",
            "proxy-token",
            "agent-token",
            "aggregator-token",
            "ingest-token",
            "ui-password",
        ] {
            assert_eq!(mode(&creds.join(f)), 0o600, "{f}");
        }
        assert_eq!(mode(&creds), 0o700);
        let tls = dir.path().join("tls");
        assert_eq!(mode(&tls.join("controller.key")), 0o600);
        assert_eq!(mode(&tls.join("ca.key")), 0o600);
        assert_eq!(mode(&tls.join("ca.pem")), 0o644);
        assert_eq!(mode(&tls.join("controller.pem")), 0o644);
        assert!(!tls.join("aggregator.pem").exists());
        assert!(summary.contains("sha256:") && summary.contains("systemctl"));
        for v in secret_values(dir.path()) {
            assert!(!summary.contains(&v), "the summary shows a secret value");
        }
    }

    #[test]
    fn control_systemd_writes_one_certificate_for_the_one_process() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = args(Target::Systemd, dir.path());
        a.control = true;
        let (_, summary) = run_to_strings(&a).unwrap();
        let tls = dir.path().join("tls");
        assert_eq!(mode(&tls.join("control.key")), 0o600);
        assert_eq!(mode(&tls.join("control.pem")), 0o644);
        assert!(!tls.join("controller.pem").exists());
        assert!(!tls.join("aggregator.pem").exists());
        assert!(summary.contains("wayhouse-control.service"), "{summary}");
        // The same six credentials as the split layout: the unit renames them per program.
        assert!(dir.path().join("credentials/ui-password").exists());
    }

    #[test]
    fn control_is_not_a_k8s_option() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = args(Target::K8sSecret, dir.path());
        a.control = true;
        assert!(matches!(
            run_to_strings(&a).unwrap_err(),
            InitError::WrongTarget("--control", _)
        ));
    }

    #[test]
    fn control_compose_writes_one_certificate_and_keeps_the_env_file_name() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = args(Target::Compose, dir.path());
        a.control = true;
        let (_, summary) = run_to_strings(&a).unwrap();
        let tls = dir.path().join("tls");
        assert!(tls.join("control.pem").is_file());
        assert_eq!(mode(&tls.join("control.key")), 0o600);
        for gone in [
            "controller.pem",
            "controller.key",
            "aggregator.pem",
            "aggregator.key",
        ] {
            assert!(!tls.join(gone).exists(), "{gone}");
        }
        assert!(summary.contains("compose.solo-control.yml"));
        assert!(summary.contains("https://127.0.0.1:9903"));
        // The env file keeps the name the operator gave it, even if it starts with `controller.`.
        let other = tempfile::tempdir().unwrap();
        let mut b = args(Target::Compose, other.path());
        b.control = true;
        b.env_file = Some(other.path().join("controller.env"));
        let (_, summary) = run_to_strings(&b).unwrap();
        assert!(other.path().join("controller.env").is_file());
        assert!(summary.contains("controller.env"));
    }

    #[test]
    fn compose_writes_the_env_file_the_bundle_reads() {
        let dir = tempfile::tempdir().unwrap();
        let (_, summary) = run_to_strings(&args(Target::Compose, dir.path())).unwrap();
        let env_path = dir.path().join(".env");
        assert_eq!(mode(&env_path), 0o600);
        let env = std::fs::read_to_string(&env_path).unwrap();
        for var in [
            "WAYHOUSE_CONTROLLER_TOKEN",
            "WAYHOUSE_PROXY_TOKEN",
            "WAYHOUSE_AGENT_TOKEN",
            "WAYHOUSE_AGGREGATOR_TOKEN",
            "WAYHOUSE_INGEST_TOKEN",
            "WAYHOUSE_UI_PASSWORD",
        ] {
            let line = env
                .lines()
                .find(|l| l.starts_with(&format!("{var}=")))
                .unwrap();
            assert!(line.len() > var.len() + 16, "{var} is too short");
        }
        let tls_line = env
            .lines()
            .find_map(|l| l.strip_prefix("WAYHOUSE_TLS_DIR="))
            .unwrap();
        assert!(Path::new(tls_line).is_absolute());
        assert!(Path::new(tls_line).join("aggregator.key").is_file());
        assert!(summary.contains("compose.solo.yml"));
        for v in secret_values(dir.path()) {
            assert!(!summary.contains(&v));
        }
    }

    #[test]
    fn a_second_run_refuses_and_changes_nothing() {
        for target in [Target::Systemd, Target::Compose] {
            let dir = tempfile::tempdir().unwrap();
            run_to_strings(&args(target, dir.path())).unwrap();
            let before = snapshot(dir.path());
            let e = run_to_strings(&args(target, dir.path())).unwrap_err();
            assert!(matches!(e, InitError::Exists(_)), "{e}");
            assert_eq!(snapshot(dir.path()), before, "{target:?} changed a file");
        }
    }

    #[test]
    fn one_existing_file_stops_the_whole_run_before_anything_is_written() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("tls")).unwrap();
        std::fs::write(dir.path().join("tls/ca.pem"), "mine").unwrap();
        let e = run_to_strings(&args(Target::Compose, dir.path())).unwrap_err();
        assert!(matches!(e, InitError::Exists(_)));
        assert!(!dir.path().join(".env").exists());
        assert_eq!(
            std::fs::read(dir.path().join("tls/ca.pem")).unwrap(),
            b"mine"
        );
    }

    #[test]
    fn k8s_secret_emits_three_secrets_without_the_ca_key() {
        let a = args(Target::K8sSecret, Path::new("."));
        let (out, summary) = run_to_strings(&a).unwrap();
        let docs: Vec<serde_json::Value> = out
            .split("---\n")
            .map(|d| serde_norway::from_str(d).unwrap())
            .collect();
        let names: Vec<&str> = docs
            .iter()
            .map(|d| d["metadata"]["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            ["wayhouse-tokens", "wayhouse-keyring", "wayhouse-tls"]
        );
        let get = |doc: usize, key: &str| -> Vec<u8> {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD
                .decode(docs[doc]["data"][key].as_str().unwrap())
                .unwrap()
        };
        // The keys the example Secret defines, so the base manifests can mount it.
        for key in [
            "controller-token",
            "proxy-token",
            "agent-token",
            "aggregator-token",
            "ingest-token",
            "admin-token",
            "ha-token",
            "ui-password",
        ] {
            assert!(get(0, key).len() >= 16, "{key}");
        }
        wayhouse_secrets::Keyring::parse(std::str::from_utf8(&get(1, "secret.keys")).unwrap())
            .unwrap();
        assert!(String::from_utf8(get(2, "controller.key"))
            .unwrap()
            .contains("PRIVATE KEY"));
        assert!(docs[2]["data"].get("ca.key").is_none());
        assert!(
            !out.contains("PRIVATE KEY"),
            "keys are base64 inside the manifest"
        );
        assert!(summary.contains("sha256:") && !summary.contains("PRIVATE"));
        // The service names are in the controller certificate.
        let pem = String::from_utf8(get(2, "controller.crt")).unwrap();
        assert!(pem.contains("BEGIN CERTIFICATE"));
    }

    #[test]
    fn k8s_secret_refuses_a_terminal_unless_it_writes_a_file() {
        let a = args(Target::K8sSecret, Path::new("."));
        let (mut out, mut summary) = (Vec::new(), Vec::new());
        let e = run(&a, &mut out, true, &mut summary).unwrap_err();
        assert!(matches!(e, InitError::Terminal));
        assert!(out.is_empty());

        let dir = tempfile::tempdir().unwrap();
        let mut a = args(Target::K8sSecret, Path::new("."));
        a.output = Some(dir.path().join("secrets.yaml"));
        run(&a, &mut out, true, &mut summary).unwrap();
        assert!(out.is_empty());
        assert_eq!(mode(&dir.path().join("secrets.yaml")), 0o600);
        let again = run(&a, &mut out, true, &mut summary).unwrap_err();
        assert!(matches!(again, InitError::Exists(_)));
    }

    #[test]
    fn options_that_belong_to_another_target_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = args(Target::Systemd, dir.path());
        a.env_file = Some(dir.path().join("x.env"));
        assert!(matches!(
            run_to_strings(&a).unwrap_err(),
            InitError::WrongTarget("--env-file", _)
        ));
        assert!(snapshot(dir.path()).is_empty());
    }

    #[test]
    fn a_bad_san_is_refused_before_any_file_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = args(Target::Systemd, dir.path());
        a.sans = vec!["not a name".into()];
        let e = run_to_strings(&a).unwrap_err();
        assert!(matches!(e, InitError::BadName { .. }), "{e}");
        assert!(snapshot(dir.path()).is_empty());
    }

    /// The contract that matters: a client that trusts only `ca.pem` accepts the controller
    /// certificate for the names the bundles use, and for no other name, and a leaf of another
    /// run's CA is refused.
    #[test]
    fn the_certificates_verify_against_their_ca_for_their_names_only() {
        use rustls::client::WebPkiServerVerifier;
        use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
        use rustls::RootCertStore;

        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let tls = Tls::generate(
            &names(&["controller".into()], &["ctl.example.org".into()]),
            &names(&["aggregator".into()], &[]),
        )
        .unwrap();
        let der = |pem: &str| -> CertificateDer<'static> {
            use rustls::pki_types::pem::PemObject;
            CertificateDer::from_pem_slice(pem.as_bytes()).unwrap()
        };
        let verifier_for = |ca: &str| {
            let mut roots = RootCertStore::empty();
            roots.add(der(ca)).unwrap();
            WebPkiServerVerifier::builder(roots.into()).build().unwrap()
        };
        let check = |ca: &str, leaf: &str, host: &str| {
            use rustls::client::danger::ServerCertVerifier;
            verifier_for(ca)
                .verify_server_cert(
                    &der(leaf),
                    &[],
                    &ServerName::try_from(host.to_owned()).unwrap(),
                    &[],
                    UnixTime::now(),
                )
                .map(|_| ())
        };
        for host in [
            "controller",
            "localhost",
            "127.0.0.1",
            "::1",
            "ctl.example.org",
        ] {
            check(&tls.ca_pem, &tls.controller.cert_pem, host)
                .unwrap_or_else(|e| panic!("{host}: {e}"));
        }
        assert!(check(&tls.ca_pem, &tls.controller.cert_pem, "evil.example").is_err());
        assert!(
            check(&tls.ca_pem, &tls.controller.cert_pem, "aggregator").is_err(),
            "the controller leaf must not be valid for the aggregator's name"
        );
        check(&tls.ca_pem, &tls.aggregator.cert_pem, "aggregator").unwrap();
        let other = Tls::generate(&names(&[], &[]), &names(&[], &[])).unwrap();
        assert!(check(&other.ca_pem, &tls.controller.cert_pem, "controller").is_err());
        // The CA signs leaves only.
        assert!(tls.controller.fingerprint.starts_with("sha256:"));
        assert_eq!(tls.controller.fingerprint.len(), "sha256:".len() + 64);
    }
}
