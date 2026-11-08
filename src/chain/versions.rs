//! The versions index: each contract's code as it was deployed and as each upgrade after replaced
//! it, oldest first. What GET /contracts/{address}/versions lists, for the explorer to show a
//! contract's earlier code beside what runs now. It points the way only: each version is the code
//! its transaction carried, and a light client proves that transaction and its receipt, which says
//! Deployed or Upgraded, itself.
//!
//! Every node keeps it, as it is one entry a deploy or upgrade. A node that had none when it
//! started fills it from its blocks' receipts in the background, newest first, as the other indexes
//! catch up; one that synced from a snapshot reaches back no further than the block after it.
//! Contracts that genesis placed, and clones a contract made, ran no Deploy, so they have none for
//! their first code. How it holds blocks is what every index shares, in the index module.

use {
    super::index::Landed,
    crate::chain::receipt::{TxReceipt, TxStatus},
    crate::crypto::hashable::Hashable,
    crate::crypto::hasher::Hasher,
    crate::db::{Column, Committed, DbError, State, address_key},
    crate::structs::address::Address,
    crate::structs::code::{Code, Engine},
    crate::structs::hash::Hash,
    crate::structs::receipt::StepEffect,
    crate::structs::tx::{SignedTx, Tx},
    r#macro::document,
    wincode::{SchemaRead, SchemaWrite},
};

#[document(
    summary = "One version of a contract's code: the transaction that deployed or upgraded it, where it landed, and what the code it carried was. What the versions index keeps under the contract, and GET /contracts/{address}/versions lists.",
    category = "Chain",
    see_also = "stored_versions, versions_in",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct ContractVersion {
    #[document(summary = "The transaction's id.")]
    pub tx: Hash,
    #[document(summary = "The block it landed in.")]
    pub block: Hash,
    #[document(summary = "That block's height.")]
    pub height: u64,
    #[document(summary = "Its place in the block.")]
    pub index: u32,
    #[document(summary = "The batch step that did it, for a batch; None for a transaction of its own.")]
    pub step: Option<u32>,
    #[document(summary = "Whether an upgrade replaced the code, rather than a deploy placing it.")]
    pub upgrade: bool,
    #[document(summary = "The engine it runs on.")]
    pub engine: Engine,
    #[document(summary = "The code as the transaction carried it (Lua source, a WebAssembly module, EVM init code), hashed by the chain's hash.")]
    pub code_hash: Hash,
    #[document(summary = "Its size in bytes, as carried.")]
    pub size: u64,
    #[document(summary = "What a WebAssembly module exports and declares, read from its sections; None for Lua, whose source says, and for EVM code.")]
    pub outline: Option<WasmOutline>,
}

impl Hashable for ContractVersion {}

#[document(
    summary = "What a WebAssembly module's sections say of it: the functions it exports that take and give nothing, which calls reach, and its chainv4.reentrant and chainv4.layout entries, as the explorer lists them beside the version after.",
    category = "Chain",
    see_also = "ContractVersion, wasm_outline",
)]
#[derive(Clone, Debug, Default, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct WasmOutline {
    #[document(summary = "The exported functions without parameters or results, sorted.")]
    pub functions: Vec<String>,
    #[document(summary = "The names its chainv4.reentrant sections give, sorted.")]
    pub reentrant: Vec<String>,
    #[document(summary = "The entries its chainv4.layout sections give, in order.")]
    pub layout: Vec<String>,
}

impl Hashable for WasmOutline {}

#[document(
    summary = "Reads a WebAssembly module's outline from its sections alone, with no engine: its type, import, function and export sections for which exports take and give nothing, and its chainv4 custom sections. For a module the chain took, which is valid.",
    params = { module = "The module's bytes." },
    returns = "The outline, or None for bytes that do not read as a module.",
    category = "Chain",
    see_also = "WasmOutline",
)]
pub fn wasm_outline(module: &[u8]) -> Option<WasmOutline> {
    let mut read = Reader { bytes: module.strip_prefix(b"\0asm\x01\0\0\0")? };
    let (mut types, mut imported, mut declared, mut exports) = (Vec::new(), 0u32, Vec::new(), Vec::new());
    let mut outline = WasmOutline::default();

    while !read.bytes.is_empty() {
        let id = read.byte()?;
        let size = read.leb()? as usize;
        let mut section = Reader { bytes: read.take(size)? };

        match id {
            // Types: each a function type, 0x60 with its parameters and results.
            1 => {
                for _ in 0..section.leb()? {
                    if section.byte()? != 0x60 {
                        return None;
                    }

                    let params = section.leb()? as usize;
                    section.take(params)?;
                    let results = section.leb()? as usize;
                    section.take(results)?;
                    types.push(params == 0 && results == 0);
                }
            }
            // Imports: functions imported come first among the functions' indices.
            2 => {
                for _ in 0..section.leb()? {
                    section.name()?;
                    section.name()?;

                    match section.byte()? {
                        0x00 => {
                            section.leb()?;
                            imported += 1;
                        }
                        0x01 => {
                            section.byte()?;
                            section.limits()?;
                        }
                        0x02 => section.limits()?,
                        0x03 => {
                            section.take(2)?;
                        }
                        _ => return None,
                    }
                }
            }
            // Functions: each defined one's type.
            3 => {
                for _ in 0..section.leb()? {
                    declared.push(section.leb()?);
                }
            }
            // Exports: a name, a kind and an index.
            7 => {
                for _ in 0..section.leb()? {
                    let name = section.name()?;
                    let kind = section.byte()?;
                    let index = section.leb()?;

                    if kind == 0x00 {
                        exports.push((name, index));
                    }
                }
            }
            0 => {
                let name = section.name()?;
                let entries = || section.bytes.strip_suffix(&[0]).map(|data| data.split(|byte| *byte == 0).map(|entry| String::from_utf8_lossy(entry).into_owned()).collect::<Vec<_>>());

                match name.as_str() {
                    "chainv4.reentrant" => outline.reentrant.extend(entries()?),
                    "chainv4.layout" => outline.layout.extend(entries()?),
                    _ => {}
                }
            }
            _ => {}
        }
    }

    for (name, index) in exports {
        let plain = index.checked_sub(imported).and_then(|defined| declared.get(defined as usize)).and_then(|ty| types.get(*ty as usize)).copied();

        if plain == Some(true) {
            outline.functions.push(name);
        }
    }

    outline.functions.sort();
    outline.reentrant.sort();
    outline.reentrant.dedup();

    Some(outline)
}

/// Bytes read from the front.
struct Reader<'a> {
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Option<&'a [u8]> {
        let (taken, rest) = self.bytes.split_at_checked(count)?;
        self.bytes = rest;

        Some(taken)
    }

    fn byte(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }

    /// An unsigned LEB128 number of at most 32 bits.
    fn leb(&mut self) -> Option<u32> {
        let mut value = 0u32;

        for shift in (0..35).step_by(7) {
            let byte = self.byte()?;
            value |= u32::from(byte & 0x7f).checked_shl(shift)?;

            if byte & 0x80 == 0 {
                return Some(value);
            }
        }

        None
    }

    fn name(&mut self) -> Option<String> {
        let len = self.leb()? as usize;

        Some(String::from_utf8_lossy(self.take(len)?).into_owned())
    }

    /// A table's or a memory's limits: a flag, the least, and the most when the flag says.
    fn limits(&mut self) -> Option<()> {
        let flag = self.byte()?;
        self.leb()?;

        if flag & 1 == 1 {
            self.leb()?;
        }

        Some(())
    }
}

#[document(
    summary = "The versions a transaction made, as the index keeps them: for each contract it deployed or upgraded, the step that did it, whether it was an upgrade, and the code it carried. A failed transaction made none.",
    params = {
        signed = "The transaction.",
        status = "What it did.",
    },
    category = "Chain",
    see_also = "ContractVersion",
)]
pub fn versions_in<'a>(signed: &'a SignedTx, status: &TxStatus) -> Vec<(Address, Option<u32>, bool, &'a Code)> {
    let runs = signed.tx.what_it_runs();
    let one = |address: Address, upgrade: bool| {
        runs.first().and_then(|tx| code_of(tx, upgrade)).map(|code| (address, None, upgrade, code))
    };

    match status {
        TxStatus::Deployed(address) => one(*address, false).into_iter().collect(),
        TxStatus::Upgraded(address) => one(*address, true).into_iter().collect(),
        TxStatus::Batch(steps) => steps
            .iter()
            .zip(runs)
            .enumerate()
            .filter_map(|(step, (outcome, tx))| {
                let (address, upgrade) = match outcome.effect {
                    StepEffect::Deployed(address) => (address, false),
                    StepEffect::Upgraded(address) => (address, true),
                    _ => return None,
                };

                code_of(tx, upgrade).map(|code| (address, Some(step as u32), upgrade, code))
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// The code a deploy or an upgrade carried.
fn code_of(tx: &Tx, upgrade: bool) -> Option<&Code> {
    match (tx, upgrade) {
        (Tx::Deploy { code }, false) | (Tx::Upgrade { code, .. }, true) => Some(code),
        _ => None,
    }
}

/// Where a version sits under its contract, oldest first: the height, the place and the step.
fn key(contract: &Address, height: u64, index: u32, step: Option<u32>) -> Vec<u8> {
    [&address_key(contract)[..], &height.to_be_bytes(), &index.to_be_bytes(), &step.unwrap_or(0).to_be_bytes()].concat()
}

/// Indexes the versions one transaction made, in the layer its block commits.
pub(super) fn index(layer: &State, landed: Landed, signed: &SignedTx, receipt: &TxReceipt) {
    let Landed { block, height, at: index, tx } = landed;

    for (contract, step, upgrade, code) in versions_in(signed, &receipt.status) {
        let version = ContractVersion {
            tx,
            block,
            height,
            index,
            step,
            upgrade,
            engine: code.engine(),
            code_hash: Hasher::digest(tx.algorithm(), code.bytes()),
            size: code.bytes().len() as u64,
            outline: match code {
                Code::Wasm(module) => wasm_outline(module),
                Code::Lua(_) | Code::Evm(_) => None,
            },
        };
        layer.put_value(Column::Versions, key(&contract, height, index, step), &version);
    }
}

/// Takes one transaction's versions back out.
pub(super) fn unindex(layer: &State, height: u64, index: u32, signed: &SignedTx, receipt: &TxReceipt) {
    for (contract, step, ..) in versions_in(signed, &receipt.status) {
        layer.delete(Column::Versions, key(&contract, height, index, step));
    }
}

#[document(
    summary = "A page of a contract's versions, oldest first, read straight from a database.",
    category = "Chain",
    params = {
        db = "The database.",
        contract = "Whose.",
        cursor = "Where the last page stopped, as it returned; None for the first.",
        limit = "The most versions to return.",
    },
    returns = "The versions, and the cursor for the next page, or None after the last.",
    see_also = "ContractVersion, stored_index",
)]
pub fn stored_versions(db: &dyn Committed, contract: &Address, cursor: Option<&[u8]>, limit: usize) -> Result<(Vec<ContractVersion>, Option<Vec<u8>>), DbError> {
    let prefix = address_key(contract);
    let mut found: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    db.scan(Column::Versions, &prefix, cursor.unwrap_or(&prefix), &mut |key, value| {
        found.push((key.to_vec(), value.to_vec()));

        found.len() <= limit
    })?;
    let next = (found.len() > limit).then(|| found.remove(limit).0);

    let versions = found
        .into_iter()
        .map(|(_, value)| wincode::deserialize::<ContractVersion>(&value).map_err(|err| DbError::Corrupt(format!("an indexed version would not decode: {err}"))))
        .collect::<Result<_, _>>()?;

    Ok((versions, next))
}
