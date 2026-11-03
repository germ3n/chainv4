//! The gate: admits transactions to the mempool off the node loop, so a loop busy building or
//! importing a block never keeps clients or peers waiting, nor lets the mempool run dry. The
//! verifier hands it each transaction once its signature is checked; it checks the rest against
//! the tip's state without a lock, puts it in the mempool, which the loop locks only to pick a
//! block's transactions and drop what landed, answers the client, passes it on to peers and tells
//! subscribers. The loop then only counts what came of it.

use {
    super::stream::NodeEvent,
    super::verifier::Arrival,
    crate::chain::Fees,
    crate::crypto::verifiable::VerifyError,
    crate::db::{Database, State},
    crate::mempool::{Mempool, MempoolError, MempoolLimits},
    crate::net::{Backlog, Fault, NetCommand, NetEvent, Origin, Verdict},
    crate::structs::hash::{Hash, HashType},
    crate::structs::tx::SignedTx,
    libp2p::PeerId,
    std::sync::atomic::{AtomicBool, Ordering},
    std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError},
    tokio::sync::{broadcast, mpsc as async_mpsc},
};

pub(super) struct Gate {
    mempool: Mutex<Mempool>,
    db: Arc<dyn Database>,
    /// The fees the next block charges, as the tip's state decides: what governance changes.
    fees: Mutex<Option<Arc<Fees>>>,
    /// The mempool's limits, kept here too so the gap is checked before a verify without locking
    /// the mempool, which a producer may hold while it picks a block.
    limits: Mutex<MempoolLimits>,
    hash: HashType,
    /// The chain's genesis id, which EIP-712 signatures are bound to.
    genesis: Hash,
    /// Whether the node is taking its state from a snapshot, when nothing can be checked.
    syncing: AtomicBool,
    commands: async_mpsc::UnboundedSender<NetCommand>,
    /// Where the node tells what it does, once it streams.
    events: OnceLock<broadcast::Sender<NodeEvent>>,
    backlog: Arc<Backlog>,
}

/// What came of the transactions the gate took, for the loop to count.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Tally {
    /// Submitted to this node and taken.
    pub(super) submitted: u64,
    /// Refused, submitted or gossiped.
    pub(super) refused: u64,
    /// Gossiped, by the peer that passed each on.
    pub(super) heard: Vec<PeerId>,
    /// Gossiped with a signature that does not hold, counted against its sender.
    pub(super) rejected: u64,
}

impl Tally {
    pub(super) fn add(&mut self, other: Tally) {
        self.submitted += other.submitted;
        self.refused += other.refused;
        self.rejected += other.rejected;
        self.heard.extend(other.heard);
    }

    pub(super) fn is_empty(&self) -> bool {
        *self == Tally::default()
    }
}

impl Gate {
    /// Charges what the next block charges, once the tip moved and governance may have changed it.
    pub(super) fn set_fees(&self, fees: Option<Arc<Fees>>) {
        *self.fees.lock().unwrap_or_else(PoisonError::into_inner) = fees;
    }

    pub(super) fn new(
        mempool: Mempool,
        db: Arc<dyn Database>,
        fees: Option<Arc<Fees>>,
        hash: HashType,
        genesis: Hash,
        commands: async_mpsc::UnboundedSender<NetCommand>,
        backlog: Arc<Backlog>,
    ) -> Gate {
        Gate {
            limits: Mutex::new(mempool.limits()),
            mempool: Mutex::new(mempool),
            db,
            fees: Mutex::new(fees),
            hash,
            genesis,
            syncing: AtomicBool::new(false),
            commands,
            events: OnceLock::new(),
            backlog,
        }
    }

    /// Holds the mempool to these limits from now on.
    pub(super) fn set_limits(&self, limits: MempoolLimits) {
        *self.limits.lock().unwrap_or_else(PoisonError::into_inner) = limits;
        self.mempool().set_limits(limits);
    }

    /// The mempool's limits, read without locking it.
    pub(super) fn limits(&self) -> MempoolLimits {
        *self.limits.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The chain's genesis id, which a transaction's signature is checked on.
    pub(super) fn genesis(&self) -> Hash {
        self.genesis
    }

    /// The mempool, locked: hold it only as long as the work takes.
    pub(super) fn mempool(&self) -> MutexGuard<'_, Mempool> {
        self.mempool.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(super) fn set_syncing(&self, syncing: bool) {
        self.syncing.store(syncing, Ordering::Relaxed);
    }

    /// Tells subscribers of queued transactions too, once the node streams.
    pub(super) fn stream(&self, events: broadcast::Sender<NodeEvent>) {
        let _ = self.events.set(events);
    }

    /// Takes an arrival that carries a transaction, admitting it and answering for it, and says
    /// what came of it; any other arrival comes back for the loop.
    pub(super) fn take(&self, arrival: Arrival) -> Result<Tally, Arrival> {
        let Arrival::Event { event, checked } = arrival else {
            return Err(arrival);
        };

        match *event {
            NetEvent::Submitted { tx, reply } => {
                self.backlog.release(Origin::Local);
                let verdict = self.submit(tx, checked);
                let tally = match &verdict {
                    Ok(_) => Tally { submitted: 1, ..Tally::default() },
                    Err(_) => Tally { refused: 1, ..Tally::default() },
                };

                // The client may have stopped waiting; the transaction stands either way.
                let _ = reply.send(verdict);

                Ok(tally)
            }
            NetEvent::Tx { from, tx, gossip } => {
                self.backlog.release(Origin::Peer(from));
                let id = tx.id(self.hash);
                let (refused, verdict) = match self.admit(tx.clone(), checked) {
                    Ok(()) => {
                        log::debug!("[node] queued transaction {id} from {from}");
                        // Passed on in this node's own next batch: batches are never forwarded
                        // whole, so what spreads is what each node took.
                        let _ = self.commands.send(NetCommand::PublishTx(tx.clone()));
                        self.publish(|| NodeEvent::Queued(Arc::new(tx)));

                        (0, Verdict::Accept)
                    }
                    Err(err) => {
                        log::debug!("[node] refused transaction {id} from {from}: {err}");

                        // A signature that does not hold is the sender's fault; the rest, such as
                        // a nonce used or a fee too low, is not anyone's.
                        (1, match err {
                            MempoolError::Invalid(_) => Verdict::Reject(Fault::Tx),
                            _ => Verdict::Ignore,
                        })
                    }
                };
                let _ = self.commands.send(NetCommand::Judge { peer: from, gossip, verdict });

                Ok(Tally { submitted: 0, refused, heard: vec![from], rejected: u64::from(matches!(verdict, Verdict::Reject(_))) })
            }
            event => Err(Arrival::Event { event: Box::new(event), checked }),
        }
    }

    /// Admits a transaction submitted to this node, and passes it on to peers.
    pub(super) fn submit(&self, tx: SignedTx, checked: Option<Result<(), VerifyError>>) -> Result<Hash, MempoolError> {
        let id = tx.id(self.hash);

        match self.admit(tx.clone(), checked) {
            Ok(()) => {
                log::info!("[node] accepted transaction {id} from {}", tx.sender());
                self.publish(|| NodeEvent::Queued(Arc::new(tx.clone())));
                let _ = self.commands.send(NetCommand::PublishTx(tx));

                Ok(id)
            }
            Err(err) => {
                log::info!("[node] refused submitted transaction {id}: {err}");

                Err(err)
            }
        }
    }

    /// Puts a transaction in the mempool: checked against the tip's state unlocked, then inserted
    /// under the lock.
    fn admit(&self, tx: SignedTx, checked: Option<Result<(), VerifyError>>) -> Result<(), MempoolError> {
        if self.syncing.load(Ordering::Relaxed) {
            return Err(MempoolError::Syncing);
        }

        let state = State::new(self.db.clone());
        let fees = self.fees.lock().unwrap_or_else(PoisonError::into_inner).clone();

        // The cheap checks first: a transaction that cannot pay, whose nonce is used or is too far
        // ahead is turned away before any signature is verified. The verifier skips the verify for
        // such a one too, so checked is None and the fallback here never runs for it.
        let next = Mempool::affordable(&tx, &state, fees.as_deref())?;
        self.limits().reaches(tx.nonce, next)?;

        let checked = checked.unwrap_or_else(|| tx.verify_on(&self.genesis));
        Mempool::backed(&tx, checked, &self.genesis, &state)?;

        self.mempool().insert(tx, next, fees.as_deref())
    }

    /// Whether a transaction could be admitted on the tip's state as it stands, signatures aside:
    /// what the verifier checks before spending a verify on it. A transaction being synced from a
    /// snapshot is worth verifying, as its state is not yet whole to judge it by.
    pub(super) fn affordable(&self, tx: &SignedTx) -> bool {
        if self.syncing.load(Ordering::Relaxed) {
            return true;
        }

        let fees = self.fees.lock().unwrap_or_else(PoisonError::into_inner).clone();

        Mempool::affordable(tx, &State::new(self.db.clone()), fees.as_deref()).is_ok_and(|next| self.limits().reaches(tx.nonce, next).is_ok())
    }

    fn publish(&self, event: impl FnOnce() -> NodeEvent) {
        if let Some(events) = self.events.get()
            && events.receiver_count() > 0
        {
            let _ = events.send(event());
        }
    }
}
