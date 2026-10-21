use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;

use crate::solver::{DnsSolver, SolverError};

/// An in-memory DNS zone that is also a [`DnsSolver`]. Clones share the records.
#[derive(Clone, Default)]
pub struct MemDns {
    records: Arc<Mutex<BTreeSet<(String, String)>>>,
    fail_present: Arc<Mutex<Option<String>>>,
    /// `from` to `to`: a lookup of `from` is answered from `to` (one hop).
    cnames: Arc<Mutex<std::collections::BTreeMap<String, String>>>,
    /// Every `present` and `cleanup` in order, as `("present"|"cleanup", fqdn)`.
    pub log: Arc<Mutex<Vec<(&'static str, String)>>>,
}

impl MemDns {
    /// Whether a resolver asking for `fqdn` would see the TXT `value`, following one CNAME.
    pub fn has(&self, fqdn: &str, value: &str) -> bool {
        let target = self
            .cnames
            .lock()
            .unwrap()
            .get(fqdn)
            .cloned()
            .unwrap_or_else(|| fqdn.to_string());
        self.records
            .lock()
            .unwrap()
            .contains(&(target, value.to_string()))
    }

    /// Makes `from` a CNAME to `to`.
    pub fn cname(&self, from: &str, to: &str) {
        self.cnames
            .lock()
            .unwrap()
            .insert(from.to_string(), to.to_string());
    }

    /// Whether the TXT `value` is stored at exactly `fqdn` (no CNAME followed).
    pub fn stored_at(&self, fqdn: &str, value: &str) -> bool {
        self.records
            .lock()
            .unwrap()
            .contains(&(fqdn.to_string(), value.to_string()))
    }

    pub fn is_empty(&self) -> bool {
        self.records.lock().unwrap().is_empty()
    }

    /// The next `present` fails with `message`.
    pub fn fail_next_present(&self, message: &str) {
        *self.fail_present.lock().unwrap() = Some(message.to_string());
    }
}

#[async_trait]
impl DnsSolver for MemDns {
    async fn present(&self, fqdn: &str, value: &str) -> Result<(), SolverError> {
        self.log.lock().unwrap().push(("present", fqdn.to_string()));
        if let Some(m) = self.fail_present.lock().unwrap().take() {
            return Err(SolverError(m));
        }
        self.records
            .lock()
            .unwrap()
            .insert((fqdn.to_string(), value.to_string()));
        Ok(())
    }

    async fn cleanup(&self, fqdn: &str, value: &str) -> Result<(), SolverError> {
        self.log.lock().unwrap().push(("cleanup", fqdn.to_string()));
        self.records
            .lock()
            .unwrap()
            .remove(&(fqdn.to_string(), value.to_string()));
        Ok(())
    }

    async fn wait_visible(
        &self,
        fqdn: &str,
        value: &str,
        _timeout: Duration,
    ) -> Result<(), SolverError> {
        if self.has(fqdn, value) {
            Ok(())
        } else {
            Err(SolverError(format!("{fqdn} never became visible")))
        }
    }
}
