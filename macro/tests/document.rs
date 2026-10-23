use r#macro::document;
use std::borrow::Cow;

#[document(summary = "A state change.", category = "Transactions")]
#[derive(Debug, PartialEq)]
pub enum Tx {
    #[document(summary = "Moves an amount.", params = { from = "The sender." })]
    Transfer { from: u8, to: u8, amount: u128 },
    #[document(summary = "Raw bytes.")]
    Raw(Vec<u8>),
    Empty,
}

#[document(name = "Account", summary = "An account.")]
pub struct AccountState {
    #[document(summary = "Spendable balance.")]
    pub balance: u128,
}

#[document(summary = "Something that can be hashed.")]
pub trait Hashable {
    #[document(parent = "Hashable", summary = "The bytes that get hashed.")]
    fn hashable_bytes(&self) -> Cow<'_, [u8]>;
}

impl Hashable for AccountState {
    #[document(parent = "AccountState", summary = "Little-endian balance.")]
    fn hashable_bytes(&self) -> Cow<'_, [u8]> {
        Cow::Owned(self.balance.to_le_bytes().to_vec())
    }
}

#[document(summary = "Blocks per epoch.")]
pub const EPOCH: u64 = 432;

#[document(lua, parent = "chain", name = "epoch", summary = "Blocks per epoch.", returns = { ty = "number", desc = "The length." })]
fn lua_chain_epoch() {}

#[test]
fn documented_items_still_work() {
    let tx = Tx::Transfer {
        from: 1,
        to: 2,
        amount: 3,
    };
    let state = AccountState { balance: 258 };

    assert_ne!(tx, Tx::Empty);
    assert_ne!(Tx::Raw(vec![1]), Tx::Empty);
    assert_eq!(state.hashable_bytes()[..2], [2, 1]);
    assert_eq!(EPOCH, 432);
}
