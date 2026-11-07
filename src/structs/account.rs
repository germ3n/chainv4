use {
    crate::crypto::hashable::Hashable,
    crate::structs::u256::U256Le,
    crate::structs::u256::u256,
    r#macro::document,
    wincode::SchemaRead,
    wincode::SchemaWrite,
};

#[document(
    summary = "What the chain knows about an address. An address that was never written to has the default: nonce 0 and nothing in it.",
    category = "Primitives",
    see_also = "Accounts, Address",
)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct Account {
    #[document(summary = "How many transactions the address has sent. Each one must carry the next value, so none can be replayed.")]
    pub nonce: u64,
    #[document(summary = "The balance.")]
    #[wincode(with = "U256Le")]
    pub balance: u256,
}

impl Hashable for Account {}

/// The prefix an EIP-7702 delegation's key starts with, in Column::Storage.
const DELEGATION: &[u8] = b"evm delegation";

#[r#macro::document(
    summary = "Where an account's EIP-7702 delegation is kept, in Column::Storage, by its 20 bytes: what a light client proves to say whose code the account runs. Here, apart from the VM, so the apps' web builds prove it too.",
    category = "VM",
    see_also = "delegation, delegated_to",
)]
pub fn delegation_key(bytes: &[u8; 20]) -> Vec<u8> {
    [DELEGATION, bytes].concat()
}
