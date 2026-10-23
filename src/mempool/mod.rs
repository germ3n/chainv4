//! Transactions waiting for a block, keyed by sender and nonce so each sender's run in order.

use {
    crate::crypto::verifiable::{Verifiable, VerifyError},
    crate::db::{DbError, State},
    crate::registry::Accounts,
    crate::structs::address::Address,
    crate::structs::tx::SignedTx,
    r#macro::document,
    std::collections::BTreeMap,
    std::collections::btree_map::Entry,
    std::fmt,
};

#[document(
    summary = "Transactions waiting for a block. Admission checks everything that does not change with state, so a selected transaction can only fail at the nonce if another one with the same nonce landed first.",
    category = "Node",
    see_also = "SignedTx, Node",
)]
pub struct Mempool {
    txs: BTreeMap<(Address, u64), SignedTx>,
    capacity: usize,
}

#[document(
    summary = "Why a transaction was not admitted.",
    category = "Node",
)]
#[derive(Debug, PartialEq, Eq)]
pub enum MempoolError {
    #[document(summary = "The transaction, key or signature is not valid.")]
    Invalid(VerifyError),
    #[document(summary = "The sender has already used this nonce.")]
    Stale,
    #[document(summary = "A transaction from this sender with this nonce is already waiting.")]
    Duplicate,
    #[document(summary = "The mempool is full.")]
    Full,
    #[document(summary = "The database failed.")]
    Db(DbError),
}

impl fmt::Display for MempoolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MempoolError::Invalid(err) => write!(f, "{err}"),
            MempoolError::Stale => write!(f, "nonce already used"),
            MempoolError::Duplicate => write!(f, "already waiting"),
            MempoolError::Full => write!(f, "mempool full"),
            MempoolError::Db(err) => write!(f, "{err}"),
        }
    }
}

impl From<DbError> for MempoolError {
    fn from(err: DbError) -> MempoolError {
        MempoolError::Db(err)
    }
}

impl Mempool {
    #[document(
        parent = "Mempool",
        summary = "An empty mempool.",
        params = { capacity = "The most transactions it holds." },
    )]
    pub fn new(capacity: usize) -> Mempool {
        Mempool {
            txs: BTreeMap::new(),
            capacity,
        }
    }

    #[document(parent = "Mempool", summary = "How many transactions are waiting.")]
    pub fn len(&self) -> usize {
        self.txs.len()
    }

    #[document(parent = "Mempool", summary = "Whether nothing is waiting.")]
    pub fn is_empty(&self) -> bool {
        self.txs.is_empty()
    }

    #[document(
        parent = "Mempool",
        summary = "Admits a transaction after checking its signature and that its nonce is not already used.",
        params = {
            tx = "The transaction.",
            state = "The state at the tip.",
        },
    )]
    pub fn add(&mut self, tx: SignedTx, state: &State) -> Result<(), MempoolError> {
        if self.txs.len() >= self.capacity {
            return Err(MempoolError::Full);
        }

        tx.verify().map_err(MempoolError::Invalid)?;
        let sender = tx.sender();

        if tx.nonce < Accounts::new(state).get(&sender)?.nonce {
            return Err(MempoolError::Stale);
        }

        if self.txs.contains_key(&(sender, tx.nonce)) {
            return Err(MempoolError::Duplicate);
        }

        self.txs.insert((sender, tx.nonce), tx);

        Ok(())
    }

    #[document(
        parent = "Mempool",
        summary = "Picks transactions for a block: for each sender, their waiting transactions in nonce order starting at their current nonce, stopping at the first gap.",
        params = {
            state = "The state at the tip.",
            max = "The most to pick.",
        },
    )]
    pub fn select(&self, state: &State, max: usize) -> Result<Vec<SignedTx>, DbError> {
        let accounts = Accounts::new(state);
        let mut picked = Vec::new();
        let mut current: Option<(Address, u64)> = None;

        for ((sender, nonce), tx) in &self.txs {
            if picked.len() >= max {
                break;
            }

            let next = match current {
                Some((address, next)) if address == *sender => next,
                _ => accounts.get(sender)?.nonce,
            };

            if *nonce == next {
                picked.push(tx.clone());
                current = Some((*sender, next + 1));
            } else {
                // A gap, or a nonce already used: nothing after it can run yet.
                current = Some((*sender, u64::MAX));
            }
        }

        Ok(picked)
    }

    #[document(
        parent = "Mempool",
        summary = "Drops transactions whose nonce is already used, after a block landed.",
        params = { state = "The state at the new tip." },
    )]
    pub fn prune(&mut self, state: &State) -> Result<(), DbError> {
        let accounts = Accounts::new(state);
        let mut nonces: BTreeMap<Address, u64> = BTreeMap::new();

        for sender in self.txs.keys().map(|(sender, _)| *sender) {
            if let Entry::Vacant(entry) = nonces.entry(sender) {
                entry.insert(accounts.get(&sender)?.nonce);
            }
        }

        self.txs.retain(|(sender, nonce), _| *nonce >= nonces[sender]);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consensus::DevAuthority;
    use crate::crypto::keypair::Keypair;
    use crate::db::MemoryDb;
    use crate::structs::amount::Amount;
    use crate::structs::tx::Tx;
    use std::sync::Arc;

    fn keypair() -> Keypair {
        DevAuthority::dev_keypair()
    }

    fn transfer(keypair: &Keypair, nonce: u64) -> SignedTx {
        let from = keypair.public().address();
        let tx = Tx::Transfer {
            from,
            to: Keypair::generate(keypair.public().key_type()).unwrap().public().address(),
            amount: Amount::U128(1),
        };

        SignedTx::sign(tx, nonce, keypair)
    }

    fn state() -> State {
        State::new(Arc::new(MemoryDb::default()))
    }

    #[test]
    fn selection_follows_nonce_order_and_stops_at_gaps() {
        let state = state();
        let mut mempool = Mempool::new(10);
        let keypair = keypair();

        for nonce in [2, 0, 1, 4] {
            mempool.add(transfer(&keypair, nonce), &state).unwrap();
        }

        let nonces: Vec<u64> = mempool.select(&state, 10).unwrap().iter().map(|tx| tx.nonce).collect();

        assert_eq!(nonces, [0, 1, 2]);
        assert_eq!(mempool.select(&state, 2).unwrap().len(), 2);
    }

    #[test]
    fn bad_and_repeated_transactions_are_refused() {
        let state = state();
        let mut mempool = Mempool::new(2);
        let keypair = keypair();
        let mut forged = transfer(&keypair, 0);
        forged.nonce = 1;

        assert!(matches!(mempool.add(forged, &state), Err(MempoolError::Invalid(_))));

        mempool.add(transfer(&keypair, 0), &state).unwrap();

        assert_eq!(mempool.add(transfer(&keypair, 0), &state), Err(MempoolError::Duplicate));

        mempool.add(transfer(&keypair, 1), &state).unwrap();

        assert_eq!(mempool.add(transfer(&keypair, 2), &state), Err(MempoolError::Full));
    }

    #[test]
    fn used_nonces_are_stale_and_pruned() {
        let state = state();
        let mut mempool = Mempool::new(10);
        let keypair = keypair();
        mempool.add(transfer(&keypair, 0), &state).unwrap();
        mempool.add(transfer(&keypair, 1), &state).unwrap();
        Accounts::new(&state).bump_nonce(&keypair.public().address()).unwrap();

        assert_eq!(mempool.add(transfer(&keypair, 0), &state), Err(MempoolError::Stale));

        mempool.prune(&state).unwrap();

        assert_eq!(mempool.len(), 1);
        assert_eq!(mempool.select(&state, 10).unwrap()[0].nonce, 1);
    }
}
