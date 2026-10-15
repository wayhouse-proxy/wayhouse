//! `standalone` vs. `slave` — an install-time fact about this controller
//! tier, install-time no longer meaning "fixed for the process lifetime"
//! since phase 12 slice 5 added [`crate::adopt`] (`docs/10` "Controller
//! role", "Adoption").
//!
//! `standalone` is today's phase 10+11 behaviour unchanged: the tier accepts
//! writes directly and is the top of its own subtree. `slave` never accepts
//! a write of its own — every write must originate at the root and arrive
//! here via [`crate::parent_client`] / [`crate::intent::relay`], the only
//! callers allowed to bypass the gate (they write straight to
//! [`crate::store::Store::put`] via `apply_revision`). The role is never
//! *inferred* from connectivity: a `slave` that loses its parent freezes on
//! last-known-good, it does not promote itself back to `standalone` on its
//! own (`docs/10` "never inferred from connectivity") — the only way this
//! process's role ever changes post-startup is the explicit, operator-
//! triggered flip in [`crate::adopt`].
//!
//! [`RoleHandle`] is the shared, mutable cell both `crate::api::AppState`
//! and `crate::intent::api::IntentState` hold a clone of — a plain `Role`
//! field would go stale on one of the two states the moment `adopt` flips
//! the other. A `RwLock` (not an atomic) because `Role` isn't representable
//! as a single machine word without inventing an encoding for one; this is
//! a control-plane fact read/written a handful of times per request, not a
//! hot path.

use std::fmt;
use std::sync::{Arc, PoisonError, RwLock};

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Role {
    Standalone,
    Slave,
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Role::Standalone => write!(f, "standalone"),
            Role::Slave => write!(f, "slave"),
        }
    }
}

/// A shared, cloneable handle onto this tier's current [`Role`]. Every clone
/// reads/writes the same underlying cell.
///
/// It also owns the gate between direct writes and adoption: a direct write
/// holds [`RoleHandle::write_gate`] from its role check through its store
/// commit, and [`crate::adopt`] holds [`RoleHandle::adoption_gate`] from its
/// quiescence check through the role claim, so no write can read `Standalone`
/// before the claim and commit history after the second quiescence check.
#[derive(Clone)]
pub struct RoleHandle {
    role: Arc<RwLock<Role>>,
    gate: Arc<tokio::sync::RwLock<()>>,
}

impl RoleHandle {
    pub fn new(role: Role) -> Self {
        RoleHandle {
            role: Arc::new(RwLock::new(role)),
            gate: Arc::new(tokio::sync::RwLock::new(())),
        }
    }

    pub fn get(&self) -> Role {
        *self.role.read().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn set(&self, role: Role) {
        *self.role.write().unwrap_or_else(PoisonError::into_inner) = role;
    }

    /// Held by a direct write from its role check through its store commit;
    /// any number of writes hold it at once, none while an adoption does.
    pub async fn write_gate(&self) -> tokio::sync::OwnedRwLockReadGuard<()> {
        self.gate.clone().read_owned().await
    }

    /// Held by an adoption from its quiescence check through the role claim;
    /// waits for every in-flight direct write and keeps new ones out.
    pub async fn adoption_gate(&self) -> tokio::sync::OwnedRwLockWriteGuard<()> {
        self.gate.clone().write_owned().await
    }

    /// Sets the role to `to` only if it is `from` now, atomically; `true` when it did.
    /// Lets exactly one of several concurrent callers win a role flip.
    pub fn compare_and_set(&self, from: Role, to: Role) -> bool {
        let mut cell = self.role.write().unwrap_or_else(PoisonError::into_inner);
        if *cell == from {
            *cell = to;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clone_observes_a_set_made_through_the_original() {
        let handle = RoleHandle::new(Role::Standalone);
        let clone = handle.clone();
        assert_eq!(clone.get(), Role::Standalone);
        handle.set(Role::Slave);
        assert_eq!(clone.get(), Role::Slave);
    }

    #[test]
    fn compare_and_set_succeeds_only_from_the_expected_role() {
        let handle = RoleHandle::new(Role::Standalone);
        assert!(handle.compare_and_set(Role::Standalone, Role::Slave));
        assert!(!handle.compare_and_set(Role::Standalone, Role::Slave));
        assert_eq!(handle.get(), Role::Slave);
    }

    #[tokio::test]
    async fn the_adoption_gate_waits_for_an_in_flight_write_and_blocks_new_ones() {
        let handle = RoleHandle::new(Role::Standalone);
        let write = handle.write_gate().await;
        let second_write = handle.write_gate().await; // writes share the gate

        let adopting = tokio::spawn({
            let handle = handle.clone();
            async move { handle.adoption_gate().await }
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(
            !adopting.is_finished(),
            "adoption must wait for in-flight writes"
        );

        drop(write);
        drop(second_write);
        let adoption = tokio::time::timeout(std::time::Duration::from_secs(2), adopting)
            .await
            .expect("adoption proceeds once the writes finish")
            .unwrap();

        let late_write = tokio::spawn({
            let handle = handle.clone();
            async move { handle.write_gate().await }
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(
            !late_write.is_finished(),
            "a write must wait out an adoption"
        );
        drop(adoption);
        tokio::time::timeout(std::time::Duration::from_secs(2), late_write)
            .await
            .expect("the write proceeds once the adoption is done")
            .unwrap();
    }
}
