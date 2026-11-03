//! The member index: each multisig under the address of every member's key, for a wallet to list
//! every multisig its accounts belong to. A member is named by the address its key controls, as
//! member_at finds whom a member's own approval comes from, so an Ethereum or Bitcoin account,
//! whose address is a hash of its key, is found by the address it sends from.
//!
//! It is built from the multisig registry, not from blocks: whatever the chain's state says the
//! members are now, the index says too. Each block that changes a multisig's policy, making one or
//! changing its members, moves the index in the same commit, from the members before to those
//! after, so a member removed goes; and as it joins the block's undo record, a reorg takes it back
//! with the block. A node that starts keeping it, on a chain that has run for a while or after a
//! snapshot, builds it whole from the registry in one atomic write.
//!
//! It points the way only: GET /accounts/{address}/multisigs reads each policy it lists back from
//! state, and a light client proves the policy itself.

use {
    super::read_value,
    crate::crypto::hashable::Hashable,
    crate::db::{Column, Committed, Database, DbError, Read, State, address_key},
    crate::registry::multisig::{MultisigPolicy, Multisigs},
    crate::structs::address::Address,
    r#macro::document,
    std::collections::BTreeSet,
    std::sync::Arc,
    wincode::{SchemaRead, SchemaWrite},
};

/// Where in Meta a node marks that it keeps the index, and what its entries hold.
pub(super) const KEPT_KEY: &[u8] = b"members_kept";
const SCHEMA: u8 = 1;

#[derive(Clone, Copy, Debug, SchemaWrite, SchemaRead)]
pub(super) struct Kept {
    version: u8,
}

impl Hashable for Kept {}

#[document(
    summary = "Whether a database keeps the member index, with entries this build reads.",
    category = "Chain",
    see_also = "stored_multisigs_of, Chain::keep_member_index",
)]
pub fn stored_member_index(db: &dyn Read) -> Result<bool, DbError> {
    Ok(read_value::<Kept>(db, Column::Meta, KEPT_KEY)?.is_some_and(|kept| kept.version == SCHEMA))
}

/// Where a multisig sits under a member: the member's address, then the multisig's.
fn key(member: &Address, multisig: &Address) -> Vec<u8> {
    [address_key(member), address_key(multisig)].concat()
}

/// The addresses a policy's members' keys control.
fn members_of(policy: Option<&MultisigPolicy>) -> BTreeSet<Address> {
    policy.into_iter().flat_map(|policy| policy.members.iter().map(|member| member.key.address())).collect()
}

fn decode(bytes: &[u8]) -> Result<MultisigPolicy, DbError> {
    wincode::deserialize(bytes).map_err(|err| DbError::Corrupt(format!("a multisig's policy would not decode: {err}")))
}

/// Moves the index as a block's layer changed the multisig registry: for each policy written, the
/// members it had below and has now. Written into the same layer, so it commits and is undone with
/// the block.
pub(super) fn follow(layer: &State, below: &dyn Read) -> Result<(), DbError> {
    for (_, policy_key, value) in layer.changed(&[Column::Accounts]) {
        let Some(multisig) = Multisigs::policy_account(&policy_key) else {
            continue;
        };
        let before = below.read(Column::Accounts, &policy_key)?.map(|bytes| decode(&bytes)).transpose()?;
        let after = value.map(|bytes| decode(&bytes)).transpose()?;
        let (was, now) = (members_of(before.as_ref()), members_of(after.as_ref()));

        for gone in was.difference(&now) {
            layer.delete(Column::Members, key(gone, &multisig));
        }

        for joined in now.difference(&was) {
            layer.put_value(Column::Members, key(joined, &multisig), &multisig);
        }
    }

    Ok(())
}

/// Builds the index whole from the registry in the database, in one atomic write that empties
/// what was there first and marks it kept: how a node starts keeping it on a chain already running.
pub(super) fn rebuild(db: &Arc<dyn Database>) -> Result<usize, DbError> {
    let prefix = Multisigs::policy_prefix();
    let mut policies: Vec<(Address, Vec<u8>)> = Vec::new();
    db.scan(Column::Accounts, prefix, prefix, &mut |key, value| {
        if let Some(multisig) = Multisigs::policy_account(key) {
            policies.push((multisig, value.to_vec()));
        }

        true
    })?;
    let state = State::new(db.clone());

    for (multisig, bytes) in &policies {
        for member in members_of(Some(&decode(bytes)?)) {
            state.put_value(Column::Members, key(&member, multisig), multisig);
        }
    }

    state.put_value(Column::Meta, KEPT_KEY.to_vec(), &Kept { version: SCHEMA });
    db.clear(Column::Members, state.take_changes())?;

    Ok(policies.len())
}

/// Stops keeping the index: empties it and forgets the mark, in one atomic write.
pub(super) fn forget(db: &Arc<dyn Database>) -> Result<(), DbError> {
    let state = State::new(db.clone());
    state.delete(Column::Meta, KEPT_KEY.to_vec());

    db.clear(Column::Members, state.take_changes())
}

#[document(
    summary = "A page of the multisigs an address's key is a member of, by the member index, read straight from a database, in the order of their addresses.",
    category = "Chain",
    params = {
        db = "The database.",
        member = "The address the member's key controls.",
        after = "The last multisig of the page before, to go on after it; None for the first page.",
        limit = "The most multisigs to return.",
    },
    returns = "The multisigs, and the last of them when there may be more, to go on from.",
    see_also = "Chain::keep_member_index",
)]
pub fn stored_multisigs_of(db: &dyn Committed, member: &Address, after: Option<&Address>, limit: usize) -> Result<(Vec<Address>, Option<Address>), DbError> {
    let prefix = address_key(member);
    // Just past the one the page before ended on.
    let start = after.map_or_else(|| prefix.clone(), |after| [key(member, after), vec![0]].concat());
    let mut found = Vec::new();
    let mut failed = None;
    db.scan(Column::Members, &prefix, &start, &mut |_, value| {
        match wincode::deserialize::<Address>(value) {
            Ok(multisig) => found.push(multisig),
            Err(err) => failed = Some(DbError::Corrupt(format!("an indexed multisig would not decode: {err}"))),
        }

        failed.is_none() && found.len() <= limit
    })?;

    if let Some(err) = failed {
        return Err(err);
    }

    let more = found.len() > limit;
    found.truncate(limit);
    let next = more.then(|| found.last().copied()).flatten();

    Ok((found, next))
}
