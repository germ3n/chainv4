//! The verifier: a thread between the network and the node loop that checks the signatures of
//! arriving transactions, a batch at a time on the chain's workers, so the loop never spends its
//! time on them. Every event passes through in the order it came; only those that carry a
//! transaction are touched, and only to add what their check found.

use {
    crate::chain::Workers,
    crate::crypto::verifiable::{Verifiable, VerifyError},
    crate::net::NetEvent,
    crate::structs::tx::SignedTx,
    std::sync::{Arc, mpsc},
};

/// The most events checked as one batch, as the node loop handles at most this many a pass.
const BATCH: usize = super::EVENT_BUDGET;

/// An event, with what checking its transaction's signature found, when it carries one and the
/// verifier checked it.
pub(super) struct Arrival {
    pub(super) event: NetEvent,
    /// None when nobody checked yet, and the node must.
    pub(super) checked: Option<Result<(), VerifyError>>,
}

impl Arrival {
    /// An event straight from the network.
    pub(super) fn unchecked(event: NetEvent) -> Arrival {
        Arrival { event, checked: None }
    }
}

/// Starts the verifier on the network's events, and returns where the checked ones come out.
/// Stops when either end is gone.
pub(super) fn spawn(events: mpsc::Receiver<NetEvent>, workers: Arc<Workers>) -> std::io::Result<mpsc::Receiver<Arrival>> {
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

            for arrival in check(batch, &workers) {
                if arrivals.send(arrival).is_err() {
                    return;
                }
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

            Arrival { event, checked }
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
    fn the_thread_checks_transactions_and_keeps_every_event_in_order() {
        let (network, events) = mpsc::channel();
        let arrivals = spawn(events, Arc::new(Workers::new())).unwrap();
        let mut forged = transfer(0);
        // Its signature was over another nonce.
        forged.nonce = 7;
        let peer = PeerId::random();

        network.send(NetEvent::Tx { from: peer, tx: transfer(0) }).unwrap();
        network.send(NetEvent::PeerDown(peer)).unwrap();
        network.send(NetEvent::Tx { from: peer, tx: forged }).unwrap();
        let next = || arrivals.recv_timeout(Duration::from_secs(10)).unwrap();
        let (first, second, third) = (next(), next(), next());

        assert!(matches!(first, Arrival { event: NetEvent::Tx { .. }, checked: Some(Ok(())) }));
        assert!(matches!(second, Arrival { event: NetEvent::PeerDown(_), checked: None }));
        assert!(matches!(third, Arrival { event: NetEvent::Tx { .. }, checked: Some(Err(_)) }));

        // It stops once the network is gone.
        drop(network);

        assert!(arrivals.recv_timeout(Duration::from_secs(10)).is_err());
    }
}
