//! `RaftLogStorage` + `RaftLogReader`: the replicated Raft log itself
//! (distinct from the config/intent revision logs it carries — see
//! `crate::ha` module doc). Adapted from `openraft`'s own
//! `examples/memstore` reference implementation, with one real change: that
//! example keeps the log in a `BTreeMap` (lost on restart, fine for a demo);
//! this one persists every entry, the vote, and the purge point to two
//! `kv` trees so a replica's log survives a process restart, not just its
//! applied state machine.

use std::fmt::Debug;
use std::ops::{Bound, RangeBounds};

use openraft::storage::{LogFlushed, RaftLogReader, RaftLogStorage};
use openraft::{LogId, LogState, RaftLogId, StorageError, StorageIOError, Vote};

use super::TypeConfig;

const VOTE_KEY: &[u8] = b"vote";
const LAST_PURGED_KEY: &[u8] = b"last_purged";

fn encode_idx(idx: u64) -> [u8; 8] {
    idx.to_be_bytes()
}

/// `kv`-backed Raft log. `Clone` (cheap — a `crate::kv::Tree` is an `Arc`
/// internally) rather than the reference implementation's
/// `Arc<Mutex<BTreeMap<..>>>`: every method reads/writes `kv` directly, so
/// there is no separate in-process cache to keep consistent with the
/// on-disk state, and the database already serializes writes.
#[derive(Clone)]
pub struct LogStore {
    entries: crate::kv::Tree,
    meta: crate::kv::Tree,
}

impl LogStore {
    pub fn open(db: &crate::kv::Db) -> Result<Self, crate::kv::Error> {
        Ok(LogStore {
            entries: db.open_tree("raft_log")?,
            meta: db.open_tree("raft_log_meta")?,
        })
    }

    fn io_err(e: impl std::error::Error + 'static) -> StorageError<NodeIdT> {
        StorageIOError::write(&e).into()
    }
}

type NodeIdT = super::NodeId;

impl RaftLogReader<TypeConfig> for LogStore {
    async fn try_get_log_entries<RB: RangeBounds<u64> + Clone + Debug + Send>(
        &mut self,
        range: RB,
    ) -> Result<Vec<openraft::Entry<TypeConfig>>, StorageError<NodeIdT>> {
        let start = match range.start_bound() {
            Bound::Included(&n) => Bound::Included(encode_idx(n)),
            Bound::Excluded(&n) => Bound::Excluded(encode_idx(n)),
            Bound::Unbounded => Bound::Unbounded,
        };
        let end = match range.end_bound() {
            Bound::Included(&n) => Bound::Included(encode_idx(n)),
            Bound::Excluded(&n) => Bound::Excluded(encode_idx(n)),
            Bound::Unbounded => Bound::Unbounded,
        };
        let mut out = Vec::new();
        for item in self.entries.range::<[u8; 8], _>((start, end)) {
            let (_, v) = item.map_err(Self::io_err)?;
            let entry: openraft::Entry<TypeConfig> =
                serde_json::from_slice(&v).map_err(Self::io_err)?;
            out.push(entry);
        }
        Ok(out)
    }
}

impl RaftLogStorage<TypeConfig> for LogStore {
    type LogReader = Self;

    async fn get_log_state(&mut self) -> Result<LogState<TypeConfig>, StorageError<NodeIdT>> {
        let last_purged_log_id = self
            .meta
            .get(LAST_PURGED_KEY)
            .map_err(Self::io_err)?
            .map(|v| serde_json::from_slice::<LogId<NodeIdT>>(&v))
            .transpose()
            .map_err(Self::io_err)?;

        let last = self.entries.last().map_err(Self::io_err)?;
        let last_log_id = match last {
            Some((_, v)) => {
                let entry: openraft::Entry<TypeConfig> =
                    serde_json::from_slice(&v).map_err(Self::io_err)?;
                Some(*entry.get_log_id())
            }
            None => last_purged_log_id,
        };

        Ok(LogState {
            last_purged_log_id,
            last_log_id,
        })
    }

    async fn get_log_reader(&mut self) -> Self::LogReader {
        self.clone()
    }

    async fn save_vote(&mut self, vote: &Vote<NodeIdT>) -> Result<(), StorageError<NodeIdT>> {
        let bytes = serde_json::to_vec(vote).map_err(Self::io_err)?;
        self.meta.insert(VOTE_KEY, bytes).map_err(Self::io_err)?;
        self.meta.flush_async().await.map_err(Self::io_err)?;
        Ok(())
    }

    async fn read_vote(&mut self) -> Result<Option<Vote<NodeIdT>>, StorageError<NodeIdT>> {
        self.meta
            .get(VOTE_KEY)
            .map_err(Self::io_err)?
            .map(|v| serde_json::from_slice(&v))
            .transpose()
            .map_err(Self::io_err)
    }

    async fn append<I>(
        &mut self,
        entries: I,
        callback: LogFlushed<TypeConfig>,
    ) -> Result<(), StorageError<NodeIdT>>
    where
        I: IntoIterator<Item = openraft::Entry<TypeConfig>> + Send,
        I::IntoIter: Send,
    {
        for entry in entries {
            let idx = entry.get_log_id().index;
            let bytes = serde_json::to_vec(&entry).map_err(Self::io_err)?;
            self.entries
                .insert(encode_idx(idx), bytes)
                .map_err(Self::io_err)?;
        }
        self.entries.flush_async().await.map_err(Self::io_err)?;
        callback.log_io_completed(Ok(()));
        Ok(())
    }

    async fn truncate(&mut self, log_id: LogId<NodeIdT>) -> Result<(), StorageError<NodeIdT>> {
        let keys: Vec<[u8; 8]> = self
            .entries
            .range::<[u8; 8], _>((Bound::Included(encode_idx(log_id.index)), Bound::Unbounded))
            .map(|item| item.map(|(k, _)| k))
            .collect::<Result<Vec<_>, _>>()
            .map_err(Self::io_err)?
            .into_iter()
            .map(|k| {
                let mut buf = [0u8; 8];
                buf.copy_from_slice(&k);
                buf
            })
            .collect();
        // One atomic batch: removing the keys one at a time, lowest first, lets a
        // crash part-way keep the high entries and drop the low ones, a hole
        // below the stale tail that `get_log_state` would report as the log's end.
        let mut batch = crate::kv::Batch::default();
        for k in keys {
            batch.remove(k);
        }
        self.entries.apply_batch(batch).map_err(Self::io_err)?;
        self.entries.flush_async().await.map_err(Self::io_err)?;
        Ok(())
    }

    async fn purge(&mut self, log_id: LogId<NodeIdT>) -> Result<(), StorageError<NodeIdT>> {
        let keys: Vec<[u8; 8]> = self
            .entries
            .range::<[u8; 8], _>((Bound::Unbounded, Bound::Included(encode_idx(log_id.index))))
            .map(|item| item.map(|(k, _)| k))
            .collect::<Result<Vec<_>, _>>()
            .map_err(Self::io_err)?
            .into_iter()
            .map(|k| {
                let mut buf = [0u8; 8];
                buf.copy_from_slice(&k);
                buf
            })
            .collect();
        for k in keys {
            self.entries.remove(k).map_err(Self::io_err)?;
        }
        let bytes = serde_json::to_vec(&log_id).map_err(Self::io_err)?;
        self.meta
            .insert(LAST_PURGED_KEY, bytes)
            .map_err(Self::io_err)?;
        self.entries.flush_async().await.map_err(Self::io_err)?;
        self.meta.flush_async().await.map_err(Self::io_err)?;
        Ok(())
    }
}

/// Scrubs the plaintext secrets out of the retained, already-applied `Config` and `RelayConfig`
/// entries from index `from` up to `up_to`, except an entry whose config is one of `keep` (the
/// exact bytes of a revision that holds its plaintext on purpose, see
/// `AppState::kept_bytes`). Returns how many entries it rewrote and the index to start the next
/// scan at: everything below it is scrubbed, so the next pass skips it. A kept entry holds the
/// mark back, because it is scrubbed once its revision retires.
///
/// A purely local rewrite, never replicated: openraft matches log entries on term and index,
/// never on payload, so the entry keeps its place and a follower that replays it stores a
/// scrubbed copy of a revision that is retired on the leader anyway. Entries above `up_to` (not
/// applied here yet) are never touched, so this node can still apply them.
///
/// Each rewrite is checked against the entry as it was read, inside one transaction: a purge or a
/// truncation that removed the entry meanwhile is not undone, and one that replaced it wins.
pub fn redact_applied(
    entries: &crate::kv::Tree,
    from: u64,
    up_to: u64,
    keep: &[Vec<u8>],
) -> Result<(usize, u64), crate::kv::Error> {
    let (changes, next) = plan_redaction(entries, from, up_to, keep)?;
    let written = commit_redaction(entries, &changes)?;
    // A rewrite that lost its race to a newer entry is looked at again.
    let next = if written == changes.len() { next } else { from };
    Ok((written, next))
}

/// One planned rewrite: the key, the entry as read, and its scrubbed form.
type Change = (Vec<u8>, Vec<u8>, Vec<u8>);

fn plan_redaction(
    entries: &crate::kv::Tree,
    from: u64,
    up_to: u64,
    keep: &[Vec<u8>],
) -> Result<(Vec<Change>, u64), crate::kv::Error> {
    use super::WriteRequest;
    use openraft::EntryPayload;

    let mut changes: Vec<Change> = Vec::new();
    let mut next = up_to.saturating_add(1);
    for item in entries.range::<[u8; 8], _>((
        Bound::Included(encode_idx(from)),
        Bound::Included(encode_idx(up_to)),
    )) {
        let (key, raw) = item?;
        // Most entries are not configs; skip the JSON decode for those.
        if !raw.windows(6).any(|w| w == b"Config") {
            continue;
        }
        let Ok(mut entry) = serde_json::from_slice::<openraft::Entry<TypeConfig>>(&raw) else {
            continue;
        };
        let EntryPayload::Normal(
            WriteRequest::Config { bytes, .. } | WriteRequest::RelayConfig { bytes, .. },
        ) = &mut entry.payload
        else {
            continue;
        };
        if keep.contains(bytes) {
            next = next.min(entry.log_id.index);
            continue;
        }
        let Some(text) = std::str::from_utf8(bytes).ok() else {
            continue;
        };
        let Some(scrubbed) = wayhouse_config::redact_secrets(text) else {
            continue;
        };
        *bytes = scrubbed.into_bytes();
        let new = serde_json::to_vec(&entry).expect("a log entry serializes");
        changes.push((key.to_vec(), raw.to_vec(), new));
    }
    Ok((changes, next))
}

fn commit_redaction(
    entries: &crate::kv::Tree,
    changes: &[Change],
) -> Result<usize, crate::kv::Error> {
    if changes.is_empty() {
        return Ok(0);
    }
    let written = entries
        .transaction(|t| {
            let mut n = 0;
            for (key, old, new) in changes {
                if t.get(key)?
                    .is_some_and(|now| now.as_ref() == old.as_slice())
                {
                    t.insert(key, new)?;
                    n += 1;
                }
            }
            Ok::<_, crate::kv::transaction::ConflictableTransactionError<std::convert::Infallible>>(
                n,
            )
        })
        .map_err(|e| match e {
            crate::kv::transaction::TransactionError::Storage(e) => e,
            crate::kv::transaction::TransactionError::Abort(never) => match never {},
        })?;
    entries.flush()?;
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use openraft::{CommittedLeaderId, Entry, EntryPayload, LogId};

    fn open() -> (LogStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::kv::Db::open(dir.path()).unwrap();
        (LogStore::open(&db).unwrap(), dir)
    }

    fn entry(index: u64) -> openraft::Entry<TypeConfig> {
        Entry {
            log_id: LogId::new(CommittedLeaderId::new(1, 0), index),
            payload: EntryPayload::Blank,
        }
    }

    /// `openraft::storage::LogFlushed::new` is crate-private upstream (only
    /// `openraft`'s own core ever constructs one to hand to `append`), so
    /// these tests seed entries straight into the backing tree instead of
    /// going through the trait method — exercising the same encode/decode
    /// and range-query logic `append`/`try_get_log_entries` share, without
    /// needing a real callback. `append` itself gets real exercise from
    /// `crates/wayhouse-fleet-tests`' multi-process HA test, which drives an
    /// actual `Raft` end to end.
    fn seed(store: &LogStore, entries: &[openraft::Entry<TypeConfig>]) {
        for e in entries {
            let bytes = serde_json::to_vec(e).unwrap();
            store
                .entries
                .insert(encode_idx(e.get_log_id().index), bytes)
                .unwrap();
        }
    }

    #[tokio::test]
    async fn seeded_entries_are_readable_and_persist_across_a_reopen() {
        let (store, dir) = open();
        seed(&store, &[entry(1), entry(2)]);

        let mut reader = store.clone();
        let read = reader.try_get_log_entries(1..=2).await.unwrap();
        assert_eq!(read.len(), 2);

        drop(store);
        drop(reader); // both clones must go — the file lock is held until the last handle drops
        let db = crate::store::reopen_when_unlocked(|| crate::kv::Db::open(dir.path()));
        let mut reopened = LogStore::open(&db).unwrap();
        let state = reopened.get_log_state().await.unwrap();
        assert_eq!(state.last_log_id.unwrap().index, 2);
    }

    #[tokio::test]
    async fn vote_round_trips() {
        let (mut store, _dir) = open();
        assert!(store.read_vote().await.unwrap().is_none());
        let vote = Vote::new(3, 7);
        store.save_vote(&vote).await.unwrap();
        assert_eq!(store.read_vote().await.unwrap(), Some(vote));
    }

    #[tokio::test]
    async fn purge_removes_entries_up_to_and_including_the_given_id() {
        let (mut store, _dir) = open();
        seed(&store, &[entry(1), entry(2), entry(3)]);

        store.purge(*entry(2).get_log_id()).await.unwrap();
        let remaining = store.try_get_log_entries(..).await.unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].get_log_id().index, 3);

        let state = store.get_log_state().await.unwrap();
        assert_eq!(state.last_purged_log_id.unwrap().index, 2);
    }

    const TOKEN: &str = "plain-admin-token-0123456789";

    fn config_text(extra: &str) -> Vec<u8> {
        format!("settings:\n  admin:\n    auth_token: \"{TOKEN}\"\n{extra}").into_bytes()
    }

    fn config_entry(index: u64, bytes: Vec<u8>) -> openraft::Entry<TypeConfig> {
        Entry {
            log_id: LogId::new(CommittedLeaderId::new(1, 0), index),
            payload: EntryPayload::Normal(super::super::WriteRequest::Config {
                bytes,
                stage: crate::api::Stage::promoted(),
                actor: Some("ops".into()),
                trial: None,
            }),
        }
    }

    fn payload_bytes(e: &openraft::Entry<TypeConfig>) -> Vec<u8> {
        match &e.payload {
            EntryPayload::Normal(super::super::WriteRequest::Config { bytes, .. }) => bytes.clone(),
            other => panic!("not a config entry: {other:?}"),
        }
    }

    #[tokio::test]
    async fn retired_config_entries_are_scrubbed_in_place_and_the_rest_is_left_alone() {
        let (mut store, _dir) = open();
        let (old, live, unapplied) = (
            config_text("# one\n"),
            config_text("# two\n"),
            config_text("# three\n"),
        );
        seed(
            &store,
            &[
                entry(1),
                config_entry(2, old),
                config_entry(3, live.clone()),
                config_entry(4, unapplied.clone()),
            ],
        );
        // Entries 1..=3 are applied here; `live` is a revision that keeps its plaintext.
        let n = redact_applied(&store.entries, 0, 3, std::slice::from_ref(&live))
            .unwrap()
            .0;
        assert_eq!(n, 1);
        let all = store.try_get_log_entries(..).await.unwrap();
        assert_eq!(all.len(), 4);
        assert!(!String::from_utf8(payload_bytes(&all[1]))
            .unwrap()
            .contains(TOKEN));
        assert!(String::from_utf8(payload_bytes(&all[1]))
            .unwrap()
            .contains("# one"));
        assert_eq!(
            payload_bytes(&all[2]),
            live,
            "a kept revision's entry is whole"
        );
        assert_eq!(
            payload_bytes(&all[3]),
            unapplied,
            "an entry not applied here is untouched"
        );
        // Same term and index, so the log still matches the leader's; a repeat does nothing.
        assert_eq!(*all[1].get_log_id(), *config_entry(2, vec![]).get_log_id());
        assert_eq!(redact_applied(&store.entries, 0, 3, &[live]).unwrap().0, 0);
    }

    #[tokio::test]
    async fn a_scan_starts_at_the_watermark_and_stops_below_a_kept_entry() {
        let (mut store, _dir) = open();
        let live = config_text("# live\n");
        seed(
            &store,
            &[
                config_entry(1, config_text("# one\n")),
                config_entry(2, live.clone()),
                config_entry(3, config_text("# three\n")),
            ],
        );
        // From 3 on: entry 1 is below the watermark and is not read at all.
        let (n, next) = redact_applied(&store.entries, 3, 3, &[]).unwrap();
        assert_eq!((n, next), (1, 4));
        let all = store.try_get_log_entries(..).await.unwrap();
        assert!(
            String::from_utf8(payload_bytes(&all[0]))
                .unwrap()
                .contains(TOKEN),
            "below the watermark"
        );
        // A full scan with `live` kept: the next watermark stays on the kept entry, so it is
        // looked at again once that revision retires.
        let (n, next) = redact_applied(&store.entries, 0, 3, std::slice::from_ref(&live)).unwrap();
        assert_eq!((n, next), (1, 2));
    }

    #[tokio::test]
    async fn a_rewrite_never_resurrects_a_purged_entry_or_replaces_a_newer_one() {
        let (mut store, _dir) = open();
        seed(
            &store,
            &[
                config_entry(1, config_text("# one\n")),
                config_entry(2, config_text("# two\n")),
            ],
        );
        // Planned from what was read; then entry 1 is purged and entry 2 is replaced.
        let plan = plan_redaction(&store.entries, 0, 2, &[]).unwrap().0;
        assert_eq!(plan.len(), 2);
        store.purge(*entry(1).get_log_id()).await.unwrap();
        let newer = serde_json::to_vec(&config_entry(2, config_text("# newer\n"))).unwrap();
        store.entries.insert(encode_idx(2), &newer).unwrap();

        assert_eq!(commit_redaction(&store.entries, &plan).unwrap(), 0);
        assert!(store.entries.get(encode_idx(1)).unwrap().is_none());
        assert_eq!(
            store.entries.get(encode_idx(2)).unwrap().unwrap().to_vec(),
            newer
        );
    }
}
