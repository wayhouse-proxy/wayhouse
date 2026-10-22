//! ACME state in the controller's `kv` database. A standalone controller applies ops
//! through [`AcmeStore::apply_local`]; under HA the state machine is the only writer and uses
//! [`AcmeStore::apply_at`], which is crash-idempotent per Raft index (the entry and the cursor
//! commit in one transaction).

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use crate::kv::transaction::{ConflictableTransactionError, TransactionError};
use crate::kv::Transactional;

use crate::plugins::validate_sealed;

use super::types::{
    AcmeOp, AcmeSnapshot, Applied, Attempt, Gateway, HttpChallenge, Site, StoredAccount, StoredCert,
};

const APPLIED_KEY: &[u8] = b"applied";
const ACCOUNT_KEY: &[u8] = b"account";
/// Certificate versions kept per site: the current one and the one before it.
const KEEP_VERSIONS: usize = 2;

#[derive(Debug, thiserror::Error)]
pub enum AcmeStoreError {
    #[error("acme store: {0}")]
    Kv(#[from] crate::kv::Error),
    #[error("acme store record is corrupt: {0}")]
    Corrupt(#[from] serde_json::Error),
}

#[derive(Clone)]
pub struct AcmeStore {
    sites: crate::kv::Tree,
    account: crate::kv::Tree,
    /// `<site>\0<version, big-endian>` to a [`StoredCert`].
    certs: crate::kv::Tree,
    attempts: crate::kv::Tree,
    gateways: crate::kv::Tree,
    /// Token to [`HttpChallenge`].
    challenges: crate::kv::Tree,
    meta: crate::kv::Tree,
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

fn read<T: serde::de::DeserializeOwned>(
    v: Option<crate::kv::IVec>,
) -> Result<Option<T>, AcmeStoreError> {
    v.map(|b| serde_json::from_slice(&b))
        .transpose()
        .map_err(Into::into)
}

impl AcmeStore {
    pub fn open(db: &crate::kv::Db) -> Result<Self, crate::kv::Error> {
        Ok(Self {
            sites: db.open_tree("acme_sites")?,
            account: db.open_tree("acme_account")?,
            certs: db.open_tree("acme_certs")?,
            attempts: db.open_tree("acme_attempts")?,
            gateways: db.open_tree("acme_gateways")?,
            challenges: db.open_tree("acme_challenges")?,
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
        read(
            self.certs
                .scan_prefix(cert_prefix(site))
                .values()
                .next_back()
                .transpose()?,
        )
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

    pub fn gateways(&self) -> Result<Vec<Gateway>, AcmeStoreError> {
        self.gateways
            .iter()
            .values()
            .map(|v| Ok(serde_json::from_slice(&v?)?))
            .collect()
    }

    pub fn gateway(&self, name: &str) -> Result<Option<Gateway>, AcmeStoreError> {
        read(self.gateways.get(name.as_bytes())?)
    }

    /// The HTTP-01 answer for `token` while it is still valid at `now`.
    pub fn challenge(
        &self,
        token: &str,
        now: u64,
    ) -> Result<Option<HttpChallenge>, AcmeStoreError> {
        Ok(
            read::<HttpChallenge>(self.challenges.get(token.as_bytes())?)?
                .filter(|c| c.expires_at > now),
        )
    }

    pub fn challenges(&self) -> Result<Vec<HttpChallenge>, AcmeStoreError> {
        self.challenges
            .iter()
            .values()
            .map(|v| Ok(serde_json::from_slice(&v?)?))
            .collect()
    }

    // ---- writes ----

    /// Applies `op` on a standalone controller: the next local index, and the entry term is
    /// the one the op was tagged with (there is no other leader).
    pub fn apply_local(&self, op: &AcmeOp) -> Result<Applied, AcmeStoreError> {
        // The cursor is read under the write lock, so two local writers cannot pick the same
        // index and have the second one dropped as a replay.
        let guard = self.lock();
        let next = self.applied_index()?.map_or(1, |i| i + 1);
        let term = match op {
            AcmeOp::CertIssued { tagged_term, .. } => *tagged_term,
            _ => 0,
        };
        self.apply_locked(&guard, next, term, op)
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
        let guard = self.lock();
        self.apply_locked(&guard, index, entry_term, op)
    }

    fn apply_locked(
        &self,
        _guard: &std::sync::MutexGuard<'_, ()>,
        index: u64,
        entry_term: u64,
        op: &AcmeOp,
    ) -> Result<Applied, AcmeStoreError> {
        // What a transaction cannot scan, read first; the lock keeps it from changing.
        let other_sites: Vec<Site> = match op {
            AcmeOp::SiteSet(site) => self
                .sites()?
                .into_iter()
                .filter(|s| s.name != site.name)
                .collect(),
            _ => Vec::new(),
        };
        let wipe_certs: Vec<crate::kv::IVec> = match op {
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
        let old_versions: Vec<crate::kv::IVec> = match op {
            AcmeOp::CertIssued { site, .. } => self
                .certs
                .scan_prefix(cert_prefix(site))
                .keys()
                .collect::<Result<_, _>>()?,
            _ => Vec::new(),
        };
        let expired: Vec<crate::kv::IVec> = match op {
            AcmeOp::ChallengesPrune { now } => self
                .challenges()?
                .into_iter()
                .filter(|c| c.expires_at <= *now)
                .map(|c| crate::kv::IVec::from(c.token.as_bytes()))
                .collect(),
            _ => Vec::new(),
        };
        let abort = ConflictableTransactionError::Abort;
        let outcome = (
            &self.sites,
            &self.account,
            &self.certs,
            &self.attempts,
            &self.gateways,
            &self.challenges,
            &self.meta,
        )
            .transaction(
                |(sites, account, certs, attempts, gateways, challenges, meta)| {
                    let covered = meta
                        .get(APPLIED_KEY)?
                        .and_then(|v| <[u8; 8]>::try_from(v.as_ref()).ok())
                        .map(u64::from_be_bytes);
                    if covered.is_some_and(|c| c >= index) {
                        return Ok(Applied::Replayed);
                    }
                    meta.insert(APPLIED_KEY, index.to_be_bytes())?;
                    let json = |v: &dyn erased::Ser| {
                        v.to_json().map_err(|e| abort(AcmeStoreError::from(e)))
                    };
                    let get_site = |name: &str| -> Result<
                        Option<Site>,
                        ConflictableTransactionError<AcmeStoreError>,
                    > {
                        match sites.get(name.as_bytes())? {
                            Some(v) => Ok(Some(
                                serde_json::from_slice(&v).map_err(|e| abort(e.into()))?,
                            )),
                            None => Ok(None),
                        }
                    };
                    match op {
                        AcmeOp::SiteSet(site) => {
                            if let Err(why) = site.validate() {
                                return Ok(Applied::Invalid(why));
                            }
                            let taken: HashSet<&str> = other_sites
                                .iter()
                                .flat_map(|o| o.names.iter().map(String::as_str))
                                .collect();
                            if let Some(n) = site.names.iter().find(|n| taken.contains(n.as_str()))
                            {
                                return Ok(Applied::NameTaken(n.clone()));
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
                            if let Some(old) = account.get(ACCOUNT_KEY)? {
                                // The first writer wins per CA; an account for another CA
                                // (after `--acme-ca` changed) replaces the stored one.
                                let old: StoredAccount =
                                    serde_json::from_slice(&old).map_err(|e| abort(e.into()))?;
                                if old.directory_url == acct.directory_url {
                                    return Ok(Applied::Exists);
                                }
                            }
                            account.insert(ACCOUNT_KEY, json(acct)?)?;
                            Ok(Applied::Done)
                        }
                        AcmeOp::CertIssued {
                            site,
                            tagged_term,
                            cert,
                        } => {
                            let Some(current) = get_site(site)? else {
                                return Ok(Applied::NoSuchSite);
                            };
                            if *tagged_term != entry_term {
                                return Ok(Applied::WrongTerm);
                            }
                            if cert.version != newest.unwrap_or(0) + 1
                                || cert.names != current.names
                            {
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
                        AcmeOp::GatewaySet(gw) => {
                            if !Site::valid_name(&gw.name) {
                                return Ok(Applied::Invalid(
                                    "a gateway name is 1 to 63 of a-z, 0-9 and '-'".into(),
                                ));
                            }
                            if gateways.get(gw.name.as_bytes())?.is_some() {
                                return Ok(Applied::Exists);
                            }
                            gateways.insert(gw.name.as_bytes(), json(gw)?)?;
                            Ok(Applied::Done)
                        }
                        AcmeOp::GatewayDelete { name } => {
                            if gateways.remove(name.as_bytes())?.is_none() {
                                return Ok(Applied::NoSuchGateway);
                            }
                            Ok(Applied::Done)
                        }
                        AcmeOp::Rewrap {
                            account: new_account,
                            certs: new_certs,
                        } => {
                            let mut skipped = Vec::new();
                            if let Some(item) = new_account {
                                let current: Option<StoredAccount> = match account
                                    .get(ACCOUNT_KEY)?
                                {
                                    Some(v) => Some(
                                        serde_json::from_slice(&v).map_err(|e| abort(e.into()))?,
                                    ),
                                    None => None,
                                };
                                match current {
                                    Some(mut a)
                                        if a.sealed.nonce == item.from_nonce
                                            && validate_sealed(&item.sealed).is_ok() =>
                                    {
                                        a.sealed = item.sealed.clone();
                                        account.insert(ACCOUNT_KEY, json(&a)?)?;
                                    }
                                    _ => skipped.push("account".to_string()),
                                }
                            }
                            for item in new_certs {
                                let key = cert_key(&item.site, item.version);
                                let current: Option<StoredCert> = match certs.get(&key)? {
                                    Some(v) => Some(
                                        serde_json::from_slice(&v).map_err(|e| abort(e.into()))?,
                                    ),
                                    None => None,
                                };
                                match current {
                                    Some(mut c)
                                        if c.sealed_key.nonce == item.from_nonce
                                            && validate_sealed(&item.sealed).is_ok() =>
                                    {
                                        c.sealed_key = item.sealed.clone();
                                        certs.insert(key, json(&c)?)?;
                                    }
                                    _ => skipped.push(format!("{}@{}", item.site, item.version)),
                                }
                            }
                            Ok(Applied::Rewrapped(skipped))
                        }
                        AcmeOp::ChallengeSet(c) => {
                            if let Err(why) = c.validate() {
                                return Ok(Applied::Invalid(why));
                            }
                            challenges.insert(c.token.as_bytes(), json(c)?)?;
                            Ok(Applied::Done)
                        }
                        AcmeOp::ChallengeDelete { token } => {
                            challenges.remove(token.as_bytes())?;
                            Ok(Applied::Done)
                        }
                        AcmeOp::ChallengesPrune { .. } => {
                            for k in &expired {
                                challenges.remove(k.clone())?;
                            }
                            Ok(Applied::Done)
                        }
                    }
                },
            )
            .map_err(|e| match e {
                TransactionError::Abort(e) => e,
                TransactionError::Storage(e) => e.into(),
            })?;
        for t in [
            &self.sites,
            &self.account,
            &self.certs,
            &self.attempts,
            &self.gateways,
            &self.challenges,
            &self.meta,
        ] {
            t.flush()?;
        }
        if matches!(outcome, Applied::Done | Applied::Rewrapped(_)) {
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
            gateways: self.gateways()?,
            challenges: self.challenges()?,
            applied_index: self.applied_index()?,
        })
    }

    /// Makes the store exactly `snap`, in one transaction.
    pub fn replace(&self, snap: &AcmeSnapshot) -> Result<(), AcmeStoreError> {
        let _guard = self.lock();
        let keys = |t: &crate::kv::Tree| -> Result<Vec<crate::kv::IVec>, crate::kv::Error> {
            t.iter().keys().collect()
        };
        let (old_sites, old_certs, old_attempts, old_gateways, old_challenges) = (
            keys(&self.sites)?,
            keys(&self.certs)?,
            keys(&self.attempts)?,
            keys(&self.gateways)?,
            keys(&self.challenges)?,
        );
        (
            &self.sites,
            &self.account,
            &self.certs,
            &self.attempts,
            &self.gateways,
            &self.challenges,
            &self.meta,
        )
            .transaction(
                |(sites, account, certs, attempts, gateways, challenges, meta)| {
                    let err = |e: serde_json::Error| {
                        ConflictableTransactionError::Abort(AcmeStoreError::from(e))
                    };
                    for k in &old_sites {
                        sites.remove(k.clone())?;
                    }
                    for k in &old_certs {
                        certs.remove(k.clone())?;
                    }
                    for k in &old_attempts {
                        attempts.remove(k.clone())?;
                    }
                    for k in &old_gateways {
                        gateways.remove(k.clone())?;
                    }
                    for k in &old_challenges {
                        challenges.remove(k.clone())?;
                    }
                    for c in &snap.challenges {
                        challenges
                            .insert(c.token.as_bytes(), serde_json::to_vec(c).map_err(err)?)?;
                    }
                    account.remove(ACCOUNT_KEY)?;
                    for g in &snap.gateways {
                        gateways.insert(g.name.as_bytes(), serde_json::to_vec(g).map_err(err)?)?;
                    }
                    for s in &snap.sites {
                        sites.insert(s.name.as_bytes(), serde_json::to_vec(s).map_err(err)?)?;
                    }
                    for (site, c) in &snap.certs {
                        certs.insert(
                            cert_key(site, c.version),
                            serde_json::to_vec(c).map_err(err)?,
                        )?;
                    }
                    for (site, a) in &snap.attempts {
                        attempts.insert(site.as_bytes(), serde_json::to_vec(a).map_err(err)?)?;
                    }
                    if let Some(a) = &snap.account {
                        account.insert(ACCOUNT_KEY, serde_json::to_vec(a).map_err(err)?)?;
                    }
                    match snap.applied_index {
                        Some(i) => meta.insert(APPLIED_KEY, i.to_be_bytes())?,
                        None => meta.remove(APPLIED_KEY)?,
                    };
                    Ok::<_, ConflictableTransactionError<AcmeStoreError>>(())
                },
            )
            .map_err(|e| match e {
                TransactionError::Abort(e) => e,
                TransactionError::Storage(e) => e.into(),
            })?;
        for t in [
            &self.sites,
            &self.account,
            &self.certs,
            &self.attempts,
            &self.gateways,
            &self.challenges,
            &self.meta,
        ] {
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
        let db = crate::kv::Db::open(dir.path()).unwrap();
        (AcmeStore::open(&db).unwrap(), dir)
    }

    #[test]
    fn concurrent_local_writes_are_never_lost() {
        let (store, _dir) = store();
        let threads: Vec<_> = (0..8)
            .map(|t| {
                let store = store.clone();
                std::thread::spawn(move || {
                    for i in 0..25 {
                        let name = format!("s{t}-{i}");
                        let host = format!("{name}.example.com");
                        let applied = store
                            .apply_local(&AcmeOp::SiteSet(site(&name, &[&host])))
                            .unwrap();
                        assert_eq!(applied, Applied::Done, "{name}");
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        assert_eq!(store.sites().unwrap().len(), 200);
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
            challenge: crate::acme::ChallengeKind::Dns01,
            dns_plugin: "dns".into(),
            challenge_zone: None,
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
            untrusted: false,
        }
    }

    fn issued(site: &str, term: u64, c: StoredCert) -> AcmeOp {
        AcmeOp::CertIssued {
            site: site.into(),
            tagged_term: term,
            cert: c,
        }
    }

    #[test]
    fn sites_round_trip_and_a_name_belongs_to_one_site() {
        let (s, _d) = store();
        assert_eq!(
            s.apply_local(&AcmeOp::SiteSet(site("a", &["a.example.com"])))
                .unwrap(),
            Applied::Done
        );
        assert_eq!(
            s.apply_local(&AcmeOp::SiteSet(site(
                "b",
                &["x.example.com", "a.example.com"]
            )))
            .unwrap(),
            Applied::NameTaken("a.example.com".into())
        );
        // Re-setting the same site with the same names is fine.
        assert_eq!(
            s.apply_local(&AcmeOp::SiteSet(site("a", &["a.example.com"])))
                .unwrap(),
            Applied::Done
        );
        assert_eq!(s.sites().unwrap().len(), 1);
        assert_eq!(s.site("a").unwrap().unwrap().names, vec!["a.example.com"]);
    }

    #[test]
    fn invalid_sites_are_rejected_deterministically() {
        let (s, _d) = store();
        let mut wild = site("w", &["*.example.com"]);
        wild.gateways.clear();
        assert!(matches!(
            s.apply_local(&AcmeOp::SiteSet(wild)).unwrap(),
            Applied::Invalid(_)
        ));
        assert!(matches!(
            s.apply_local(&AcmeOp::SiteSet(site("Bad Name", &["a.example.com"])))
                .unwrap(),
            Applied::Invalid(_)
        ));
        assert!(s.sites().unwrap().is_empty());
    }

    #[test]
    fn the_first_account_wins() {
        let (s, _d) = store();
        let acct = |u: &str| StoredAccount {
            directory_url: "https://ca".into(),
            account_url: u.into(),
            sealed: sealed(),
        };
        assert_eq!(
            s.apply_local(&AcmeOp::AccountSet(acct("one"))).unwrap(),
            Applied::Done
        );
        assert_eq!(
            s.apply_local(&AcmeOp::AccountSet(acct("two"))).unwrap(),
            Applied::Exists
        );
        assert_eq!(s.account().unwrap().unwrap().account_url, "one");
    }

    #[test]
    fn an_account_for_another_ca_replaces_the_stored_one() {
        let (s, _d) = store();
        let acct = |dir: &str, u: &str| StoredAccount {
            directory_url: dir.into(),
            account_url: u.into(),
            sealed: sealed(),
        };
        let set = |a| s.apply_local(&AcmeOp::AccountSet(a)).unwrap();
        assert_eq!(set(acct("https://staging", "one")), Applied::Done);
        assert_eq!(set(acct("https://prod", "two")), Applied::Done);
        assert_eq!(s.account().unwrap().unwrap().account_url, "two");
        assert_eq!(
            set(acct("https://prod", "three")),
            Applied::Exists,
            "a second leader at the same CA still loses"
        );
        assert_eq!(s.account().unwrap().unwrap().account_url, "two");
    }

    #[test]
    fn a_certificate_is_applied_only_in_its_term_at_the_next_version_for_the_current_names() {
        let (s, _d) = store();
        let names = ["a.example.com"];
        s.apply_local(&AcmeOp::SiteSet(site("a", &names))).unwrap();
        let at = |idx: u64, term: u64, op: AcmeOp| s.apply_at(idx, term, &op).unwrap();
        // Entry appended in term 4 but proposed in term 3: a deposed leader's late result.
        assert_eq!(
            at(2, 4, issued("a", 3, cert(1, &names))),
            Applied::WrongTerm
        );
        assert!(s.cert("a").unwrap().is_none());
        assert_eq!(
            at(3, 4, issued("a", 4, cert(2, &names))),
            Applied::Stale,
            "must start at 1"
        );
        assert_eq!(
            at(4, 4, issued("a", 4, cert(1, &["other.example.com"]))),
            Applied::Stale
        );
        assert_eq!(at(5, 4, issued("a", 4, cert(1, &names))), Applied::Done);
        assert_eq!(
            at(6, 4, issued("a", 4, cert(1, &names))),
            Applied::Stale,
            "same version twice"
        );
        assert_eq!(
            at(7, 4, issued("zzz", 4, cert(1, &names))),
            Applied::NoSuchSite
        );
    }

    #[test]
    fn only_the_two_newest_versions_are_kept_and_success_clears_the_attempt_state() {
        let (s, _d) = store();
        let names = ["a.example.com"];
        s.apply_local(&AcmeOp::SiteSet(site("a", &names))).unwrap();
        s.apply_local(&AcmeOp::Attempt {
            site: "a".into(),
            attempt: Attempt {
                failures: 3,
                next_attempt_at: 99,
                last_error: Some("x".into()),
                ..Attempt::default()
            },
        })
        .unwrap();
        for v in 1..=3 {
            assert_eq!(
                s.apply_local(&issued("a", 0, cert(v, &names))).unwrap(),
                Applied::Done
            );
        }
        let versions: Vec<u64> = s.certs("a").unwrap().iter().map(|c| c.version).collect();
        assert_eq!(versions, vec![2, 3]);
        assert_eq!(s.cert("a").unwrap().unwrap().version, 3);
        assert!(s.attempt("a").unwrap().is_none());
    }

    #[test]
    fn a_rewrap_skips_a_secret_whose_ciphertext_changed_since_it_was_read() {
        use super::super::types::{RewrapAccount, RewrapCert};
        let (s, _d) = store();
        s.apply_local(&AcmeOp::SiteSet(site("web", &["a.example.com"])))
            .unwrap();
        s.apply_local(&AcmeOp::AccountSet(StoredAccount {
            directory_url: "https://ca/dir".into(),
            account_url: "https://ca/acct".into(),
            sealed: sealed(),
        }))
        .unwrap();
        s.apply_local(&issued("web", 0, cert(1, &["a.example.com"])))
            .unwrap();
        let mut fresh = sealed();
        fresh.key_id = "0123456789abcdef".into();
        let stale = "not-the-stored-nonce".to_string();
        let op = AcmeOp::Rewrap {
            account: Some(RewrapAccount {
                from_nonce: stale.clone(),
                sealed: fresh.clone(),
            }),
            certs: vec![
                RewrapCert {
                    site: "web".into(),
                    version: 1,
                    from_nonce: stale,
                    sealed: fresh.clone(),
                },
                RewrapCert {
                    site: "gone".into(),
                    version: 1,
                    from_nonce: sealed().nonce,
                    sealed: fresh,
                },
            ],
        };
        assert_eq!(
            s.apply_local(&op).unwrap(),
            Applied::Rewrapped(vec!["account".into(), "web@1".into(), "gone@1".into()])
        );
        assert_eq!(s.account().unwrap().unwrap().sealed, sealed());
        assert_eq!(s.cert("web").unwrap().unwrap().sealed_key, sealed());
    }

    #[test]
    fn deleting_a_site_removes_its_certificates_and_attempt_state() {
        let (s, _d) = store();
        let names = ["a.example.com"];
        s.apply_local(&AcmeOp::SiteSet(site("a", &names))).unwrap();
        s.apply_local(&issued("a", 0, cert(1, &names))).unwrap();
        assert_eq!(
            s.apply_local(&AcmeOp::SiteDelete { name: "a".into() })
                .unwrap(),
            Applied::Done
        );
        assert!(s.cert("a").unwrap().is_none() && s.site("a").unwrap().is_none());
        assert_eq!(
            s.apply_local(&AcmeOp::SiteDelete { name: "a".into() })
                .unwrap(),
            Applied::NoSuchSite
        );
    }

    #[test]
    fn a_replayed_index_changes_nothing() {
        let (s, _d) = store();
        assert_eq!(
            s.apply_at(5, 0, &AcmeOp::SiteSet(site("a", &["a.example.com"])))
                .unwrap(),
            Applied::Done
        );
        assert_eq!(
            s.apply_at(5, 0, &AcmeOp::SiteDelete { name: "a".into() })
                .unwrap(),
            Applied::Replayed
        );
        assert_eq!(
            s.apply_at(3, 0, &AcmeOp::SiteDelete { name: "a".into() })
                .unwrap(),
            Applied::Replayed
        );
        assert!(s.site("a").unwrap().is_some());
        assert_eq!(s.applied_index().unwrap(), Some(5));
    }

    #[test]
    fn gateways_are_registered_once_deleted_and_carried_by_a_snapshot() {
        let (store, _dir) = store();
        let gw = |name: &str, hash: &str| Gateway {
            name: name.into(),
            token_hash: hash.into(),
            created_at: 7,
        };
        let set = |g: Gateway| store.apply_local(&AcmeOp::GatewaySet(g)).unwrap();
        assert_eq!(set(gw("edge", "aa")), Applied::Done);
        assert_eq!(
            set(gw("edge", "bb")),
            Applied::Exists,
            "a taken name is not rotated"
        );
        assert_eq!(store.gateway("edge").unwrap().unwrap().token_hash, "aa");
        assert!(matches!(set(gw("Bad_Name", "cc")), Applied::Invalid(_)));
        assert_eq!(set(gw("edge-2", "dd")), Applied::Done);
        let snap = store.snapshot().unwrap();
        assert_eq!(snap.gateways.len(), 2);

        let (other, _d2) = self::store();
        other.replace(&snap).unwrap();
        assert_eq!(other.gateways().unwrap(), store.gateways().unwrap());

        let del = |n: &str| {
            store
                .apply_local(&AcmeOp::GatewayDelete { name: n.into() })
                .unwrap()
        };
        assert_eq!(del("edge"), Applied::Done);
        assert_eq!(del("edge"), Applied::NoSuchGateway);
        assert!(store.gateway("edge").unwrap().is_none());
        // A snapshot taken before gateways existed still decodes.
        let old: AcmeSnapshot = serde_json::from_str(
            r#"{"sites":[],"account":null,"certs":[],"attempts":[],"applied_index":null}"#,
        )
        .unwrap();
        assert!(old.gateways.is_empty());
    }

    #[test]
    fn a_snapshot_restores_everything_on_another_replica() {
        let (a, _da) = store();
        let names = ["a.example.com"];
        a.apply_at(1, 0, &AcmeOp::SiteSet(site("a", &names)))
            .unwrap();
        a.apply_at(
            2,
            0,
            &AcmeOp::AccountSet(StoredAccount {
                directory_url: "u".into(),
                account_url: "acct".into(),
                sealed: sealed(),
            }),
        )
        .unwrap();
        a.apply_at(3, 1, &issued("a", 1, cert(1, &names))).unwrap();
        a.apply_at(
            4,
            1,
            &AcmeOp::Attempt {
                site: "a".into(),
                attempt: Attempt {
                    failures: 1,
                    ..Attempt::default()
                },
            },
        )
        .unwrap();
        let snap = a.snapshot().unwrap();
        let (b, _db) = store();
        b.apply_at(1, 0, &AcmeOp::SiteSet(site("stale", &["old.example.com"])))
            .unwrap();
        b.replace(&snap).unwrap();
        assert_eq!(b.snapshot().unwrap(), snap);
        assert_eq!(b.applied_index().unwrap(), Some(4));
        assert!(b.site("stale").unwrap().is_none());
        assert_eq!(
            b.apply_at(4, 1, &AcmeOp::SiteDelete { name: "a".into() })
                .unwrap(),
            Applied::Replayed
        );
    }
}
