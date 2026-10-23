use {
    crate::crypto::verifiable::{Verifiable, VerifyError},
    crate::db::{DbError, State},
    crate::registry::{Accounts, Contracts, RegistryError},
    crate::structs::address::Address,
    crate::structs::hash::Hash,
    crate::structs::tx::{SignedTx, Tx, TxCallArgumentType},
    crate::vm::lvm::{CallContext, Limits, LvmError},
    r#macro::document,
    std::fmt,
    std::sync::Arc,
};

#[document(
    summary = "The chain-wide rules a transaction runs under, from genesis.",
    category = "Chain",
    see_also = "Genesis::rules",
)]
#[derive(Clone, Copy, Debug)]
pub struct Rules {
    #[document(summary = "The one address allowed to mint, if any.")]
    pub minter: Option<Address>,
    #[document(summary = "What one deploy or contract call may use.")]
    pub limits: Limits,
}

#[document(
    summary = "What a transaction that ran did.",
    category = "Chain",
    see_also = "Receipt",
)]
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    #[document(summary = "A transfer, mint or burn went through.")]
    Done,
    #[document(summary = "A contract was deployed at this address.")]
    Deployed(Address),
    #[document(summary = "A contract call returned these values.")]
    Returned(Vec<TxCallArgumentType>),
}

#[document(
    summary = "Why a transaction that ran failed. It still counts: it stays in its block and uses up its nonce, but nothing else it did is kept.",
    category = "Chain",
    see_also = "Receipt",
)]
#[derive(Clone, Debug, PartialEq)]
pub enum Failure {
    #[document(summary = "Moving funds or deploying failed, for example for lack of balance.")]
    Registry(RegistryError),
    #[document(summary = "The contract failed.")]
    Contract(LvmError),
    #[document(summary = "A mint from an address other than the minter.")]
    NotMinter,
}

#[document(
    summary = "What happened to one transaction in a block.",
    category = "Chain",
)]
#[derive(Clone, Debug, PartialEq)]
pub struct Receipt {
    #[document(summary = "The transaction's id.")]
    pub id: Hash,
    #[document(summary = "Fuel a contract call burned; 0 for everything else.")]
    pub fuel_used: u64,
    #[document(summary = "What it did, or why it failed.")]
    pub result: Result<Effect, Failure>,
}

#[document(
    summary = "Why a transaction could not run at all. A block holding one is invalid.",
    category = "Chain",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TxError {
    #[document(summary = "The transaction, its key or its signature is not valid.")]
    Invalid(VerifyError),
    #[document(summary = "The nonce is not the sender's next one.")]
    WrongNonce { expected: u64, got: u64 },
}

impl fmt::Display for TxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TxError::Invalid(err) => write!(f, "{err}"),
            TxError::WrongNonce { expected, got } => {
                write!(f, "nonce {got}, but the sender's next is {expected}")
            }
        }
    }
}

/// What can stop one transaction: it is invalid, or the node itself failed.
#[derive(Debug)]
pub(crate) enum ExecuteError {
    Invalid(TxError),
    Db(DbError),
}

impl From<DbError> for ExecuteError {
    fn from(err: DbError) -> ExecuteError {
        ExecuteError::Db(err)
    }
}

/// Runs one transaction on top of its block's state. The nonce is used up in the block's layer
/// either way; everything else happens in a layer of its own that is kept only on success.
pub(crate) fn execute(block: &Arc<State>, tx: &SignedTx, rules: &Rules) -> Result<Receipt, ExecuteError> {
    tx.verify().map_err(|err| ExecuteError::Invalid(TxError::Invalid(err)))?;
    let sender = tx.sender();
    let accounts = Accounts::new(block);
    let expected = accounts.get(&sender)?.nonce;

    if tx.nonce != expected {
        return Err(ExecuteError::Invalid(TxError::WrongNonce {
            expected,
            got: tx.nonce,
        }));
    }

    accounts.bump_nonce(&sender).map_err(node_fault)?;
    let layer = Arc::new(State::new(block.clone()));
    let (fuel_used, result) = run(&layer, tx, sender, rules);

    // A failing database is the node's fault, never the transaction's.
    match &result {
        Err(Failure::Registry(RegistryError::Db(err)))
        | Err(Failure::Contract(LvmError::Storage(err))) => return Err(ExecuteError::Db(err.clone())),
        Ok(_) => block.merge(layer.take_changes()),
        Err(_) => {}
    }

    Ok(Receipt {
        id: tx.id(),
        fuel_used,
        result,
    })
}

fn node_fault(err: RegistryError) -> ExecuteError {
    match err {
        RegistryError::Db(err) => ExecuteError::Db(err),
        other => unreachable!("bumping a nonce only fails in the database or at u64::MAX: {other}"),
    }
}

fn run(layer: &Arc<State>, signed: &SignedTx, sender: Address, rules: &Rules) -> (u64, Result<Effect, Failure>) {
    let accounts = Accounts::new(layer);

    match &signed.tx {
        Tx::Transfer { from, to, amount } => (0, moved(accounts.transfer(from, to, amount.to_u256()))),
        Tx::Mint { to, amount } => {
            if rules.minter != Some(sender) {
                return (0, Err(Failure::NotMinter));
            }

            (0, moved(accounts.credit(to, amount.to_u256()).map(|_| ())))
        }
        Tx::Burn { from, amount } => (0, moved(accounts.debit(from, amount.to_u256()).map(|_| ()))),
        Tx::ContractCall {
            to,
            func,
            amount,
            data,
        } => {
            let value = amount.to_u256();

            if let Err(err) = accounts.transfer(&sender, to, value) {
                return (0, Err(Failure::Registry(err)));
            }

            let context = CallContext {
                sender,
                contract: *to,
                value,
            };

            match Contracts::new(layer).call(&context, func, data, rules.limits) {
                Ok(outcome) => (
                    outcome.fuel_used,
                    outcome.result.map(Effect::Returned).map_err(Failure::Contract),
                ),
                Err(err) => (0, Err(Failure::Registry(err))),
            }
        }
        Tx::Deploy { source } => {
            let deployed = Contracts::new(layer).deploy(&sender, signed.nonce, source, rules.limits);

            (0, deployed.map(Effect::Deployed).map_err(Failure::Registry))
        }
    }
}

fn moved(result: Result<(), RegistryError>) -> Result<Effect, Failure> {
    result.map(|()| Effect::Done).map_err(Failure::Registry)
}
