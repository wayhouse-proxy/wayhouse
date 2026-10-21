use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::solver::{HttpSolver, SolverError};

/// The answers an HTTP-01 server would give, keyed by token, that is also an [`HttpSolver`].
/// Clones share the answers.
#[derive(Clone, Default)]
pub struct MemHttp {
    answers: Arc<Mutex<BTreeMap<String, String>>>,
    fail_present: Arc<Mutex<Option<String>>>,
    /// Every `present` and `cleanup` in order, as `("present"|"cleanup", token)`.
    pub log: Arc<Mutex<Vec<(&'static str, String)>>>,
}

impl MemHttp {
    /// The body `GET /.well-known/acme-challenge/<token>` would return.
    pub fn answer(&self, token: &str) -> Option<String> {
        self.answers.lock().unwrap().get(token).cloned()
    }

    pub fn is_empty(&self) -> bool {
        self.answers.lock().unwrap().is_empty()
    }

    /// The next `present` fails with `message`.
    pub fn fail_next_present(&self, message: &str) {
        *self.fail_present.lock().unwrap() = Some(message.to_string());
    }
}

#[async_trait]
impl HttpSolver for MemHttp {
    async fn present(&self, token: &str, key_authorization: &str) -> Result<(), SolverError> {
        self.log
            .lock()
            .unwrap()
            .push(("present", token.to_string()));
        if let Some(m) = self.fail_present.lock().unwrap().take() {
            return Err(SolverError(m));
        }
        self.answers
            .lock()
            .unwrap()
            .insert(token.to_string(), key_authorization.to_string());
        Ok(())
    }

    async fn cleanup(&self, token: &str) -> Result<(), SolverError> {
        self.log
            .lock()
            .unwrap()
            .push(("cleanup", token.to_string()));
        self.answers.lock().unwrap().remove(token);
        Ok(())
    }
}
