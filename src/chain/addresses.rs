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
    crate::structs::receipt::Payment,
    super::index::Landed,
    crate::chain::receipt::{TxReceipt, TxStatus},
    crate::crypto::hashable::Hashable,
    crate::db::{Column, Committed, DbError, State, address_key},
    crate::structs::address::Address,
    crate::structs::hash::Hash,
    crate::registry::names::NameAction,
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
    summary = "Every address a transaction touches, as the address index keeps it under each: its sender; a transfer's, mint's or burn's parties; the contract called or upgraded and every address among the call's arguments; the contract it deployed; and every contract that paid coin while it ran, and whom it paid.",
    params = {
        signed = "The transaction.",
        status = "What it did, for the address a deploy landed at.",
        payments = "What its contracts paid, from its receipt.",
    },
    category = "Chain",
    see_also = "IndexedTx",
)]
pub fn touched(signed: &SignedTx, status: &TxStatus, payments: &[Payment]) -> BTreeSet<Address> {
    let mut touched = BTreeSet::from([signed.sender()]);

    // A multisig's own transaction is listed as what it does, and under the members who signed it.
    let tx = match &signed.tx {
        Tx::Multisig { tx, signatures, .. } => {
            touched.extend(signatures.iter().map(|signed| signed.key.address()));

            tx
        }
        tx => tx,
    };

    match tx {
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
        Tx::EvmCall { to, .. } => {
            touched.insert(*to);
        }
        // Whom it pays may be an EVM contract or an Ethereum account: listed under both.
        Tx::Ethereum { raw } => {
            if let Some(to) = crate::crypto::ethereum::decode(raw).ok().and_then(|ethereum| ethereum.to) {
                touched.extend([Address::Evm(to), Address::Secp256k1(to)]);
            }
        }
        // A name given or pointed at another account shows in that account's history too.
        Tx::Name { action, .. } => match action {
            NameAction::Transfer { to } | NameAction::Subname { owner: to, .. } | NameAction::Target { target: Some(to) } => {
                touched.insert(*to);
            }
            _ => {}
        },
        Tx::Deploy { .. }
        | Tx::Bond { .. }
        | Tx::Unbond { .. }
        | Tx::Withdraw { .. }
        | Tx::Slash { .. }
        | Tx::ApproveSigners { .. }
        | Tx::WithdrawApproval { .. }
        | Tx::ApproveChange { .. }
        | Tx::WithdrawChange { .. }
        | Tx::SetDirectory { .. }
        | Tx::HandDirectory { to: None }
        | Tx::PrimaryName { .. } => {}
        // The new admin sees the role handed to it.
        Tx::HandDirectory { to: Some(to) } => {
            touched.insert(*to);
        }
        // A new multisig, or a change of one, shows in its members' histories; an approval in
        // the multisig's.
        Tx::CreateMultisig { members, .. } | Tx::ChangeMultisig { members, .. } => touched.extend(members.iter().map(|member| member.key.address())),
        Tx::ApproveMultisig { account, .. } | Tx::WithdrawMultisig { account, .. } => {
            touched.insert(*account);
        }
        // A claim shows in the miner's history.
        Tx::Claim { to, .. } => {
            touched.insert(*to);
        }
        // Read past above: never what a multisig sends.
        Tx::Multisig { .. } => {}
    }

    if let TxStatus::Deployed(contract) = status {
        touched.insert(*contract);
    }

    // What a contract paid shows in the history of whom it paid, and its own.
    touched.extend(payments.iter().flat_map(|payment| [payment.from, payment.to]));

    touched
}

/// How many index entries a scan reads at a time, between looking through what it read.
const SCAN_CHUNK: usize = 256;

/// Where a transaction sits under an address, newest first: the height and place inverted.
fn key(address: &Address, height: u64, index: u32) -> Vec<u8> {
    [&address_key(address)[..], &(u64::MAX - height).to_be_bytes(), &(u32::MAX - index).to_be_bytes()].concat()
}

/// Indexes one transaction under every address it touches, in the layer its block commits.
pub(super) fn index(layer: &State, landed: Landed, signed: &SignedTx, receipt: &TxReceipt) {
    let Landed { block, height, at: index, tx } = landed;

    for address in touched(signed, &receipt.status, &receipt.payments) {
        layer.put_value(Column::Addresses, key(&address, height, index), &IndexedTx { tx, block, height, index });
    }
}

/// Takes one transaction back out.
pub(super) fn unindex(layer: &State, height: u64, index: u32, signed: &SignedTx, receipt: &TxReceipt) {
    for address in touched(signed, &receipt.status, &receipt.payments) {
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
pub fn stored_address_txs(db: &dyn Committed, address: &Address, cursor: Option<&[u8]>, limit: usize) -> Result<(Vec<IndexedTx>, Option<Vec<u8>>), DbError> {
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

#[document(
    summary = "What a scan of the address index came to: what it kept, how far it got, and whether it stopped at its bound rather than at a full page or the end.",
    category = "Chain",
    see_also = "scan_address_txs",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddressScan {
    #[document(summary = "The transactions kept, newest first.")]
    pub found: Vec<IndexedTx>,
    #[document(summary = "Where to go on from, as a cursor; None once there is nothing further.")]
    pub next: Option<Vec<u8>>,
    #[document(summary = "How many entries it looked at.")]
    pub scanned: usize,
    #[document(summary = "Whether it stopped at its bound with the page not full: more may match from next on.")]
    pub limited: bool,
}

#[document(
    summary = "The cursor that starts a listing of an address's transactions at a height, the newest of that height first: what a seek to a height is, as the index is by height.",
    category = "Chain",
    see_also = "scan_address_txs",
)]
pub fn address_txs_from(address: &Address, height: u64) -> Vec<u8> {
    key(address, height, u32::MAX)
}

#[document(
    summary = "A page of the transactions touching an address that pass a test, newest first, read straight from a database, looking at no more than a bound of entries: a page may hold fewer than its limit, with a cursor that goes on from where it stopped.",
    category = "Chain",
    params = {
        db = "The database.",
        address = "Whose.",
        start = "Where to start, as a cursor; None for the newest.",
        lowest = "The lowest height to look at: the scan ends there.",
        limit = "The most transactions to keep.",
        bound = "The most entries to look at.",
        keep = "Whether to keep an entry.",
    },
    returns = "What it kept, where to go on from, and how far it looked.",
    see_also = "stored_address_txs, AddressScan",
)]
pub fn scan_address_txs(
    db: &dyn Committed,
    address: &Address,
    start: Option<&[u8]>,
    lowest: u64,
    limit: usize,
    bound: usize,
    keep: &mut dyn FnMut(&IndexedTx) -> Result<bool, DbError>,
) -> Result<AddressScan, DbError> {
    let prefix = address_key(address);
    let mut at = start.map_or_else(|| prefix.clone(), <[u8]>::to_vec);
    let mut found = Vec::new();
    let mut scanned = 0;

    loop {
        // One more than it looks through, which says whether there is more, and where it is.
        let wanted = (bound - scanned).min(SCAN_CHUNK) + 1;
        let mut chunk: Vec<(Vec<u8>, Vec<u8>)> = Vec::with_capacity(wanted);
        db.scan(Column::Addresses, &prefix, &at, &mut |key, value| {
            chunk.push((key.to_vec(), value.to_vec()));

            chunk.len() < wanted
        })?;
        let beyond = (chunk.len() == wanted).then(|| chunk.pop().map(|(key, _)| key)).flatten();

        for (key, value) in chunk {
            if found.len() == limit {
                return Ok(AddressScan { found, next: Some(key), scanned, limited: false });
            }

            let indexed: IndexedTx = wincode::deserialize(&value).map_err(|err| DbError::Corrupt(format!("an indexed transaction would not decode: {err}")))?;

            if indexed.height < lowest {
                return Ok(AddressScan { found, next: None, scanned, limited: false });
            }

            scanned += 1;

            if keep(&indexed)? {
                found.push(indexed);
            }
        }

        match beyond {
            None => return Ok(AddressScan { found, next: None, scanned, limited: false }),
            Some(key) if found.len() == limit => return Ok(AddressScan { found, next: Some(key), scanned, limited: false }),
            Some(key) if scanned >= bound => return Ok(AddressScan { found, next: Some(key), scanned, limited: true }),
            Some(key) => at = key,
        }
    }
}
