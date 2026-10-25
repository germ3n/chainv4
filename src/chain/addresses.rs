//! The address index: every transaction on the main chain, findable by each address it touches.
//! What a wallet lists an address's history from, plain coin transfers to it included, which no
//! event says anything of. It points the way only: a light client proves each transaction and its
//! receipt itself.
//!
//! A transaction touches its sender; the parties of a transfer, mint or burn; the contract it
//! calls or upgrades, and every address among the call's arguments, as transfer(to, amount) names
//! its recipient; and the contract it deploys. Each address keeps its own record of it, newest
//! first, so a listing reads forward from where the last page stopped.
//!
//! A node keeps it only when started with --index-addresses, and once on it stays on until
//! --index-addresses=off drops it. How it holds blocks, and catches up, is what every index
//! shares, in the index module.

use {
    super::index::Landed,
    crate::chain::receipt::{TxReceipt, TxStatus},
    crate::crypto::hashable::Hashable,
    crate::db::{Column, Database, DbError, State, address_key},
    crate::structs::address::Address,
    crate::structs::hash::Hash,
    crate::structs::tx::{SignedTx, Tx, TxCallArgumentType},
    r#macro::document,
    std::collections::BTreeSet,
    wincode::{SchemaRead, SchemaWrite},
};

#[document(
    summary = "A transaction, with where it landed: what the address index keeps under each address it touches, and GET /accounts/{address}/txs lists.",
    category = "Chain",
    see_also = "stored_address_txs, touched",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct IndexedTx {
    #[document(summary = "The transaction's id.")]
    pub tx: Hash,
    #[document(summary = "The block it landed in.")]
    pub block: Hash,
    #[document(summary = "That block's height.")]
    pub height: u64,
    #[document(summary = "Its place in the block.")]
    pub index: u32,
}

impl Hashable for IndexedTx {}

#[document(
    summary = "Every address a transaction touches, as the address index keeps it under each: its sender; a transfer's, mint's or burn's parties; the contract called or upgraded and every address among the call's arguments; and the contract it deployed.",
    params = {
        signed = "The transaction.",
        status = "What it did, for the address a deploy landed at.",
    },
    category = "Chain",
    see_also = "IndexedTx",
)]
pub fn touched(signed: &SignedTx, status: &TxStatus) -> BTreeSet<Address> {
    let mut touched = BTreeSet::from([signed.sender()]);

    match &signed.tx {
        Tx::Transfer { from, to, .. } => touched.extend([*from, *to]),
        Tx::Mint { to, .. } => {
            touched.insert(*to);
        }
        Tx::Burn { from, .. } => {
            touched.insert(*from);
        }
        Tx::ContractCall { to, data, .. } => {
            touched.insert(*to);
            touched.extend(data.iter().filter_map(|argument| match argument {
                TxCallArgumentType::Address(address) => Some(*address),
                _ => None,
            }));
        }
        Tx::Upgrade { contract, .. } => {
            touched.insert(*contract);
        }
        Tx::Deploy { .. }
        | Tx::Bond { .. }
        | Tx::Unbond { .. }
        | Tx::Withdraw { .. }
        | Tx::Slash { .. }
        | Tx::ApproveSigners { .. }
        | Tx::WithdrawApproval { .. } => {}
    }

    if let TxStatus::Deployed(contract) = status {
        touched.insert(*contract);
    }

    touched
}

/// Where a transaction sits under an address, newest first: the height and place inverted.
fn key(address: &Address, height: u64, index: u32) -> Vec<u8> {
    [&address_key(address)[..], &(u64::MAX - height).to_be_bytes(), &(u32::MAX - index).to_be_bytes()].concat()
}

/// Indexes one transaction under every address it touches, in the layer its block commits.
pub(super) fn index(layer: &State, landed: Landed, signed: &SignedTx, receipt: &TxReceipt) {
    let Landed { block, height, at: index, tx } = landed;

    for address in touched(signed, &receipt.status) {
        layer.put_value(Column::Addresses, key(&address, height, index), &IndexedTx { tx, block, height, index });
    }
}

/// Takes one transaction back out.
pub(super) fn unindex(layer: &State, height: u64, index: u32, signed: &SignedTx, receipt: &TxReceipt) {
    for address in touched(signed, &receipt.status) {
        layer.delete(Column::Addresses, key(&address, height, index));
    }
}

#[document(
    summary = "A page of the transactions touching an address, newest first, read straight from a database.",
    category = "Chain",
    params = {
        db = "The database.",
        address = "Whose.",
        cursor = "Where the last page stopped, as it returned; None for the newest.",
        limit = "The most transactions to return.",
    },
    returns = "The transactions, and the cursor for the next page, or None after the last.",
    see_also = "stored_index, IndexedTx",
)]
pub fn stored_address_txs(db: &dyn Database, address: &Address, cursor: Option<&[u8]>, limit: usize) -> Result<(Vec<IndexedTx>, Option<Vec<u8>>), DbError> {
    let prefix = address_key(address);
    let mut found: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    db.scan(Column::Addresses, &prefix, cursor.unwrap_or(&prefix), &mut |key, value| {
        found.push((key.to_vec(), value.to_vec()));

        found.len() <= limit
    })?;
    let next = (found.len() > limit).then(|| found.remove(limit).0);

    let txs = found
        .into_iter()
        .map(|(_, value)| wincode::deserialize::<IndexedTx>(&value).map_err(|err| DbError::Corrupt(format!("an indexed transaction would not decode: {err}"))))
        .collect::<Result<_, _>>()?;

    Ok((txs, next))
}
