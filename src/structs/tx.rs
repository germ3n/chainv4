use {
    crate::crypto::hashable::Hashable,
    crate::crypto::verifiable::Verifiable,
    crate::crypto::verifiable::VerifyError,
    crate::structs::address::Address,
    crate::structs::amount::Amount,
    crate::structs::hash::Hash,
    crate::structs::u256::U256Le,
    crate::structs::u256::u256,
    r#macro::document,
    wincode::SchemaRead,
    wincode::SchemaWrite,
};

#[document(
    summary = "A state change submitted to the chain.",
    category = "Transactions",
    see_also = "Address, Amount",
)]
#[derive(Clone, Debug, PartialEq, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
pub enum Tx {
    #[document(
        summary = "Moves an amount from one address to another.",
        params = {
            from = "The address the amount is taken from.",
            to = "The address the amount is credited to.",
            amount = "How much to move.",
        },
    )]
    #[wincode(tag = 0)]
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
    #[wincode(tag = 1)]
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
    #[wincode(tag = 2)]
    Burn {
        from: Address,
        amount: Amount,
    },
    #[document(
        summary = "Calls a function on a contract.",
        params = {
            to = "The address of the contract to call.",
            func = "Which function to call: the XXH3-64 of its name, from Hasher::digest.",
            amount = "How much to send to the contract.",
            data = "The data to send to the contract.",
        },
        see_also = "Hasher::digest, TxCallArgumentType",
    )]
    #[wincode(tag = 3)]
    ContractCall {
        to: Address,
        func: Hash,
        amount: Amount,
        data: Vec<TxCallArgumentType>,
    },
}

#[document(
    summary = "The type of an argument to a contract call.",
    category = "Transactions",
    see_also = "Tx::ContractCall",
)]
#[derive(Clone, Debug, PartialEq, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
pub enum TxCallArgumentType {
    #[document(summary = "An address.")]
    #[wincode(tag = 0)]
    Address(Address),
    #[document(summary = "An amount.")]
    #[wincode(tag = 1)]
    Amount(Amount),
    #[document(summary = "A vector of bytes.")]
    #[wincode(tag = 2)]
    Bytes(Vec<u8>),
    #[document(summary = "A boolean.")]
    #[wincode(tag = 3)]
    Bool(bool),
    #[document(summary = "A string.")]
    #[wincode(tag = 4)]
    String(String),
    #[document(summary = "A number.")]
    #[wincode(tag = 5)]
    Number(f64),
    #[document(summary = "A signed 64-bit integer.")]
    #[wincode(tag = 6)]
    I64(i64),
    #[document(summary = "An unsigned 64-bit integer.")]
    #[wincode(tag = 7)]
    U64(u64),
    #[document(summary = "A signed 128-bit integer.")]
    #[wincode(tag = 8)]
    I128(i128),
    #[document(summary = "An unsigned 128-bit integer.")]
    #[wincode(tag = 9)]
    U128(u128),
    #[document(summary = "An unsigned 256-bit integer.")]
    #[wincode(tag = 10)]
    U256(#[wincode(with = "U256Le")] u256),
}

impl Verifiable for Tx {
    #[document(
        parent = "Tx",
        summary = "Checks every address and argument inside, that a transfer, mint or burn moves more than zero, and that a contract call names its function by XXH3-64. A contract call may send zero.",
        see_also = "VerifyError, Hasher::digest",
    )]
    fn verify(&self) -> Result<(), VerifyError> {
        match self {
            Tx::Transfer { from, to, amount } => {
                from.verify()?;
                to.verify()?;
                amount.verify()?;

                nonzero(amount)
            }
            Tx::Mint { to, amount } => {
                to.verify()?;
                amount.verify()?;

                nonzero(amount)
            }
            Tx::Burn { from, amount } => {
                from.verify()?;
                amount.verify()?;

                nonzero(amount)
            }
            Tx::ContractCall {
                to,
                func,
                amount,
                data,
            } => {
                to.verify()?;
                func.verify()?;
                amount.verify()?;
                function_hash(func)?;

                for argument in data {
                    argument.verify()?;
                }

                Ok(())
            }
        }
    }
}

fn nonzero(amount: &Amount) -> Result<(), VerifyError> {
    match amount {
        Amount::U128(0) => Err(VerifyError::ZeroAmount),
        Amount::U256(value) if *value == u256::ZERO => Err(VerifyError::ZeroAmount),
        Amount::U128(_) | Amount::U256(_) => Ok(()),
    }
}

/// Functions are named by the XXH3-64 of their name, so every client computes the same hash. A
/// build without the xxh3 feature cannot check that and rejects every contract call.
fn function_hash(func: &Hash) -> Result<(), VerifyError> {
    match func {
        #[cfg(feature = "xxh3")]
        Hash::Xxh3_64(_) => Ok(()),
        #[allow(unreachable_patterns)]
        other => Err(VerifyError::FunctionHash(other.algorithm())),
    }
}

impl Verifiable for TxCallArgumentType {
    #[document(
        parent = "TxCallArgumentType",
        summary = "Checks address arguments, and that number arguments are finite: NaN has many encodings, which would let the same call go out under different hashes.",
        see_also = "VerifyError::NonFiniteNumber",
    )]
    fn verify(&self) -> Result<(), VerifyError> {
        // Every variant is listed, so a new one has to decide its own rule.
        match self {
            TxCallArgumentType::Address(address) => address.verify(),
            TxCallArgumentType::Amount(amount) => amount.verify(),
            TxCallArgumentType::Number(number) if !number.is_finite() => {
                Err(VerifyError::NonFiniteNumber)
            }
            TxCallArgumentType::Number(_)
            | TxCallArgumentType::Bytes(_)
            | TxCallArgumentType::Bool(_)
            | TxCallArgumentType::String(_)
            | TxCallArgumentType::I64(_)
            | TxCallArgumentType::U64(_)
            | TxCallArgumentType::I128(_)
            | TxCallArgumentType::U128(_)
            | TxCallArgumentType::U256(_) => Ok(()),
        }
    }
}

impl Hashable for Tx {
    const TAG_LEN: usize = 1;
}

impl Hashable for TxCallArgumentType {
    const TAG_LEN: usize = 1;
}
