//! The peers a node met, kept in its database, so a restarted node dials them again rather than
//! starting from its --peer and bootnodes alone. A peer is kept by its id, which a peer now keeps
//! across starts, with the addresses identify says it listens on and when it was last seen; the
//! oldest go once there are more than MAX_REMEMBERED. A banned peer is forgotten, and its ban kept
//! in the bans column until it ends, so a restart lifts none.

use {
    crate::db::{Changes, Column, Database, DbError},
    crate::net::{Ban, Unban},
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

/// A ban as the database keeps it, by its peer's id.
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
struct KeptBan {
    /// The IP banned with it, as text, under --ban-ips.
    ip: Option<String>,
    /// When it ends, in milliseconds since 1970.
    until: u64,
}

/// Every ban kept, ended or not, with its key.
fn bans(db: &dyn Database) -> Result<Vec<(Vec<u8>, Ban)>, DbError> {
    let mut all = Vec::new();
    db.scan(Column::Bans, &[], &[], &mut |key, value| {
        if let (Ok(peer), Ok(kept)) = (PeerId::from_bytes(key), wincode::deserialize::<KeptBan>(value)) {
            all.push((key.to_vec(), Ban { peer, ip: kept.ip.and_then(|ip| ip.parse().ok()), until_ms: kept.until }));
        }

        true
    })?;

    Ok(all)
}

/// Keeps a ban until it ends: one made again replaces the one before.
pub(super) fn keep_ban(db: &dyn Database, ban: &Ban) -> Result<(), DbError> {
    let kept = KeptBan { ip: ban.ip.map(|ip| ip.to_string()), until: ban.until_ms };

    db.commit(Changes::from([((Column::Bans, ban.peer.to_bytes()), Some(wincode::serialize(&kept).expect("a ban always encodes")))]))
}

#[document(
    summary = "The bans this node kept that have not ended by now, for the network to restore as it starts; those that ended are let go.",
    category = "Node",
    params = { db = "The node's database.", now_ms = "The time on the wall clock, in milliseconds since 1970." },
    see_also = "NetConfig::banned, lift_bans",
)]
pub fn kept_bans(db: &dyn Database, now_ms: u64) -> Result<Vec<Ban>, DbError> {
    let (live, ended): (Vec<_>, Vec<_>) = bans(db)?.into_iter().partition(|(_, ban)| ban.until_ms > now_ms);

    if !ended.is_empty() {
        db.commit(ended.into_iter().map(|(key, _)| ((Column::Bans, key), None)).collect())?;
    }

    Ok(live.into_iter().map(|(_, ban)| ban).collect())
}

#[document(
    summary = "Lets go of kept bans: all, one peer's, or one IP's, whose peers stay banned by id. How many peer ids and IPs it let go.",
    category = "Node",
    params = { db = "The node's database.", which = "Which." },
    see_also = "kept_bans, Bans::lift",
)]
pub fn lift_bans(db: &dyn Database, which: Unban) -> Result<usize, DbError> {
    let mut changes = Changes::new();
    let mut lifted = 0;

    for (key, ban) in bans(db)? {
        match which {
            Unban::All => {
                lifted += 1 + usize::from(ban.ip.is_some());
                changes.insert((Column::Bans, key), None);
            }
            Unban::Peer(peer) if ban.peer == peer => {
                lifted += 1;
                changes.insert((Column::Bans, key), None);
            }
            Unban::Ip(ip) if ban.ip == Some(ip) => {
                lifted += 1;
                let kept = KeptBan { ip: None, until: ban.until_ms };
                changes.insert((Column::Bans, key), Some(wincode::serialize(&kept).expect("a ban always encodes")));
            }
            _ => {}
        }
    }

    // Lifted twice over one IP counts once.
    if let Unban::Ip(_) = which {
        lifted = lifted.min(1);
    }

    db.commit(changes)?;

    Ok(lifted)
}

#[cfg(test)]
mod tests {
    use {super::*, crate::db::MemoryDb};

    #[test]
    fn bans_are_kept_until_they_end_and_lifted_when_asked() {
        let db = MemoryDb::default();
        let (one, two, ended) = (PeerId::random(), PeerId::random(), PeerId::random());
        let ip: std::net::IpAddr = "203.0.113.20".parse().unwrap();
        keep_ban(&db, &Ban { peer: one, ip: Some(ip), until_ms: 5_000 }).unwrap();
        keep_ban(&db, &Ban { peer: two, ip: None, until_ms: 9_000 }).unwrap();
        keep_ban(&db, &Ban { peer: ended, ip: None, until_ms: 1_000 }).unwrap();

        // At 2 s, the one that ended goes, for good.
        let live = kept_bans(&db, 2_000).unwrap();
        assert_eq!(live.len(), 2);
        assert!(live.contains(&Ban { peer: one, ip: Some(ip), until_ms: 5_000 }));
        assert_eq!(bans(&db).unwrap().len(), 2, "the ended one is let go");

        // The IP lifted, its peer stays banned by id.
        assert_eq!(lift_bans(&db, Unban::Ip(ip)).unwrap(), 1);
        assert!(kept_bans(&db, 2_000).unwrap().contains(&Ban { peer: one, ip: None, until_ms: 5_000 }));

        assert_eq!(lift_bans(&db, Unban::Peer(two)).unwrap(), 1);
        assert_eq!(lift_bans(&db, Unban::All).unwrap(), 1);
        assert!(kept_bans(&db, 2_000).unwrap().is_empty());
    }

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
