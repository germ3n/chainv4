//! Who a transaction touches, as a block read here can tell: what a node's address index lists under
//! an address, and what its event index does, a token's transfer in an EVM log's topics included.
//! chain::addresses is the node's own, and not in an app's build; this names every kind of transaction,
//! with no catch-all, so that a new one has to be decided here too before it builds.

use {
    chainv4::crypto::ethereum,
    chainv4::registry::names::NameAction,
    chainv4::structs::address::Address,
    chainv4::structs::event::Event,
    chainv4::structs::receipt::CommittedReceipt,
    chainv4::structs::tx::{SignedTx, Tx, TxCallArgumentType},
    std::collections::{BTreeMap, BTreeSet},
};

/// A wallet's addresses, as the chain writes them and as the EVM does.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ours {
    addresses: BTreeSet<Address>,
    /// The 20 bytes each goes by in the EVM, and in the form an EVM move names its parties.
    evm: BTreeMap<[u8; 20], Address>,
}

impl Ours {
    pub fn of(addresses: &[Address]) -> Ours {
        Ours {
            addresses: addresses.iter().copied().collect(),
            evm: addresses.iter().map(|address| (chainv4::abi::evm_address(address), *address)).collect(),
        }
    }

    pub fn len(&self) -> usize {
        self.addresses.len()
    }

    pub fn is_empty(&self) -> bool {
        self.addresses.is_empty()
    }

    /// Whether a move's party, as text, is one of these.
    pub fn is_ours(&self, party: &str) -> bool {
        party.parse::<Address>().is_ok_and(|address| self.addresses.contains(&address))
    }

    fn holds(&self, address: &Address) -> bool {
        self.addresses.contains(address)
    }
}

/// Whether a transaction touches one of these: it sent it, or named it, or what ran paid it or
/// emitted an event that mentions it.
pub fn touches(signed: &SignedTx, receipt: &CommittedReceipt, ours: &Ours) -> bool {
    parties(signed, Some(receipt)).iter().any(|address| ours.holds(address)) || receipt.events.iter().any(|event| mentions(event, ours))
}

/// Who a transaction names or paid, as an address index lists it under each: who sent it, who signed
/// for it, who it is for, and, with its receipt, who what ran paid.
pub fn parties(signed: &SignedTx, receipt: Option<&CommittedReceipt>) -> BTreeSet<Address> {
    let mut touched = BTreeSet::from([signed.sender()]);

    if let Tx::Multisig { signatures, .. } = &signed.tx {
        touched.extend(signatures.iter().map(|signed| signed.key.address()));
    }

    for tx in signed.tx.what_it_runs() {
        named(tx, &mut touched);
    }

    touched.extend(receipt.into_iter().flat_map(|receipt| &receipt.payments).flat_map(|payment| [payment.from, payment.to]));

    touched
}

/// Who an event names: the addresses in what it holds, and the 20 bytes each address an EVM log's
/// topics hold goes by in the EVM, padded.
pub fn mentioned(event: &Event) -> (Vec<Address>, Vec<[u8; 20]>) {
    let held = event.data.iter().filter_map(|value| match value {
        TxCallArgumentType::Address(address) => Some(*address),
        _ => None,
    });
    let topics = event.evm_log().map_or_else(Vec::new, |(topics, _)| {
        topics
            .iter()
            .filter_map(|topic| {
                let (padding, held) = topic.split_at(12);

                padding.iter().all(|byte| *byte == 0).then(|| <[u8; 20]>::try_from(held).ok()).flatten()
            })
            .collect()
    });

    (held.collect(), topics)
}

/// Whether an event names one of these: in what it holds, or an EVM log's topics, which hold an
/// address as its 20 bytes, padded.
fn mentions(event: &Event, ours: &Ours) -> bool {
    let (held, topics) = mentioned(event);

    held.iter().any(|address| ours.holds(address)) || topics.iter().any(|held| ours.evm.contains_key(held))
}

/// Everyone one transaction names, as the node lists it under each.
fn named(tx: &Tx, touched: &mut BTreeSet<Address>) {
    match tx {
        Tx::Transfer { from, to, .. } => touched.extend([*from, *to]),
        Tx::Mint { to, .. } | Tx::Claim { to, .. } | Tx::HandDirectory { to: Some(to) } => {
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
        Tx::Remove { contract, beneficiary } => {
            touched.extend([*contract, *beneficiary]);
        }
        Tx::EvmCall { to, .. } => {
            touched.insert(*to);
        }
        Tx::Ethereum { raw } => {
            if let Some(to) = ethereum::decode(raw).ok().and_then(|ethereum| ethereum.to) {
                touched.extend([Address::Evm(to), Address::Secp256k1(to)]);
            }
        }
        Tx::Name { action, .. } => {
            if let NameAction::Transfer { to } | NameAction::Subname { owner: to, .. } | NameAction::Target { target: Some(to) } = action {
                touched.insert(*to);
            }
        }
        Tx::CreateMultisig { members, .. } | Tx::ChangeMultisig { members, .. } => touched.extend(members.iter().map(|member| member.key.address())),
        Tx::ApproveMultisig { account, .. } | Tx::WithdrawMultisig { account, .. } => {
            touched.insert(*account);
        }
        Tx::Deploy { .. }
        | Tx::Bond { .. }
        | Tx::Unbond { .. }
        | Tx::Withdraw { .. }
        | Tx::SetAttester { .. }
        | Tx::Slash { .. }
        | Tx::ApproveSigners { .. }
        | Tx::WithdrawApproval { .. }
        | Tx::VoteChange { .. }
        | Tx::ClearVote { .. }
        | Tx::SetDirectory { .. }
        | Tx::HandDirectory { to: None }
        | Tx::PrimaryName { .. }
        | Tx::Anchor { .. }
        // Read past above: what_it_runs gives what these do, never one of these.
        | Tx::Multisig { .. }
        | Tx::Batch { .. } => {}
    }
}
