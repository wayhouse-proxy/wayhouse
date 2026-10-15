//! ACME state in the controller's `sled` database. A standalone controller applies ops
//! through [`AcmeStore::apply_local`]; under HA the state machine is the only writer and uses
//! [`AcmeStore::apply_at`], which is crash-idempotent per Raft index (the entry and the cursor
//! commit in one transaction).

use std::sync::{Arc, Mutex};

use sled::transaction::{ConflictableTransactionError, TransactionError};
use sled::Transactional;

use super::types::{
    AcmeOp, AcmeSnapshot, Applied, Attempt, Site, StoredAccount, StoredCert,
};

const APPLIED_KEY: &[u8] = b"applied";
const ACCOUNT_KEY: &[u8] = b"account";
/// Certificate versions kept per site: the current one and the one before it.
const KEEP_VERSIONS: usize = 2;

#[derive(Debug, thiserror::Error)]
pub enum AcmeStoreError {
    #[error("acme store: {0}")]
    Sled(#[from] sled::Error),
    #[error("acme store record is corrupt: {0}")]
    Corrupt(#[from] serde_json::Error),
}

#[derive(Clone)]
pub struct AcmeStore {
    sites: sled::Tree,
    account: sled::Tree,
    /// `<site>\0<version, big-endian>` to a [`StoredCert`].
    certs: sled::Tree,
    attempts: sled::Tree,
    meta: sled::Tree,
    write: Arc<Mutex<()>>,
    changed: Arc<tokio::sync::watch::Sender<u64>>,
}

fn cert_key(site: &str, version: u64) -> Vec<u8> {
    let mut k = cert_prefix(site);
    k.extend_from_slice(&version.to_be_bytes());
    k
}

fn cert_prefix(site: &str) -> Vec<u8> {
    let mut k = site.as_bytes().to_vec();
    k.push(0);
    k
}

fn read<T: serde::de::DeserializeOwned>(v: Option<sled::IVec>) -> Result<Option<T>, AcmeStoreError> {
    v.map(|b| serde_json::from_slice(&b)).transpose().map_err(Into::into)
}

impl AcmeStore {
    pub fn open(db: &sled::Db) -> Result<Self, sled::Error> {
        Ok(Self {
            sites: db.open_tree("acme_sites")?,
            account: db.open_tree("acme_account")?,
            certs: db.open_tree("acme_certs")?,
            attempts: db.open_tree("acme_attempts")?,
            meta: db.open_tree("acme_meta")?,
            write: Arc::default(),
            changed: Arc::new(tokio::sync::watch::channel(0).0),
        })
    }

    /// A receiver that wakes after every write.
    pub fn watch(&self) -> tokio::sync::watch::Receiver<u64> {
        self.changed.subscribe()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ()> {
        self.write
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn applied_index(&self) -> Result<Option<u64>, AcmeStoreError> {
        Ok(self
            .meta
            .get(APPLIED_KEY)?
            .and_then(|v| <[u8; 8]>::try_from(v.as_ref()).ok())
            .map(u64::from_be_bytes))
    }

    // ---- reads ----

    pub fn sites(&self) -> Result<Vec<Site>, AcmeStoreError> {
        self.sites
            .iter()
            .values()
            .map(|v| Ok(serde_json::from_slice(&v?)?))
            .collect()
    }

    pub fn site(&self, name: &str) -> Result<Option<Site>, AcmeStoreError> {
        read(self.sites.get(name.as_bytes())?)
    }

    pub fn account(&self) -> Result<Option<StoredAccount>, AcmeStoreError> {
        read(self.account.get(ACCOUNT_KEY)?)
    }

    /// The newest certificate of `site`.
    pub fn cert(&self, site: &str) -> Result<Option<StoredCert>, AcmeStoreError> {
        read(self.certs.scan_prefix(cert_prefix(site)).values().next_back().transpose()?)
    }

    /// Every kept version of `site`, oldest first.
    pub fn certs(&self, site: &str) -> Result<Vec<StoredCert>, AcmeStoreError> {
        self.certs
            .scan_prefix(cert_prefix(site))
            .values()
            .map(|v| Ok(serde_json::from_slice(&v?)?))
            .collect()
    }

    pub fn attempt(&self, site: &str) -> Result<Option<Attempt>, AcmeStoreError> {
        read(self.attempts.get(site.as_bytes())?)
    }

    // ---- writes ----

    /// Applies `op` on a standalone controller: the next local index, and the entry term is
    /// the one the op was tagged with (there is no other leader).
    pub fn apply_local(&self, op: &AcmeOp) -> Result<Applied, AcmeStoreError> {
        let next = self.applied_index()?.map_or(1, |i| i + 1);
        let term = match op {
            AcmeOp::CertIssued { tagged_term, .. } => *tagged_term,
            _ => 0,
        };
        self.apply_at(next, term, op)
    }

    /// Applies `op` at Raft `index`, appended in `entry_term`. An entry at or below the
    /// cursor is a replay and changes nothing. Rejections advance the cursor too: they are
    /// the same answer on every replica.
    pub fn apply_at(
        &self,
        index: u64,
        entry_term: u64,
        op: &AcmeOp,
    ) -> Result<Applied, AcmeStoreError> {
        let _guard = self.lock();
        // What a transaction cannot scan, read first; the lock keeps it from changing.
        let other_sites: Vec<Site> = match op {
            AcmeOp::SiteSet(site) => self
                .sites()?
                .into_iter()
                .filter(|s| s.name != site.name)
                .collect(),
            _ => Vec::new(),
        };
        let wipe_certs: Vec<sled::IVec> = match op {
            AcmeOp::SiteDelete { name } => self
                .certs
                .scan_prefix(cert_prefix(name))
                .keys()
                .collect::<Result<_, _>>()?,
            _ => Vec::new(),
        };
        let newest: Option<u64> = match op {
            AcmeOp::CertIssued { site, .. } => self.cert(site)?.map(|c| c.version),
            _ => None,
        };
        let old_versions: Vec<sled::IVec> = match op {
            AcmeOp::CertIssued { site, .. } => self
                .certs
                .scan_prefix(cert_prefix(site))
                .keys()
                .collect::<Result<_, _>>()?,
            _ => Vec::new(),
        };
        let abort = ConflictableTransactionError::Abort;
        let outcome = (
            &self.sites,
            &self.account,
            &self.certs,
            &self.attempts,
            &self.meta,
        )
            .transaction(|(sites, account, certs, attempts, meta)| {
                let covered = meta
                    .get(APPLIED_KEY)?
                    .and_then(|v| <[u8; 8]>::try_from(v.as_ref()).ok())
                    .map(u64::from_be_bytes);
                if covered.is_some_and(|c| c >= index) {
                    return Ok(Applied::Replayed);
                }
                meta.insert(APPLIED_KEY, &index.to_be_bytes())?;
                let json = |v: &dyn erased::Ser| v.to_json().map_err(|e| abort(AcmeStoreError::from(e)));
                let get_site = |name: &str| -> Result<Option<Site>, ConflictableTransactionError<AcmeStoreError>> {
                    match sites.get(name.as_bytes())? {
                        Some(v) => Ok(Some(serde_json::from_slice(&v).map_err(|e| abort(e.into()))?)),
                        None => Ok(None),
                    }
                };
                match op {
                    AcmeOp::SiteSet(site) => {
                        if let Err(why) = site.validate() {
                            return Ok(Applied::Invalid(why));
                        }
                        for other in &other_sites {
                            if let Some(n) = site.names.iter().find(|n| other.names.contains(n)) {
                                return Ok(Applied::NameTaken(n.clone()));
                            }
                        }
                        sites.insert(site.name.as_bytes(), json(site)?)?;
                        Ok(Applied::Done)
                    }
                    AcmeOp::SiteDelete { name } => {
                        if get_site(name)?.is_none() {
                            return Ok(Applied::NoSuchSite);
                        }
                        sites.remove(name.as_bytes())?;
                        attempts.remove(name.as_bytes())?;
                        for k in &wipe_certs {
                            certs.remove(k.clone())?;
                        }
                        Ok(Applied::Done)
                    }
                    AcmeOp::AccountSet(acct) => {
                        if account.get(ACCOUNT_KEY)?.is_some() {
                            return Ok(Applied::Exists);
                        }
                        account.insert(ACCOUNT_KEY, json(acct)?)?;
                        Ok(Applied::Done)
                    }
                    AcmeOp::CertIssued { site, tagged_term, cert } => {
                        let Some(current) = get_site(site)? else {
                            return Ok(Applied::NoSuchSite);
                        };
                        if *tagged_term != entry_term {
                            return Ok(Applied::WrongTerm);
                        }
                        if cert.version != newest.unwrap_or(0) + 1 || cert.names != current.names {
                            return Ok(Applied::Stale);
                        }
                        certs.insert(cert_key(site, cert.version), json(cert)?)?;
                        // Keep the newest KEEP_VERSIONS; `old_versions` is oldest first.
                        let drop = (old_versions.len() + 1).saturating_sub(KEEP_VERSIONS);
                        for k in old_versions.iter().take(drop) {
                            certs.remove(k.clone())?;
                        }
                        attempts.remove(site.as_bytes())?;
                        Ok(Applied::Done)
                    }
                    AcmeOp::Attempt { site, attempt } => {
                        if get_site(site)?.is_none() {
                            return Ok(Applied::NoSuchSite);
                        }
                        attempts.insert(site.as_bytes(), json(attempt)?)?;
                        Ok(Applied::Done)
                    }
                }
            })
            .map_err(|e| match e {
                TransactionError::Abort(e) => e,
                TransactionError::Storage(e) => e.into(),
            })?;
        for t in [&self.sites, &self.account, &self.certs, &self.attempts, &self.meta] {
            t.flush()?;
        }
        if outcome == Applied::Done {
            self.changed.send_modify(|n| *n += 1);
        }
        Ok(outcome)
    }

    /// Everything, with the cursor, for an HA snapshot.
    pub fn snapshot(&self) -> Result<AcmeSnapshot, AcmeStoreError> {
        let _guard = self.lock();
        let sites = self.sites()?;
        let mut certs = Vec::new();
        for s in &sites {
            for c in self.certs(&s.name)? {
                certs.push((s.name.clone(), c));
            }
        }
        let mut attempts = Vec::new();
        for s in &sites {
            if let Some(a) = self.attempt(&s.name)? {
                attempts.push((s.name.clone(), a));
            }
        }
        Ok(AcmeSnapshot {
            sites,
            account: self.account()?,
            certs,
            attempts,
            applied_index: self.applied_index()?,
        })
    }

    /// Makes the store exactly `snap`, in one transaction.
    pub fn replace(&self, snap: &AcmeSnapshot) -> Result<(), AcmeStoreError> {
        let _guard = self.lock();
        let keys = |t: &sled::Tree| -> Result<Vec<sled::IVec>, sled::Error> { t.iter().keys().collect() };
        let (old_sites, old_certs, old_attempts) =
            (keys(&self.sites)?, keys(&self.certs)?, keys(&self.attempts)?);
        (&self.sites, &self.account, &self.certs, &self.attempts, &self.meta)
            .transaction(|(sites, account, certs, attempts, meta)| {
                let err = |e: serde_json::Error| ConflictableTransactionError::Abort(AcmeStoreError::from(e));
                for k in &old_sites {
                    sites.remove(k.clone())?;
                }
                for k in &old_certs {
                    certs.remove(k.clone())?;
                }
                for k in &old_attempts {
                    attempts.remove(k.clone())?;
                }
                account.remove(ACCOUNT_KEY)?;
                for s in &snap.sites {
                    sites.insert(s.name.as_bytes(), serde_json::to_vec(s).map_err(err)?)?;
                }
                for (site, c) in &snap.certs {
                    certs.insert(cert_key(site, c.version), serde_json::to_vec(c).map_err(err)?)?;
                }
                for (site, a) in &snap.attempts {
                    attempts.insert(site.as_bytes(), serde_json::to_vec(a).map_err(err)?)?;
                }
                if let Some(a) = &snap.account {
                    account.insert(ACCOUNT_KEY, serde_json::to_vec(a).map_err(err)?)?;
                }
                match snap.applied_index {
                    Some(i) => meta.insert(APPLIED_KEY, &i.to_be_bytes())?,
                    None => meta.remove(APPLIED_KEY)?,
                };
                Ok::<_, ConflictableTransactionError<AcmeStoreError>>(())
            })
            .map_err(|e| match e {
                TransactionError::Abort(e) => e,
                TransactionError::Storage(e) => e.into(),
            })?;
        for t in [&self.sites, &self.account, &self.certs, &self.attempts, &self.meta] {
            t.flush()?;
        }
        self.changed.send_modify(|n| *n += 1);
        Ok(())
    }
}

/// Lets the transaction closure serialize different types through one helper.
mod erased {
    pub trait Ser {
        fn to_json(&self) -> Result<Vec<u8>, serde_json::Error>;
    }
    impl<T: serde::Serialize> Ser for T {
        fn to_json(&self) -> Result<Vec<u8>, serde_json::Error> {
            serde_json::to_vec(self)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::secrets::Sealed;

    fn store() -> (AcmeStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db = sled::open(dir.path()).unwrap();
        (AcmeStore::open(&db).unwrap(), dir)
    }

    fn sealed() -> Sealed {
        Sealed {
            key_id: "k1".into(),
            nonce: "n".into(),
            ciphertext: "c".into(),
        }
    }

    fn site(name: &str, names: &[&str]) -> Site {
        Site {
            name: name.into(),
            names: names.iter().map(ToString::to_string).collect(),
            gateways: vec!["gw1".into()],
            dns_plugin: "dns".into(),
            propagation_timeout_secs: 120,
            enabled: true,
        }
    }

    fn cert(version: u64, names: &[&str]) -> StoredCert {
        StoredCert {
            version,
            chain_pem: format!("chain{version}"),
            sealed_key: sealed(),
            not_after: 1000 + version,
            issued_at: 10,
            serial: "01".into(),
            names: names.iter().map(ToString::to_string).collect(),
        }
    }

    fn issued(site: &str, term: u64, c: StoredCert) -> AcmeOp {
        AcmeOp::CertIssued { site: site.into(), tagged_term: term, cert: c }
    }

    #[test]
    fn sites_round_trip_and_a_name_belongs_to_one_site() {
        let (s, _d) = store();
        assert_eq!(s.apply_local(&AcmeOp::SiteSet(site("a", &["a.example.com"]))).unwrap(), Applied::Done);
        assert_eq!(
            s.apply_local(&AcmeOp::SiteSet(site("b", &["x.example.com", "a.example.com"]))).unwrap(),
            Applied::NameTaken("a.example.com".into())
        );
        // Re-setting the same site with the same names is fine.
        assert_eq!(s.apply_local(&AcmeOp::SiteSet(site("a", &["a.example.com"]))).unwrap(), Applied::Done);
        assert_eq!(s.sites().unwrap().len(), 1);
        assert_eq!(s.site("a").unwrap().unwrap().names, vec!["a.example.com"]);
    }

    #[test]
    fn invalid_sites_are_rejected_deterministically() {
        let (s, _d) = store();
        let mut wild = site("w", &["*.example.com"]);
        wild.gateways.clear();
        assert!(matches!(s.apply_local(&AcmeOp::SiteSet(wild)).unwrap(), Applied::Invalid(_)));
        assert!(matches!(
            s.apply_local(&AcmeOp::SiteSet(site("Bad Name", &["a.example.com"]))).unwrap(),
            Applied::Invalid(_)
        ));
        assert!(s.sites().unwrap().is_empty());
    }

    #[test]
    fn the_first_account_wins() {
        let (s, _d) = store();
        let acct = |u: &str| StoredAccount { directory_url: "https://ca".into(), account_url: u.into(), sealed: sealed() };
        assert_eq!(s.apply_local(&AcmeOp::AccountSet(acct("one"))).unwrap(), Applied::Done);
        assert_eq!(s.apply_local(&AcmeOp::AccountSet(acct("two"))).unwrap(), Applied::Exists);
        assert_eq!(s.account().unwrap().unwrap().account_url, "one");
    }

    #[test]
    fn a_certificate_is_applied_only_in_its_term_at_the_next_version_for_the_current_names() {
        let (s, _d) = store();
        let names = ["a.example.com"];
        s.apply_local(&AcmeOp::SiteSet(site("a", &names))).unwrap();
        let at = |idx: u64, term: u64, op: AcmeOp| s.apply_at(idx, term, &op).unwrap();
        // Entry appended in term 4 but proposed in term 3: a deposed leader's late result.
        assert_eq!(at(2, 4, issued("a", 3, cert(1, &names))), Applied::WrongTerm);
        assert!(s.cert("a").unwrap().is_none());
        assert_eq!(at(3, 4, issued("a", 4, cert(2, &names))), Applied::Stale, "must start at 1");
        assert_eq!(at(4, 4, issued("a", 4, cert(1, &["other.example.com"]))), Applied::Stale);
        assert_eq!(at(5, 4, issued("a", 4, cert(1, &names))), Applied::Done);
        assert_eq!(at(6, 4, issued("a", 4, cert(1, &names))), Applied::Stale, "same version twice");
        assert_eq!(at(7, 4, issued("zzz", 4, cert(1, &names))), Applied::NoSuchSite);
    }

    #[test]
    fn only_the_two_newest_versions_are_kept_and_success_clears_the_attempt_state() {
        let (s, _d) = store();
        let names = ["a.example.com"];
        s.apply_local(&AcmeOp::SiteSet(site("a", &names))).unwrap();
        s.apply_local(&AcmeOp::Attempt {
            site: "a".into(),
            attempt: Attempt { failures: 3, next_attempt_at: 99, last_error: Some("x".into()), ..Attempt::default() },
        })
        .unwrap();
        for v in 1..=3 {
            assert_eq!(s.apply_local(&issued("a", 0, cert(v, &names))).unwrap(), Applied::Done);
        }
        let versions: Vec<u64> = s.certs("a").unwrap().iter().map(|c| c.version).collect();
        assert_eq!(versions, vec![2, 3]);
        assert_eq!(s.cert("a").unwrap().unwrap().version, 3);
        assert!(s.attempt("a").unwrap().is_none());
    }

    #[test]
    fn deleting_a_site_removes_its_certificates_and_attempt_state() {
        let (s, _d) = store();
        let names = ["a.example.com"];
        s.apply_local(&AcmeOp::SiteSet(site("a", &names))).unwrap();
        s.apply_local(&issued("a", 0, cert(1, &names))).unwrap();
        assert_eq!(s.apply_local(&AcmeOp::SiteDelete { name: "a".into() }).unwrap(), Applied::Done);
        assert!(s.cert("a").unwrap().is_none() && s.site("a").unwrap().is_none());
        assert_eq!(s.apply_local(&AcmeOp::SiteDelete { name: "a".into() }).unwrap(), Applied::NoSuchSite);
    }

    #[test]
    fn a_replayed_index_changes_nothing() {
        let (s, _d) = store();
        assert_eq!(s.apply_at(5, 0, &AcmeOp::SiteSet(site("a", &["a.example.com"]))).unwrap(), Applied::Done);
        assert_eq!(s.apply_at(5, 0, &AcmeOp::SiteDelete { name: "a".into() }).unwrap(), Applied::Replayed);
        assert_eq!(s.apply_at(3, 0, &AcmeOp::SiteDelete { name: "a".into() }).unwrap(), Applied::Replayed);
        assert!(s.site("a").unwrap().is_some());
        assert_eq!(s.applied_index().unwrap(), Some(5));
    }

    #[test]
    fn a_snapshot_restores_everything_on_another_replica() {
        let (a, _da) = store();
        let names = ["a.example.com"];
        a.apply_at(1, 0, &AcmeOp::SiteSet(site("a", &names))).unwrap();
        a.apply_at(2, 0, &AcmeOp::AccountSet(StoredAccount { directory_url: "u".into(), account_url: "acct".into(), sealed: sealed() })).unwrap();
        a.apply_at(3, 1, &issued("a", 1, cert(1, &names))).unwrap();
        a.apply_at(4, 1, &AcmeOp::Attempt { site: "a".into(), attempt: Attempt { failures: 1, ..Attempt::default() } }).unwrap();
        let snap = a.snapshot().unwrap();
        let (b, _db) = store();
        b.apply_at(1, 0, &AcmeOp::SiteSet(site("stale", &["old.example.com"]))).unwrap();
        b.replace(&snap).unwrap();
        assert_eq!(b.snapshot().unwrap(), snap);
        assert_eq!(b.applied_index().unwrap(), Some(4));
        assert!(b.site("stale").unwrap().is_none());
        assert_eq!(b.apply_at(4, 1, &AcmeOp::SiteDelete { name: "a".into() }).unwrap(), Applied::Replayed);
    }
}
