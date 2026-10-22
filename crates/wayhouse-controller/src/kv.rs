//! The controller's embedded key-value store: named trees of byte keys and
//! values on top of `redb` (ADR 32, `docs/09-technology-choices.md`).
//!
//! The controller's stores were written against `sled`'s model (a database
//! of named trees, multi-tree transactions, ordered range scans, atomic
//! batches). This module keeps that small surface so the stores above it
//! stay as they were, and swaps the engine underneath for a maintained one.
//!
//! What differs from `sled`, and what callers can rely on:
//!
//! - Every write is durable when it returns (`redb` commits with an fsync),
//!   so [`Db::flush`] is a no-op kept for the call sites that document their
//!   durability point.
//! - There is one writer at a time per database. A transaction closure runs
//!   exactly once, never retried, and a write made by another thread waits
//!   for it. Do not write to the same database from inside a closure.
//! - A database is a directory holding one `db.redb` file. A directory left by
//!   `sled` (a build before 0.3.0) is refused with [`Error::LegacySled`]:
//!   the formats are unrelated and there is no migration.

use std::cell::RefCell;
use std::ops::{Bound, Deref, RangeBounds};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use redb::{ReadableDatabase, ReadableTable, ReadableTableMetadata};

/// Why a storage operation failed.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Another handle (or process) has the database open.
    #[error("could not acquire lock: {} is open elsewhere", .0.display())]
    Locked(PathBuf),
    /// The directory holds a `sled` database from an older build.
    #[error(
        "{} holds a sled database from a wayhouse build before 0.3.0; this build stores its data \
         in redb and cannot read it (there is no migration): move the directory aside or delete it",
        .0.display()
    )]
    LegacySled(PathBuf),
    #[error("storage I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("storage error: {0}")]
    Storage(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

macro_rules! storage_error_from {
    ($($ty:ty),* $(,)?) => {$(
        impl From<$ty> for Error {
            fn from(e: $ty) -> Self {
                Error::Storage(e.to_string())
            }
        }
    )*};
}

storage_error_from!(
    redb::TransactionError,
    redb::TableError,
    redb::StorageError,
    redb::CommitError,
);

/// An owned byte string handed back from a read.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct IVec(Vec<u8>);

impl Deref for IVec {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.0
    }
}

impl AsRef<[u8]> for IVec {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl From<&[u8]> for IVec {
    fn from(b: &[u8]) -> Self {
        IVec(b.to_vec())
    }
}

impl From<Vec<u8>> for IVec {
    fn from(b: Vec<u8>) -> Self {
        IVec(b)
    }
}

impl From<IVec> for Vec<u8> {
    fn from(v: IVec) -> Self {
        v.0
    }
}

pub mod transaction {
    //! The result types of [`Transactional::transaction`](super::Transactional).

    use super::Error;

    /// What a transaction closure returns to abandon its writes.
    #[derive(Debug)]
    pub enum ConflictableTransactionError<E> {
        /// Roll back and hand `E` to the caller.
        Abort(E),
        Storage(Error),
    }

    impl<E> From<Error> for ConflictableTransactionError<E> {
        fn from(e: Error) -> Self {
            ConflictableTransactionError::Storage(e)
        }
    }

    /// How a transaction ended without committing.
    #[derive(Debug)]
    pub enum TransactionError<E = ()> {
        /// The closure aborted; nothing was written.
        Abort(E),
        Storage(Error),
    }

    impl<E> From<Error> for TransactionError<E> {
        fn from(e: Error) -> Self {
            TransactionError::Storage(e)
        }
    }
}

use transaction::{ConflictableTransactionError, TransactionError};

type Table<'t> = redb::Table<'t, &'static [u8], &'static [u8]>;

const DB_FILE: &str = "db.redb";

/// A database: a directory of named [`Tree`]s.
#[derive(Clone)]
pub struct Db {
    inner: Arc<redb::Database>,
}

impl Db {
    /// Opens (or creates) the database in directory `path`.
    pub fn open(path: impl AsRef<Path>) -> Result<Db> {
        let path = path.as_ref();
        std::fs::create_dir_all(path)?;
        let file = path.join(DB_FILE);
        if !file.exists() && (path.join("conf").exists() || path.join("db").exists()) {
            return Err(Error::LegacySled(path.to_owned()));
        }
        match redb::Database::create(&file) {
            Ok(db) => Ok(Db {
                inner: Arc::new(db),
            }),
            Err(redb::DatabaseError::DatabaseAlreadyOpen) => Err(Error::Locked(path.to_owned())),
            Err(e) => Err(Error::Storage(e.to_string())),
        }
    }

    /// A database that lives in memory and is gone when the last handle drops.
    pub fn temporary() -> Result<Db> {
        let db = redb::Builder::new()
            .create_with_backend(redb::backends::InMemoryBackend::new())
            .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(Db {
            inner: Arc::new(db),
        })
    }

    /// The tree called `name`, created empty if it is new.
    pub fn open_tree(&self, name: &str) -> Result<Tree> {
        let tree = Tree {
            db: Arc::clone(&self.inner),
            name: name.into(),
        };
        // A table exists once created, so only a new tree needs a write
        // (and its fsync).
        if tree.read(|_| Ok(())).is_err() {
            let wtx = tree.db.begin_write()?;
            wtx.open_table(tree.definition())?;
            wtx.commit()?;
        }
        Ok(tree)
    }

    /// The names of every tree in the database.
    pub fn tree_names(&self) -> Vec<String> {
        let Ok(rtx) = self.inner.begin_read() else {
            return Vec::new();
        };
        let Ok(tables) = rtx.list_tables() else {
            return Vec::new();
        };
        tables
            .map(|t| redb::TableHandle::name(&t).to_owned())
            .collect()
    }

    /// Writes are durable when they return; kept for call sites that mark
    /// where they rely on that.
    #[allow(clippy::unused_self, clippy::unnecessary_wraps)]
    pub fn flush(&self) -> Result<()> {
        Ok(())
    }
}

/// An ordered map of byte keys to byte values inside a [`Db`].
#[derive(Clone)]
pub struct Tree {
    db: Arc<redb::Database>,
    name: Arc<str>,
}

/// Writes applied together by [`Tree::apply_batch`].
#[derive(Default)]
pub struct Batch {
    ops: Vec<(Vec<u8>, Option<Vec<u8>>)>,
}

impl Batch {
    pub fn insert(&mut self, key: impl AsRef<[u8]>, value: impl AsRef<[u8]>) {
        self.ops
            .push((key.as_ref().to_vec(), Some(value.as_ref().to_vec())));
    }

    pub fn remove(&mut self, key: impl AsRef<[u8]>) {
        self.ops.push((key.as_ref().to_vec(), None));
    }
}

impl Tree {
    pub fn name(&self) -> &str {
        &self.name
    }

    fn definition(&self) -> redb::TableDefinition<'_, &'static [u8], &'static [u8]> {
        redb::TableDefinition::new(&self.name)
    }

    fn read<R>(
        &self,
        f: impl FnOnce(&redb::ReadOnlyTable<&'static [u8], &'static [u8]>) -> Result<R>,
    ) -> Result<R> {
        let rtx = self.db.begin_read()?;
        let table = rtx.open_table(self.definition())?;
        f(&table)
    }

    fn write<R>(&self, f: impl FnOnce(&mut Table<'_>) -> Result<R>) -> Result<R> {
        let wtx = self.db.begin_write()?;
        let out = f(&mut wtx.open_table(self.definition())?)?;
        wtx.commit()?;
        Ok(out)
    }

    pub fn get(&self, key: impl AsRef<[u8]>) -> Result<Option<IVec>> {
        self.read(|t| Ok(t.get(key.as_ref())?.map(|v| IVec::from(v.value()))))
    }

    pub fn contains_key(&self, key: impl AsRef<[u8]>) -> Result<bool> {
        self.read(|t| Ok(t.get(key.as_ref())?.is_some()))
    }

    /// Sets `key`, returning the value it replaced.
    pub fn insert(&self, key: impl AsRef<[u8]>, value: impl AsRef<[u8]>) -> Result<Option<IVec>> {
        self.write(|t| {
            Ok(t.insert(key.as_ref(), value.as_ref())?
                .map(|v| IVec::from(v.value())))
        })
    }

    /// Removes `key`, returning the value it held.
    pub fn remove(&self, key: impl AsRef<[u8]>) -> Result<Option<IVec>> {
        self.write(|t| Ok(t.remove(key.as_ref())?.map(|v| IVec::from(v.value()))))
    }

    /// Applies the batch in one transaction; for one key the last op wins.
    #[allow(clippy::needless_pass_by_value)] // consumed, as the caller's batch is spent
    pub fn apply_batch(&self, batch: Batch) -> Result<()> {
        self.write(|t| {
            for (key, value) in &batch.ops {
                match value {
                    Some(v) => t.insert(key.as_slice(), v.as_slice())?.map(drop),
                    None => t.remove(key.as_slice())?.map(drop),
                };
            }
            Ok(())
        })
    }

    pub fn len(&self) -> Result<usize> {
        self.read(|t| Ok(usize::try_from(t.len()?).unwrap_or(usize::MAX)))
    }

    pub fn is_empty(&self) -> Result<bool> {
        Ok(self.len()? == 0)
    }

    /// The entry with the greatest key.
    pub fn last(&self) -> Result<Option<(IVec, IVec)>> {
        self.iter().next_back().transpose()
    }

    /// Writes are durable when they return; see [`Db::flush`].
    #[allow(clippy::unused_self, clippy::unnecessary_wraps)]
    pub fn flush(&self) -> Result<()> {
        Ok(())
    }

    /// Writes are durable when they return; see [`Db::flush`].
    #[allow(clippy::unused_async, clippy::unused_self, clippy::unnecessary_wraps)]
    pub async fn flush_async(&self) -> Result<()> {
        Ok(())
    }

    /// Every entry in key order.
    pub fn iter(&self) -> Iter {
        self.scan(&Bound::Unbounded, &Bound::Unbounded)
    }

    /// The entries whose key lies in `range`, in key order.
    pub fn range<K: AsRef<[u8]>, R: RangeBounds<K>>(&self, range: R) -> Iter {
        self.scan(
            &range.start_bound().map(|k| k.as_ref().to_vec()),
            &range.end_bound().map(|k| k.as_ref().to_vec()),
        )
    }

    /// The entries whose key starts with `prefix`, in key order.
    pub fn scan_prefix(&self, prefix: impl AsRef<[u8]>) -> Iter {
        let prefix = prefix.as_ref();
        // The first key past every key with this prefix: drop trailing 0xff
        // bytes and bump the last one. All-0xff (or empty) has no such key.
        let mut end = prefix.to_vec();
        while end.last() == Some(&0xff) {
            end.pop();
        }
        let hi = match end.last_mut() {
            Some(last) => {
                *last += 1;
                Bound::Excluded(end)
            }
            None => Bound::Unbounded,
        };
        self.scan(&Bound::Included(prefix.to_vec()), &hi)
    }

    fn scan(&self, lo: &Bound<Vec<u8>>, hi: &Bound<Vec<u8>>) -> Iter {
        let range = self.read(|t| {
            let bounds = (
                lo.as_ref().map(Vec::as_slice),
                hi.as_ref().map(Vec::as_slice),
            );
            Ok(t.range_owned::<&[u8]>(bounds)?)
        });
        Iter {
            range: range.map_err(Some),
        }
    }

    /// Runs `f` against this tree in one transaction.
    pub fn transaction<F, A, E>(&self, f: F) -> std::result::Result<A, TransactionError<E>>
    where
        F: FnOnce(
            &TransactionalTree<'_>,
        ) -> std::result::Result<A, ConflictableTransactionError<E>>,
    {
        with_views(&[self], |mut views| f(&views.remove(0)))
    }
}

/// A snapshot of a range, read in key order. Each item is a key and a value,
/// or the error that stopped the read.
pub struct Iter {
    range: std::result::Result<redb::OwnedRange<&'static [u8], &'static [u8]>, Option<Error>>,
}

impl Iter {
    /// Only the keys.
    pub fn keys(self) -> impl DoubleEndedIterator<Item = Result<IVec>> {
        self.map(|r| r.map(|(k, _)| k))
    }

    /// Only the values.
    pub fn values(self) -> impl DoubleEndedIterator<Item = Result<IVec>> {
        self.map(|r| r.map(|(_, v)| v))
    }
}

type RawEntry = std::result::Result<
    (
        redb::OwnedAccessGuard<&'static [u8]>,
        redb::OwnedAccessGuard<&'static [u8]>,
    ),
    redb::StorageError,
>;

fn entry(item: RawEntry) -> Result<(IVec, IVec)> {
    let (k, v) = item?;
    Ok((IVec::from(k.value()), IVec::from(v.value())))
}

impl Iterator for Iter {
    type Item = Result<(IVec, IVec)>;

    fn next(&mut self) -> Option<Self::Item> {
        match &mut self.range {
            Ok(range) => range.next().map(entry),
            Err(e) => e.take().map(Err),
        }
    }
}

impl DoubleEndedIterator for Iter {
    fn next_back(&mut self) -> Option<Self::Item> {
        match &mut self.range {
            Ok(range) => range.next_back().map(entry),
            Err(e) => e.take().map(Err),
        }
    }
}

/// A tree as a transaction sees it: reads include the transaction's own writes.
pub struct TransactionalTree<'t> {
    table: RefCell<Table<'t>>,
}

impl TransactionalTree<'_> {
    pub fn get(&self, key: impl AsRef<[u8]>) -> Result<Option<IVec>> {
        let table = self.table.borrow();
        let found = table.get(key.as_ref())?.map(|v| IVec::from(v.value()));
        Ok(found)
    }

    pub fn insert(&self, key: impl AsRef<[u8]>, value: impl AsRef<[u8]>) -> Result<Option<IVec>> {
        let mut table = self.table.borrow_mut();
        let old = table
            .insert(key.as_ref(), value.as_ref())?
            .map(|v| IVec::from(v.value()));
        Ok(old)
    }

    pub fn remove(&self, key: impl AsRef<[u8]>) -> Result<Option<IVec>> {
        let mut table = self.table.borrow_mut();
        let old = table.remove(key.as_ref())?.map(|v| IVec::from(v.value()));
        Ok(old)
    }
}

/// Runs `f` over views of `trees` in one write transaction: committed when it
/// returns `Ok`, rolled back when it aborts or fails.
fn with_views<A, E>(
    trees: &[&Tree],
    f: impl for<'t> FnOnce(
        Vec<TransactionalTree<'t>>,
    ) -> std::result::Result<A, ConflictableTransactionError<E>>,
) -> std::result::Result<A, TransactionError<E>> {
    let Some(first) = trees.first() else {
        return Err(Error::Storage("a transaction needs at least one tree".into()).into());
    };
    if trees.iter().any(|t| !Arc::ptr_eq(&t.db, &first.db)) {
        return Err(Error::Storage("a transaction cannot span two databases".into()).into());
    }
    let wtx = first.db.begin_write().map_err(Error::from)?;
    let views = trees
        .iter()
        .map(|t| {
            Ok(TransactionalTree {
                table: RefCell::new(wtx.open_table(t.definition())?),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    match f(views) {
        Ok(out) => {
            wtx.commit().map_err(Error::from)?;
            Ok(out)
        }
        Err(ConflictableTransactionError::Abort(e)) => Err(TransactionError::Abort(e)),
        Err(ConflictableTransactionError::Storage(e)) => Err(TransactionError::Storage(e)),
    }
}

/// A set of trees that one transaction can span: a slice of `&Tree`, or a
/// tuple of them (up to seven). The closure receives the matching views.
pub trait Transactional {
    type Views<'t>: ?Sized;

    fn transaction<F, A, E>(&self, f: F) -> std::result::Result<A, TransactionError<E>>
    where
        F: for<'t> FnOnce(
            &Self::Views<'t>,
        ) -> std::result::Result<A, ConflictableTransactionError<E>>;
}

impl Transactional for [&Tree] {
    type Views<'t> = [TransactionalTree<'t>];

    fn transaction<F, A, E>(&self, f: F) -> std::result::Result<A, TransactionError<E>>
    where
        F: for<'t> FnOnce(
            &Self::Views<'t>,
        ) -> std::result::Result<A, ConflictableTransactionError<E>>,
    {
        with_views(self, |views| f(&views))
    }
}

macro_rules! transactional_tuple {
    ($($tree:ident),+) => {
        impl Transactional for ($(transactional_tuple!(@tree $tree),)+) {
            type Views<'t> = ($(transactional_tuple!(@view $tree, 't),)+);

            #[allow(non_snake_case)]
            fn transaction<F, A, E>(&self, f: F) -> std::result::Result<A, TransactionError<E>>
            where
                F: for<'t> FnOnce(
                    &Self::Views<'t>,
                ) -> std::result::Result<A, ConflictableTransactionError<E>>,
            {
                let ($($tree,)+) = self;
                with_views(&[$($tree),+], |views| {
                    let mut views = views.into_iter();
                    f(&($({ let _ = stringify!($tree); views.next().expect("one view per tree") },)+))
                })
            }
        }
    };
    (@tree $t:ident) => { &Tree };
    (@view $t:ident, $l:lifetime) => { TransactionalTree<$l> };
}

transactional_tuple!(A);
transactional_tuple!(A, B);
transactional_tuple!(A, B, C);
transactional_tuple!(A, B, C, D);
transactional_tuple!(A, B, C, D, E);
transactional_tuple!(A, B, C, D, E, F);
transactional_tuple!(A, B, C, D, E, F, G);

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Db {
        Db::temporary().unwrap()
    }

    fn keys(tree: &Tree) -> Vec<Vec<u8>> {
        tree.iter().keys().map(|k| k.unwrap().to_vec()).collect()
    }

    #[test]
    fn insert_get_remove_round_trip() {
        let t = db().open_tree("t").unwrap();
        assert_eq!(t.get(b"a").unwrap(), None);
        assert_eq!(t.insert(b"a", b"1").unwrap(), None);
        assert_eq!(t.get(b"a").unwrap().unwrap().to_vec(), b"1");
        assert_eq!(t.insert(b"a", b"2").unwrap().unwrap().to_vec(), b"1");
        assert!(t.contains_key(b"a").unwrap());
        assert_eq!(t.remove(b"a").unwrap().unwrap().to_vec(), b"2");
        assert!(!t.contains_key(b"a").unwrap());
        assert_eq!(t.remove(b"a").unwrap(), None);
    }

    #[test]
    fn trees_are_separate_namespaces() {
        let db = db();
        let (a, b) = (db.open_tree("a").unwrap(), db.open_tree("b").unwrap());
        a.insert(b"k", b"in a").unwrap();
        assert_eq!(b.get(b"k").unwrap(), None);
        assert_eq!(
            db.open_tree("a")
                .unwrap()
                .get(b"k")
                .unwrap()
                .unwrap()
                .to_vec(),
            b"in a"
        );
    }

    #[test]
    fn iteration_is_in_byte_order_and_reversible() {
        let t = db().open_tree("t").unwrap();
        for k in [3u64, 1, 256, 2] {
            t.insert(k.to_be_bytes(), b"v").unwrap();
        }
        let order: Vec<u64> = t
            .iter()
            .keys()
            .map(|k| u64::from_be_bytes(k.unwrap().as_ref().try_into().unwrap()))
            .collect();
        assert_eq!(order, [1, 2, 3, 256]);
        let last = t.iter().next_back().unwrap().unwrap().0;
        assert_eq!(last.as_ref(), 256u64.to_be_bytes());
    }

    #[test]
    fn range_honours_each_bound_kind() {
        let t = db().open_tree("t").unwrap();
        for k in 1u64..=5 {
            t.insert(k.to_be_bytes(), k.to_be_bytes()).unwrap();
        }
        let got = |r: (Bound<[u8; 8]>, Bound<[u8; 8]>)| -> Vec<u64> {
            t.range::<[u8; 8], _>(r)
                .values()
                .map(|v| u64::from_be_bytes(v.unwrap().as_ref().try_into().unwrap()))
                .collect()
        };
        let b = |n: u64| n.to_be_bytes();
        assert_eq!(got((Bound::Excluded(b(2)), Bound::Unbounded)), [3, 4, 5]);
        assert_eq!(got((Bound::Included(b(2)), Bound::Excluded(b(4)))), [2, 3]);
        assert_eq!(got((Bound::Unbounded, Bound::Included(b(2)))), [1, 2]);
        assert_eq!(
            got((Bound::Excluded(b(5)), Bound::Unbounded)),
            Vec::<u64>::new()
        );
    }

    #[test]
    fn scan_prefix_yields_only_matching_keys() {
        let t = db().open_tree("t").unwrap();
        for k in ["a\0x", "a\0y", "ab", "b\0x", "a"] {
            t.insert(k, b"v").unwrap();
        }
        let got: Vec<Vec<u8>> = t
            .scan_prefix(b"a\0")
            .keys()
            .map(|k| k.unwrap().to_vec())
            .collect();
        assert_eq!(got, [b"a\0x".to_vec(), b"a\0y".to_vec()]);
        // A prefix of 0xff bytes has no upper bound.
        t.insert([0xff, 0xff, 1], b"v").unwrap();
        assert_eq!(t.scan_prefix([0xff, 0xff]).count(), 1);
    }

    #[test]
    fn tree_names_lists_every_tree_opened() {
        let db = db();
        db.open_tree("b").unwrap();
        db.open_tree("a").unwrap();
        let mut names = db.tree_names();
        names.sort();
        assert_eq!(names, ["a", "b"]);
    }

    #[test]
    fn last_is_the_greatest_key() {
        let t = db().open_tree("t").unwrap();
        assert_eq!(t.last().unwrap(), None);
        t.insert(b"a", b"1").unwrap();
        t.insert(b"c", b"3").unwrap();
        t.insert(b"b", b"2").unwrap();
        let (k, v) = t.last().unwrap().unwrap();
        assert_eq!((k.to_vec(), v.to_vec()), (b"c".to_vec(), b"3".to_vec()));
    }

    #[test]
    fn len_and_is_empty_follow_the_contents() {
        let t = db().open_tree("t").unwrap();
        assert!(t.is_empty().unwrap());
        t.insert(b"a", b"1").unwrap();
        t.insert(b"b", b"1").unwrap();
        assert_eq!(t.len().unwrap(), 2);
        assert!(!t.is_empty().unwrap());
    }

    #[test]
    fn a_batch_applies_its_inserts_and_removes_together() {
        let t = db().open_tree("t").unwrap();
        t.insert(b"gone", b"x").unwrap();
        let mut batch = Batch::default();
        batch.insert(b"a", b"1");
        batch.remove(b"gone");
        batch.insert(b"b", b"2");
        t.apply_batch(batch).unwrap();
        assert_eq!(keys(&t), [b"a".to_vec(), b"b".to_vec()]);
    }

    #[test]
    fn a_later_batch_op_on_the_same_key_wins() {
        let t = db().open_tree("t").unwrap();
        let mut batch = Batch::default();
        batch.insert(b"k", b"1");
        batch.remove(b"k");
        batch.insert(b"k", b"3");
        t.apply_batch(batch).unwrap();
        assert_eq!(t.get(b"k").unwrap().unwrap().to_vec(), b"3");
    }

    #[test]
    fn a_transaction_reads_its_own_writes_and_commits_across_trees() {
        let db = db();
        let (a, b) = (db.open_tree("a").unwrap(), db.open_tree("b").unwrap());
        a.insert(b"n", b"1").unwrap();
        let seen = (&a, &b)
            .transaction(|(ta, tb)| {
                ta.insert(b"n", b"2")?;
                tb.insert(b"m", b"x")?;
                Ok::<_, ConflictableTransactionError<()>>(ta.get(b"n")?.unwrap().to_vec())
            })
            .unwrap();
        assert_eq!(seen, b"2");
        assert_eq!(a.get(b"n").unwrap().unwrap().to_vec(), b"2");
        assert_eq!(b.get(b"m").unwrap().unwrap().to_vec(), b"x");
    }

    #[test]
    fn an_aborted_transaction_writes_nothing_in_any_tree() {
        let db = db();
        let (a, b) = (db.open_tree("a").unwrap(), db.open_tree("b").unwrap());
        a.insert(b"n", b"1").unwrap();
        let out = (&a, &b).transaction(|(ta, tb)| {
            ta.insert(b"n", b"2")?;
            tb.insert(b"m", b"x")?;
            Err::<(), _>(ConflictableTransactionError::Abort("no"))
        });
        assert!(matches!(out, Err(TransactionError::Abort("no"))));
        assert_eq!(a.get(b"n").unwrap().unwrap().to_vec(), b"1");
        assert_eq!(b.get(b"m").unwrap(), None);
    }

    #[test]
    fn a_slice_of_trees_and_a_single_tree_can_transact() {
        let db = db();
        let (a, b) = (db.open_tree("a").unwrap(), db.open_tree("b").unwrap());
        let trees = [&a, &b];
        trees[..]
            .transaction(|views| {
                views[0].insert(b"k", b"a")?;
                views[1].insert(b"k", b"b")?;
                Ok::<_, ConflictableTransactionError<()>>(())
            })
            .unwrap();
        a.transaction(|t| {
            let old = t.remove(b"k")?;
            assert_eq!(old.unwrap().to_vec(), b"a");
            Ok::<_, ConflictableTransactionError<()>>(())
        })
        .unwrap();
        assert_eq!(a.get(b"k").unwrap(), None);
        assert_eq!(b.get(b"k").unwrap().unwrap().to_vec(), b"b");
    }

    #[test]
    fn a_transaction_over_trees_of_two_databases_is_refused() {
        let (a, b) = (db().open_tree("a").unwrap(), db().open_tree("b").unwrap());
        let out = (&a, &b).transaction(|_| Ok::<_, ConflictableTransactionError<()>>(()));
        assert!(matches!(out, Err(TransactionError::Storage(_))));
    }

    #[test]
    fn data_survives_a_reopen_and_a_second_open_is_locked() {
        let dir = tempfile::tempdir().unwrap();
        let first = Db::open(dir.path()).unwrap();
        first.open_tree("t").unwrap().insert(b"k", b"v").unwrap();
        let err = Db::open(dir.path()).err().expect("second open must fail");
        assert!(matches!(err, Error::Locked(_)), "{err}");
        assert!(err.to_string().contains("could not acquire lock"), "{err}");
        drop(first);
        let again = Db::open(dir.path()).unwrap();
        assert_eq!(
            again
                .open_tree("t")
                .unwrap()
                .get(b"k")
                .unwrap()
                .unwrap()
                .to_vec(),
            b"v"
        );
    }

    #[test]
    fn a_sled_directory_is_refused_by_name() {
        let dir = tempfile::tempdir().unwrap();
        // The files a sled 0.34 database leaves in its directory.
        std::fs::write(dir.path().join("conf"), b"segment_size: 524288\n").unwrap();
        std::fs::write(dir.path().join("db"), b"").unwrap();
        let err = Db::open(dir.path()).err().expect("must be refused");
        assert!(matches!(err, Error::LegacySled(_)), "{err}");
        let text = err.to_string();
        assert!(text.contains(&dir.path().display().to_string()), "{text}");
        assert!(text.contains("sled"), "{text}");
        assert!(
            !dir.path().join(DB_FILE).exists(),
            "the old data must be left alone"
        );
    }

    #[test]
    fn a_missing_directory_is_created() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a").join("b");
        let db = Db::open(&nested).unwrap();
        db.open_tree("t").unwrap().insert(b"k", b"v").unwrap();
        assert!(nested.join(DB_FILE).is_file());
    }

    #[test]
    fn a_stored_value_can_be_larger_than_a_page() {
        let t = db().open_tree("blobs").unwrap();
        let big = vec![7u8; 3 * 1024 * 1024];
        t.insert(b"module", &big).unwrap();
        assert_eq!(t.get(b"module").unwrap().unwrap().to_vec(), big);
    }
}
