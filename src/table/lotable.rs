use crate::sync::atomics::AtomicBox;
use crate::txn::prelude::*;

use std::collections::hash_map::RandomState;
use std::collections::HashMap;

use std::hash::Hash;
use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

const DEFAULT_CAP: usize = 128;

#[derive(Clone)]
///
/// Lever Transactional Table implementation with [Optimistic](TransactionConcurrency::Optimistic)
/// concurrency and [RepeatableRead](TransactionIsolation::RepeatableRead) isolation.
///
/// Transactional hash table fully concurrent and as long as no conflicts are made
/// it is both lock and wait free.
pub struct LOTable<K, V, S = RandomState>
where
    K: 'static + PartialEq + Eq + Hash + Clone + Send + Sync,
    V: 'static + Clone + Send + Sync,
    S: BuildHasher,
{
    latch: Vec<TVar<Arc<AtomicBox<Container<K, V>>>>>,
    txn_man: Arc<TxnManager>,
    txn: Arc<Txn>,
    hash_builder: S,
}

impl<K, V> LOTable<K, V, RandomState>
where
    K: PartialEq + Eq + Hash + Clone + Send + Sync,
    V: Clone + Send + Sync,
{
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CAP)
    }

    pub fn with_capacity(cap: usize) -> Self {
        Self::with_capacity_and_hasher(cap, RandomState::new())
    }
}

impl<K, V, S> LOTable<K, V, S>
where
    K: PartialEq + Eq + Hash + Clone + Send + Sync,
    V: Clone + Send + Sync,
    S: BuildHasher,
{
    fn with_capacity_and_hasher(cap: usize, hasher: S) -> LOTable<K, V, S> {
        let txn_man = Arc::new(TxnManager {
            txid: Arc::new(AtomicU64::new(GLOBAL_VCLOCK.load(Ordering::SeqCst))),
        });

        let txn: Arc<Txn> = Arc::new(txn_man.txn_build(
            TransactionConcurrency::Optimistic,
            TransactionIsolation::RepeatableRead,
            100_usize,
            1_usize,
            "default".into(),
        ));

        Self {
            latch: vec![TVar::new(Arc::new(AtomicBox::new(Container(HashMap::default())))); cap],
            txn_man,
            txn,
            hash_builder: hasher,
        }
    }

    #[inline]
    pub fn insert(&self, k: K, v: V) -> Option<Arc<Option<V>>> {
        let tvar = self.seek_tvar(&k);

        let previous = self.txn.begin(|t| {
            let previous: Arc<AtomicBox<Option<V>>> = Arc::new(AtomicBox::new(None));
            let container = t.read(&tvar);
            container.replace_with(|r| {
                let mut entries = r.0.clone();
                let p = entries.insert(k.clone(), v.clone());
                previous.replace_with(|_| p.clone());
                Container(entries)
            });

            previous
        });

        previous.extract().ok()
    }

    #[inline]
    pub fn get(&self, k: &K) -> Option<V> {
        let tvar = self.seek_tvar(k);

        self.txn.begin(|t| {
            let container = t.read(&tvar);
            let entries = container.get();
            entries.0.get(k).cloned()
        })
    }

    #[inline]
    pub fn clear(&mut self) {
        self.latch.clear();
        // TODO: Shrink to fit as a optimized table.
        // self.latch.shrink_to_fit();
    }

    fn hash(&self, key: &K) -> usize {
        let mut hasher = self.hash_builder.build_hasher();
        key.hash(&mut hasher);
        hasher.finish() as usize % self.latch.len()
    }

    fn seek_tvar(&self, key: &K) -> TVar<Arc<AtomicBox<Container<K, V>>>> {
        self.latch[self.hash(key)].clone()
    }

    ////////////////////////////////////////////////////////////////////////////////
    ////////// Transactional Area
    ////////////////////////////////////////////////////////////////////////////////

    pub fn tx_manager(&self) -> Arc<TxnManager> {
        self.txn_man.clone()
    }
}

#[derive(Clone)]
struct Container<K, V>(HashMap<K, V>)
where
    K: PartialEq + Hash + Clone + Send + Sync,
    V: Clone + Send + Sync;

// impl<K, V, S> Debug for LOTable<K, V, S>
//     where
//         K: 'static + PartialEq + Eq + Hash + Clone + Send + Sync + Debug,
//         V: 'static + Clone + Send + Sync + Debug,
//         S: std::hash::BuildHasher
// {
//     fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
//         f.debug_map().entries(self.iter()).finish()
//     }
// }