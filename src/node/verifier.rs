//! The verifier: a thread between the network and the node loop that checks the signatures of
//! arriving transactions, a batch at a time on the chain's workers, and has the gate admit them,
//! so the loop spends its time on neither. Every other event passes through in the order it came,
//! and the loop hears only a tally of what came of each batch's transactions.

use {
    super::gate::{Gate, Tally},
    crate::chain::Workers,
    crate::crypto::verifiable::{Verifiable, VerifyError},
    crate::net::NetEvent,
    crate::structs::tx::SignedTx,
    std::sync::{Arc, mpsc},
};

/// The most events checked as one batch, as the node loop handles at most this many a pass.
const BATCH: usize = super::EVENT_BUDGET;

/// What reaches the node loop.
pub(super) enum Arrival {
    /// An event, with what checking its transaction's signature found, when it carries one and
    /// the verifier checked it; None when nobody checked yet, and the gate must. Boxed, as a
    /// transaction makes it ten times a tally's size.
    Event { event: Box<NetEvent>, checked: Option<Result<(), VerifyError>> },
    /// What came of transactions the gate took already, to count.
    Admitted(Tally),
}

impl Arrival {
    /// An event straight from the network.
    pub(super) fn unchecked(event: NetEvent) -> Arrival {
        Arrival::Event { event: Box::new(event), checked: None }
    }
}

/// Starts the verifier on the network's events, and returns where the rest come out, with a tally
/// for each batch's transactions after them. Stops when either end is gone.
pub(super) fn spawn(events: mpsc::Receiver<NetEvent>, workers: Arc<Workers>, gate: Arc<Gate>) -> std::io::Result<mpsc::Receiver<Arrival>> {
    let (arrivals, out) = mpsc::channel();

    std::thread::Builder::new().name("verify".to_string()).spawn(move || {
        while let Ok(first) = events.recv() {
            let mut batch = vec![first];

            while batch.len() < BATCH {
                match events.try_recv() {
                    Ok(event) => batch.push(event),
                    Err(_) => break,
                }
            }

            let mut tally = Tally::default();

            for arrival in check(batch, &workers) {
                match gate.take(arrival) {
                    Ok(taken) => tally.add(taken),
                    Err(arrival) => {
                        if arrivals.send(arrival).is_err() {
                            return;
                        }
                    }
                }
            }

            if !tally.is_empty() && arrivals.send(Arrival::Admitted(tally)).is_err() {
                return;
            }
        }
    })?;

    Ok(out)
}

/// Checks a batch's transactions side by side on the workers, keeping the batch's order.
pub(super) fn check(batch: Vec<NetEvent>, workers: &Workers) -> Vec<Arrival> {
    let txs: Vec<&SignedTx> = batch.iter().filter_map(carried).collect();
    let mut found = workers.map(&txs, |tx| tx.verify()).into_iter();

    batch
        .into_iter()
        .map(|event| {
            let checked = carried(&event).and_then(|_| found.next());

            Arrival::Event { event: Box::new(event), checked }
        })
        .collect()
}

/// The transaction an event carries for the mempool, if any.
fn carried(event: &NetEvent) -> Option<&SignedTx> {
    match event {
        NetEvent::Tx { tx, .. } | NetEvent::Submitted { tx, .. } => Some(tx),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::consensus::DevAuthority,
        crate::db::MemoryDb,
        crate::mempool::Mempool,
        crate::net::{Backlog, BacklogLimits, NetCommand},
        crate::structs::hash::HashType,
        tokio::sync::{mpsc as async_mpsc, oneshot},
        crate::crypto::keypair::Keypair,
        crate::structs::amount::Amount,
        crate::structs::tx::{FeeOffer, Tx},
        libp2p::PeerId,
        std::time::Duration,
    };

    fn transfer(nonce: u64) -> SignedTx {
        let authority = DevAuthority::dev_keypair();
        let tx = Tx::Transfer {
            from: authority.public().address(),
            to: Keypair::generate(authority.public().key_type()).unwrap().public().address(),
            amount: Amount::U128(1),
        };

        SignedTx::sign(tx, nonce, FeeOffer::NONE, &authority)
    }

    #[test]
    fn checking_keeps_every_event_in_order_and_adds_what_each_check_found() {
        let mut forged = transfer(0);
        // Its signature was over another nonce.
        forged.nonce = 7;
        let peer = PeerId::random();
        let batch = vec![NetEvent::Tx { from: peer, tx: transfer(0) }, NetEvent::PeerDown(peer), NetEvent::Tx { from: peer, tx: forged }];
        let checked = check(batch, &Workers::new());

        assert!(matches!(&checked[0], Arrival::Event { event, checked: Some(Ok(())) } if matches!(**event, NetEvent::Tx { .. })));
        assert!(matches!(&checked[1], Arrival::Event { event, checked: None } if matches!(**event, NetEvent::PeerDown(_))));
        assert!(matches!(&checked[2], Arrival::Event { event, checked: Some(Err(_)) } if matches!(**event, NetEvent::Tx { .. })));
    }

    #[test]
    fn the_thread_has_the_gate_admit_transactions_and_passes_the_rest_on() {
        let (network, events) = mpsc::channel();
        let (commands, mut published) = async_mpsc::unbounded_channel();
        let gate = Arc::new(Gate::new(
            Mempool::new(10),
            Arc::new(MemoryDb::default()),
            None,
            HashType::default_chain(),
            commands,
            Arc::new(Backlog::new(BacklogLimits::default())),
        ));
        let arrivals = spawn(events, Arc::new(Workers::new()), gate.clone()).unwrap();
        let mut forged = transfer(1);
        forged.nonce = 7;
        let peer = PeerId::random();
        let (reply, verdict) = oneshot::channel();

        network.send(NetEvent::Tx { from: peer, tx: transfer(0) }).unwrap();
        network.send(NetEvent::PeerDown(peer)).unwrap();
        network.send(NetEvent::Tx { from: peer, tx: forged }).unwrap();
        network.send(NetEvent::Submitted { tx: transfer(1), reply }).unwrap();

        // The loop sees the other event, and only tallies of the transactions.
        let mut tally = Tally::default();
        let mut others = 0;

        while tally.heard.len() < 2 || tally.submitted < 1 || others == 0 {
            match arrivals.recv_timeout(Duration::from_secs(10)).unwrap() {
                Arrival::Admitted(taken) => tally.add(taken),
                Arrival::Event { event, checked: None } if matches!(*event, NetEvent::PeerDown(_)) => others += 1,
                Arrival::Event { .. } => panic!("a transaction reached the loop"),
            }
        }

        assert_eq!((tally.heard, tally.submitted, tally.refused), (vec![peer, peer], 1, 1));
        assert!(verdict.blocking_recv().unwrap().is_ok(), "the client heard back from the gate");
        assert_eq!(gate.mempool().len(), 2);
        assert!(matches!(published.try_recv(), Ok(NetCommand::PublishTx(_))), "and its transaction went out");

        // It stops once the network is gone.
        drop(network);

        assert!(arrivals.recv_timeout(Duration::from_secs(10)).is_err());
    }
}
