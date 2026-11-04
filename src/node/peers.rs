//! The peers a node met, kept in its database, so a restarted node dials them again rather than
//! starting from its --peer and bootnodes alone. A peer is kept by its id, which a peer now keeps
//! across starts, with the addresses identify says it listens on and when it was last seen; the
//! oldest go once there are more than MAX_REMEMBERED. A banned peer is forgotten.

use {
    crate::db::{Changes, Column, Database, DbError},
    libp2p::{Multiaddr, PeerId},
    r#macro::document,
    wincode::{SchemaRead, SchemaWrite},
};

#[document(
    summary = "How many peers a node remembers between starts, the most recently seen, unless --tune peers.remembered says otherwise.",
    category = "Node",
    see_also = "remembered, NodeTuning::remembered",
)]
pub const MAX_REMEMBERED: usize = 64;

/// The most addresses kept for one peer.
const MAX_ADDRESSES: usize = 8;

/// A peer as the database keeps it.
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
struct Kept {
    addresses: Vec<String>,
    /// When it was last seen, in milliseconds since 1970.
    seen: u64,
}

/// Every peer kept, by id, oldest first.
fn kept(db: &dyn Database) -> Result<Vec<(Vec<u8>, Kept)>, DbError> {
    let mut all = Vec::new();
    db.scan(Column::Peers, &[], &[], &mut |key, value| {
        if let Ok(peer) = wincode::deserialize::<Kept>(value) {
            all.push((key.to_vec(), peer));
        }

        true
    })?;
    all.sort_by_key(|(_, peer)| peer.seen);

    Ok(all)
}

#[document(
    summary = "The addresses of the peers this node remembers, the most recently seen first, each ending in its /p2p/ id: what it dials at start, as bootnodes.",
    category = "Node",
    params = { db = "The node's database." },
    see_also = "MAX_REMEMBERED, NetConfig::remembered",
)]
pub fn remembered(db: &dyn Database) -> Result<Vec<Multiaddr>, DbError> {
    Ok(kept(db)?
        .into_iter()
        .rev()
        .flat_map(|(_, peer)| peer.addresses)
        .filter_map(|address| address.parse().ok())
        .collect())
}

/// Keeps a peer's addresses, seen now, letting the oldest go past the most it remembers.
pub(super) fn remember(db: &dyn Database, peer: &PeerId, addresses: &[Multiaddr], now: u64, most: usize) -> Result<(), DbError> {
    if addresses.is_empty() {
        return Ok(());
    }

    let kept = Kept {
        addresses: addresses.iter().take(MAX_ADDRESSES).map(ToString::to_string).collect(),
        seen: now,
    };
    let key = peer.to_bytes();
    let mut changes = Changes::new();
    let others = self::kept(db)?.into_iter().filter(|(other, _)| *other != key).collect::<Vec<_>>();

    for (old, _) in others.iter().take((others.len() + 1).saturating_sub(most.max(1))) {
        changes.insert((Column::Peers, old.clone()), None);
    }

    changes.insert((Column::Peers, key), Some(wincode::serialize(&kept).expect("a peer always encodes")));

    db.commit(changes)
}

/// Forgets a peer: a banned one.
pub(super) fn forget(db: &dyn Database, peer: &PeerId) -> Result<(), DbError> {
    let key = peer.to_bytes();

    match db.read(Column::Peers, &key)? {
        Some(_) => db.commit(Changes::from([((Column::Peers, key), None)])),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use {super::*, crate::db::MemoryDb};

    fn address(n: u16, peer: &PeerId) -> Multiaddr {
        format!("/ip4/203.0.113.1/udp/{n}/quic-v1/p2p/{peer}").parse().unwrap()
    }

    #[test]
    fn peers_are_kept_newest_first_and_the_oldest_go() {
        let db = MemoryDb::default();
        let peers: Vec<PeerId> = (0..MAX_REMEMBERED + 3).map(|_| PeerId::random()).collect();

        for (n, peer) in peers.iter().enumerate() {
            remember(&db, peer, &[address(n as u16 + 1000, peer)], n as u64, MAX_REMEMBERED).unwrap();
        }

        let dialed = remembered(&db).unwrap();
        assert_eq!(dialed.len(), MAX_REMEMBERED);
        assert_eq!(dialed[0], address((MAX_REMEMBERED + 2) as u16 + 1000, &peers[MAX_REMEMBERED + 2]), "the latest seen first");
        assert!(!dialed.contains(&address(1000, &peers[0])), "the oldest went");

        // Seen again, a peer moves up, and keeps one entry.
        let again = &peers[10];
        remember(&db, again, &[address(1, again), address(2, again)], 1_000, MAX_REMEMBERED).unwrap();
        let dialed = remembered(&db).unwrap();
        assert_eq!(&dialed[..2], &[address(1, again), address(2, again)]);
        assert_eq!(dialed.len(), MAX_REMEMBERED + 1, "two addresses for it now, no new entry");

        forget(&db, again).unwrap();
        assert!(!remembered(&db).unwrap().contains(&address(1, again)), "a banned peer is forgotten");
        forget(&db, &PeerId::random()).unwrap();
    }
}
