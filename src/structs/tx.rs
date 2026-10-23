use {
    crate::structs::address::Address,
    crate::structs::amount::Amount,
    r#macro::document,
};

#[document(
    summary = "A state change submitted to the chain.",
    category = "Transactions",
    see_also = "Address, Amount",
)]
pub enum Tx {
    #[document(
        summary = "Moves an amount from one address to another.",
        params = {
            from = "The address the amount is taken from.",
            to = "The address the amount is credited to.",
            amount = "How much to move.",
        },
    )]
    Transfer {
        from: Address,
        to: Address,
        amount: Amount,
    },
    #[document(
        summary = "Creates new supply and credits it to an address.",
        params = {
            to = "The address that receives the new supply.",
            amount = "How much to create.",
        },
        see_also = "Tx::Burn",
    )]
    Mint {
        to: Address,
        amount: Amount,
    },
    #[document(
        summary = "Destroys supply held by an address.",
        params = {
            from = "The address the supply is taken from.",
            amount = "How much to destroy.",
        },
    )]
    Burn {
        from: Address,
        amount: Amount,
    },
}