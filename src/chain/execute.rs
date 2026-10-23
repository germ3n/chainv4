use {
    crate::crypto::verifiable::{Verifiable, VerifyError},
    crate::db::{DbError, State},
    crate::registry::validators::{Staking, Validators},
    crate::registry::{Accounts, Contracts, RegistryError},
    crate::structs::address::Address,
    crate::structs::code::Engine,
    crate::structs::event::Event,
    crate::structs::hash::Hash,
    crate::structs::tx::{SignedTx, Tx, TxCallArgumentType},
    crate::vm::{CallContext, Limits, Vm, VmError},
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
    #[document(summary = "The chain's hash, which transaction ids use.")]
    pub hash: crate::structs::hash::HashType,
    #[document(summary = "The one address allowed to mint, if any.")]
    pub minter: Option<Address>,
    #[document(summary = "What one deploy or contract call may use.")]
    pub limits: Limits,
    #[document(summary = "What staking transactions run under, or None on a chain without staking, where they fail.")]
    pub staking: Option<StakingRules>,
}

#[document(
    summary = "The settings staking transactions run under, on a chain with FFG finality.",
    category = "Chain",
    see_also = "Rules, FfgConfig",
)]
#[derive(Clone, Copy, Debug)]
pub struct StakingRules {
    #[document(summary = "Bonds, delays and the slashing share.")]
    pub staking: Staking,
    #[document(summary = "Blocks per epoch, to tell which epoch a transaction runs in.")]
    pub checkpoint_interval: u64,
    #[document(summary = "The chain's genesis id, which votes are signed over.")]
    pub chain: Hash,
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
    #[document(summary = "The contract at this address runs new code.")]
    Upgraded(Address),
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
    Contract(VmError),
    #[document(summary = "A mint from an address other than the minter.")]
    NotMinter,
    #[document(summary = "A deploy for an engine this chain's genesis does not allow.")]
    EngineNotAllowed(Engine),
    #[document(summary = "A staking transaction on a chain where validators do not bond.")]
    NoStaking,
    #[document(summary = "Slashing evidence whose signatures do not check out.")]
    BadEvidence,
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Failure::Registry(err) => write!(f, "{err}"),
            Failure::Contract(err) => write!(f, "{err}"),
            Failure::NotMinter => write!(f, "only the minter may mint"),
            Failure::EngineNotAllowed(engine) => write!(f, "this chain does not run {} contracts", engine.name()),
            Failure::NoStaking => write!(f, "this chain has no staking"),
            Failure::BadEvidence => write!(f, "the evidence's signatures do not check out"),
        }
    }
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
    #[document(summary = "What its contract calls emitted, in order. Empty when it failed.")]
    pub events: Vec<Event>,
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
pub(crate) fn execute(
    block: &Arc<State>,
    tx: &SignedTx,
    rules: &Rules,
    vm: &Arc<Vm>,
    height: u64,
) -> Result<Receipt, ExecuteError> {
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
    let (fuel_used, result, events) = run(&layer, tx, sender, rules, vm, height);

    // A failing database is the node's fault, never the transaction's.
    match &result {
        Err(Failure::Registry(RegistryError::Db(err)))
        | Err(Failure::Contract(VmError::Storage(err)))
        | Err(Failure::Registry(RegistryError::Vm(VmError::Storage(err)))) => return Err(ExecuteError::Db(err.clone())),
        Ok(_) => block.merge(layer.take_changes()),
        Err(_) => {}
    }

    let events = if result.is_ok() { events } else { Vec::new() };

    Ok(Receipt {
        id: tx.id(rules.hash),
        fuel_used,
        result,
        events,
    })
}

fn node_fault(err: RegistryError) -> ExecuteError {
    match err {
        RegistryError::Db(err) => ExecuteError::Db(err),
        other => unreachable!("bumping a nonce only fails in the database or at u64::MAX: {other}"),
    }
}

fn run(
    layer: &Arc<State>,
    signed: &SignedTx,
    sender: Address,
    rules: &Rules,
    vm: &Arc<Vm>,
    height: u64,
) -> (u64, Result<Effect, Failure>, Vec<Event>) {
    let accounts = Accounts::new(layer);
    let staking = |run: &dyn Fn(&Validators, &StakingRules, u64) -> Result<(), RegistryError>| {
        let Some(finality) = &rules.staking else {
            return (0, Err(Failure::NoStaking), Vec::new());
        };
        let epoch = height / finality.checkpoint_interval.max(1);

        (0, moved(run(&Validators::new(layer), finality, epoch)), Vec::new())
    };

    match &signed.tx {
        Tx::Transfer { from, to, amount } => (0, moved(accounts.transfer(from, to, amount.to_u256())), Vec::new()),
        Tx::Mint { to, amount } => {
            if rules.minter != Some(sender) {
                return (0, Err(Failure::NotMinter), Vec::new());
            }

            (0, moved(accounts.credit(to, amount.to_u256()).map(|_| ())), Vec::new())
        }
        Tx::Burn { from, amount } => (0, moved(accounts.debit(from, amount.to_u256()).map(|_| ())), Vec::new()),
        Tx::ContractCall {
            to,
            func,
            amount,
            data,
        } => {
            let value = amount.to_u256();

            if let Err(err) = accounts.transfer(&sender, to, value) {
                return (0, Err(Failure::Registry(err)), Vec::new());
            }

            let context = CallContext {
                sender,
                contract: *to,
                value,
            };

            match Contracts::new(layer).call(vm, &context, func, data, rules.limits) {
                Ok(outcome) => (
                    outcome.fuel_used,
                    outcome.result.map(Effect::Returned).map_err(Failure::Contract),
                    outcome.events,
                ),
                Err(err) => (0, Err(Failure::Registry(err)), Vec::new()),
            }
        }
        Tx::Deploy { code } => {
            if vm.engine(code.engine()).is_none() {
                return (0, Err(Failure::EngineNotAllowed(code.engine())), Vec::new());
            }

            match Contracts::new(layer).deploy(vm, &sender, signed.nonce, code, rules.limits) {
                Ok((address, hooked)) => (hooked.fuel_used, Ok(Effect::Deployed(address)), hooked.events),
                Err(err) => (0, Err(Failure::Registry(err)), Vec::new()),
            }
        }
        Tx::Upgrade { contract, code } => {
            if vm.engine(code.engine()).is_none() {
                return (0, Err(Failure::EngineNotAllowed(code.engine())), Vec::new());
            }

            match Contracts::new(layer).upgrade(vm, &sender, contract, code, rules.limits) {
                Ok(hooked) => (hooked.fuel_used, Ok(Effect::Upgraded(*contract)), hooked.events),
                Err(err) => (0, Err(Failure::Registry(err)), Vec::new()),
            }
        }
        Tx::Bond { voter, amount } => staking(&|validators, finality, epoch| {
            validators.bond(&sender, voter, amount.to_u256(), epoch, &finality.staking)
        }),
        Tx::Unbond { voter } => staking(&|validators, finality, epoch| {
            validators.unbond(&sender, voter, epoch, &finality.staking)
        }),
        Tx::Withdraw { voter } => staking(&|validators, _, epoch| validators.withdraw(&sender, voter, epoch).map(|_| ())),
        Tx::Slash { first, second } => {
            if let Some(finality) = &rules.staking
                && (first.verify(&finality.chain).is_err() || second.verify(&finality.chain).is_err())
            {
                return (0, Err(Failure::BadEvidence), Vec::new());
            }

            staking(&|validators, finality, epoch| {
                validators.slash(&sender, &first.vote.voter, epoch, &finality.staking).map(|_| ())
            })
        }
    }
}

fn moved(result: Result<(), RegistryError>) -> Result<Effect, Failure> {
    result.map(|()| Effect::Done).map_err(Failure::Registry)
}
