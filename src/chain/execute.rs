use {
    crate::crypto::verifiable::VerifyError,
    crate::db::{DbError, State},
    crate::registry::signers::{SignerSet, Signers},
    crate::registry::validators::{Staking, Validators},
    crate::registry::contracts::{Failed, Hooked},
    crate::registry::{Accounts, Contracts, RegistryError},
    crate::structs::address::Address,
    crate::structs::code::Engine,
    crate::structs::event::Event,
    crate::structs::trace::Trace,
    crate::structs::hash::Hash,
    crate::chain::fees::{Charged, Fees, Scoring},
    crate::structs::tx::{SignedTx, Tx, TxCallArgumentType},
    crate::structs::u256::u256,
    crate::vm::{CallContext, Limits, Outcome, Vm, VmError},
    r#macro::document,
    std::fmt,
    std::sync::Arc,
};

#[document(
    summary = "The chain-wide rules a transaction runs under, from genesis.",
    category = "Chain",
    see_also = "Genesis::rules",
)]
#[derive(Clone, Debug)]
pub struct Rules {
    #[document(summary = "The chain's hash, which transaction ids use.")]
    pub hash: crate::structs::hash::HashType,
    #[document(summary = "The one address allowed to mint, if any.")]
    pub minter: Option<Address>,
    #[document(summary = "What one deploy or contract call may use.")]
    pub limits: Limits,
    #[document(summary = "The functions that make a contract a token, as genesis says; empty keeps no registry.")]
    pub token_functions: Arc<[String]>,
    #[document(summary = "What transactions pay, or None on a chain where they are free.")]
    pub fees: Option<Arc<Fees>>,
    #[document(summary = "What staking transactions run under, or None on a chain without staking, where they fail.")]
    pub staking: Option<StakingRules>,
    #[document(summary = "What signer changes run under, or None on a chain without authority finality, where they fail.")]
    pub signers: Option<SignerRules>,
}

#[document(
    summary = "The settings signer changes run under, on a chain with authority finality.",
    category = "Chain",
    see_also = "Rules, AuthorityConfig, Tx::ApproveSigners",
)]
#[derive(Clone, Copy, Debug)]
pub struct SignerRules {
    #[document(summary = "The most signers a set may have.")]
    pub max_signers: u32,
    #[document(summary = "Blocks per epoch, to tell which epoch an approval is made in.")]
    pub checkpoint_interval: u64,
    #[document(summary = "How many epochs an approval counts for.")]
    pub approval_epochs: u64,
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
    #[document(summary = "A signer change on a chain without authority finality.")]
    NoSigners,
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
            Failure::NoSigners => write!(f, "this chain has no authority signers"),
            Failure::BadEvidence => write!(f, "the evidence's signatures do not check out"),
        }
    }
}

#[document(
    summary = "The block a transaction runs in, as far as it matters to the transaction.",
    category = "Chain",
)]
#[derive(Clone, Copy, Debug)]
pub struct BlockContext {
    #[document(summary = "The block's height.")]
    pub height: u64,
    #[document(summary = "Who produced it, and so gets its fees.")]
    pub producer: Address,
    #[document(summary = "Its base price of gas.")]
    pub base_price: u256,
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
    #[document(summary = "Gas it used, which counts against the block's limit; 0 on a chain without fees.")]
    pub gas_used: u64,
    #[document(summary = "What the sender paid for it; 0 on a chain without fees.")]
    pub fee: u256,
    #[document(summary = "What it did, or why it failed.")]
    pub result: Result<Effect, Failure>,
    #[document(summary = "What its contract calls emitted, in order. Empty when it failed.")]
    pub events: Vec<Event>,
    #[document(summary = "Where its contracts went: every call, delegate and transfer, failed ones included. Empty for a transaction that runs no contract.")]
    pub trace: Trace,
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
    #[document(summary = "Its gas limit does not cover what it costs before it even runs.")]
    GasTooLow { needed: u64, limit: u64 },
    #[document(summary = "Its max price is under the block's base price.")]
    PriceTooLow { base: u256 },
    #[document(summary = "The sender cannot cover its gas limit at its price.")]
    CannotPay,
    #[document(summary = "Its gas limit is more than the block has left.")]
    BlockFull { left: u64 },
}

impl fmt::Display for TxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TxError::Invalid(err) => write!(f, "{err}"),
            TxError::WrongNonce { expected, got } => {
                write!(f, "nonce {got}, but the sender's next is {expected}")
            }
            TxError::GasTooLow { needed, limit } => write!(f, "gas limit {limit}, but it costs {needed} before it runs"),
            TxError::PriceTooLow { base } => write!(f, "max price under the base price of {base}"),
            TxError::CannotPay => write!(f, "the sender cannot pay for its gas limit"),
            TxError::BlockFull { left } => write!(f, "its gas limit is more than the {left} the block has left"),
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

/// What a transaction pays per gas, and what it reserved before running.
struct Payment<'a> {
    fees: &'a Fees,
    intrinsic: u64,
    price: u256,
    reserve: u256,
}

/// Checks a transaction can pay for itself: its gas limit covers its intrinsic cost, its price is
/// enough for the block, and the sender holds its whole gas limit at that price.
fn payment<'a>(fees: &'a Fees, tx: &SignedTx, sender: &Address, accounts: &Accounts, base: u256) -> Result<Payment<'a>, ExecuteError> {
    let invalid = |error| Err(ExecuteError::Invalid(error));
    let intrinsic = fees.schedule().intrinsic(tx);
    let limit = tx.fee.gas_limit;

    if limit < intrinsic {
        return invalid(TxError::GasTooLow { needed: intrinsic, limit });
    }

    let Some(price) = fees.pricing().price(base, &tx.fee) else {
        return invalid(TxError::PriceTooLow { base });
    };
    let Some(reserve) = u256::from(limit).checked_mul(price) else {
        return invalid(TxError::CannotPay);
    };

    if accounts.get(sender)?.balance < reserve {
        return invalid(TxError::CannotPay);
    }

    Ok(Payment {
        fees,
        intrinsic,
        price,
        reserve,
    })
}

/// A transaction that ran: its receipt, and the producer's share of its fee, which whoever runs
/// the block credits as soon as the transaction is in. Crediting it there, rather than here, keeps
/// the producer's balance out of what every transaction reads, so transactions can run side by
/// side without all depending on each other.
#[derive(Debug)]
pub(crate) struct Executed {
    pub(crate) receipt: Receipt,
    pub(crate) to_producer: u256,
}

/// Checks a transaction's form and signature, then runs it on top of its block's state, the way
/// blocks once ran one transaction at a time; what tests compare running side by side against.
#[cfg(test)]
pub(crate) fn execute(
    block: &Arc<State>,
    tx: &SignedTx,
    rules: &Rules,
    vm: &Arc<Vm>,
    context: &BlockContext,
) -> Result<Executed, ExecuteError> {
    use crate::crypto::verifiable::Verifiable;

    tx.verify().map_err(|err| ExecuteError::Invalid(TxError::Invalid(err)))?;

    execute_verified(block, tx, rules, vm, context)
}

/// Runs a transaction whose form and signature were checked already. The nonce and its fee are
/// settled in the block's layer either way; everything else happens in a layer of its own that is
/// kept only on success.
pub(crate) fn execute_verified(
    block: &Arc<State>,
    tx: &SignedTx,
    rules: &Rules,
    vm: &Arc<Vm>,
    context: &BlockContext,
) -> Result<Executed, ExecuteError> {
    let sender = tx.sender();
    let accounts = Accounts::new(block);
    let expected = accounts.get(&sender)?.nonce;

    if tx.nonce != expected {
        return Err(ExecuteError::Invalid(TxError::WrongNonce {
            expected,
            got: tx.nonce,
        }));
    }

    let payment = match &rules.fees {
        Some(fees) => Some(payment(fees, tx, &sender, &accounts, context.base_price)?),
        None => None,
    };
    accounts.bump_nonce(&sender).map_err(node_fault)?;
    let mut limits = rules.limits;

    // The gas limit pays for fuel too: what is left over the intrinsic cost bounds what its
    // contracts may burn, as well as the chain's own cap.
    if let Some(payment) = &payment {
        accounts.debit(&sender, payment.reserve).map_err(node_fault)?;
        let spare = tx.fee.gas_limit - payment.intrinsic;
        limits.fuel = limits.fuel.min(payment.fees.schedule().fuel_for(spare));
    }

    let layer = Arc::new(State::new(block.clone()));
    let (result, contracted) = run(&layer, tx, sender, rules, limits, vm, context.height);
    let Contracted {
        fuel_used,
        mut events,
        mut trace,
    } = contracted;

    // A failing database is the node's fault, never the transaction's.
    match &result {
        Err(Failure::Registry(RegistryError::Db(err)))
        | Err(Failure::Contract(VmError::Storage(err)))
        | Err(Failure::Registry(RegistryError::Vm(VmError::Storage(err)))) => return Err(ExecuteError::Db(err.clone())),
        Ok(_) => block.merge(layer.take_changes()),
        Err(_) => {}
    }

    // Running out of fuel uses all it was allowed, whatever the engine counted at the stop.
    let burned = match &result {
        Err(Failure::Contract(VmError::OutOfFuel)) | Err(Failure::Registry(RegistryError::Vm(VmError::OutOfFuel))) => limits.fuel,
        _ => fuel_used,
    };
    let charged = match payment {
        Some(payment) => settle(block, tx, sender, &payment, burned, result.is_err(), vm, rules, context)?,
        None => Charged::default(),
    };

    if result.is_err() {
        events.clear();
        trace.forget_events();
    }

    let receipt = Receipt {
        id: tx.id(rules.hash),
        fuel_used,
        gas_used: charged.gas_used,
        fee: charged.fee,
        result,
        events,
        trace,
    };

    Ok(Executed {
        receipt,
        to_producer: charged.to_producer,
    })
}

/// Settles a transaction's fee once it ran, for the fuel it burned: what it is charged for, the
/// scorer decides, up to its gas limit, and the sender gets the rest of its reserve back. What the
/// producer gets of it, the caller credits.
#[allow(clippy::too_many_arguments)]
fn settle(
    block: &Arc<State>,
    tx: &SignedTx,
    sender: Address,
    payment: &Payment,
    fuel_used: u64,
    failed: bool,
    vm: &Arc<Vm>,
    rules: &Rules,
    context: &BlockContext,
) -> Result<Charged, ExecuteError> {
    let schedule = payment.fees.schedule();
    let limit = tx.fee.gas_limit;
    let gas = payment.intrinsic.saturating_add(schedule.for_fuel(fuel_used)).min(limit);
    let scoring = Scoring {
        tx,
        sender,
        fuel_used,
        gas,
        failed,
        state: block,
        vm,
        limits: rules.limits,
    };
    let charged = schedule.charge(&scoring).min(limit);
    let settled = payment.fees.settle(context.base_price, payment.price, gas, charged);
    let refund = payment.reserve - settled.fee;

    // It cannot fail: the reserve came from this balance.
    if refund > u256::ZERO {
        Accounts::new(block).credit(&sender, refund).map_err(node_fault)?;
    }

    Ok(settled)
}

/// A registry failure that only the node can cause: bumping a nonce or settling a fee fails in
/// the database, or a balance or nonce at the very end of its range, which no chain reaches.
fn node_fault(err: RegistryError) -> ExecuteError {
    match err {
        RegistryError::Db(err) => ExecuteError::Db(err),
        other => unreachable!("only the database can fail here: {other}"),
    }
}

/// What a transaction's contracts did: the fuel they burned, what they emitted, and where they
/// went. Nothing, for a transaction that runs none.
#[derive(Default)]
struct Contracted {
    fuel_used: u64,
    events: Vec<Event>,
    trace: Trace,
}

impl From<Hooked> for Contracted {
    fn from(hooked: Hooked) -> Contracted {
        Contracted {
            fuel_used: hooked.fuel_used,
            events: hooked.events,
            trace: hooked.trace,
        }
    }
}

/// A deploy or upgrade that failed: it emitted nothing, but burned what it burned and went where
/// it went.
fn failed_hooks(failed: Failed) -> (Result<Effect, Failure>, Contracted) {
    let contracted = Contracted {
        fuel_used: failed.fuel_used,
        events: Vec::new(),
        trace: failed.trace,
    };

    (Err(Failure::Registry(failed.error)), contracted)
}

/// A call's result, once the tokens it asked to be asked again with tokens.refresh are: what
/// their questions burn and where they go join the call's, and an answer that does not fit fails
/// the transaction.
fn refreshed(
    layer: &Arc<State>,
    vm: &Arc<Vm>,
    sender: Address,
    limits: Limits,
    token_functions: &[String],
    outcome: Outcome,
) -> (Result<Effect, Failure>, Contracted) {
    let mut contracted = Contracted {
        fuel_used: outcome.fuel_used,
        events: outcome.events,
        trace: outcome.trace,
    };
    let returned = match outcome.result {
        Ok(returned) => returned,
        Err(err) => return (Err(Failure::Contract(err)), contracted),
    };
    let contracts = Contracts::new(layer);

    for token in &outcome.refresh_tokens {
        let left = Limits {
            fuel: limits.fuel.saturating_sub(contracted.fuel_used),
            ..limits
        };

        match contracts.refresh(vm, token, sender, left, token_functions) {
            Ok((fuel_used, trace)) => {
                contracted.fuel_used += fuel_used;
                contracted.trace.append(trace);
            }
            Err(failed) => {
                contracted.fuel_used += failed.fuel_used;
                contracted.trace.append(failed.trace);

                return (Err(Failure::Registry(failed.error)), contracted);
            }
        }
    }

    (Ok(Effect::Returned(returned)), contracted)
}

fn run(
    layer: &Arc<State>,
    signed: &SignedTx,
    sender: Address,
    rules: &Rules,
    limits: Limits,
    vm: &Arc<Vm>,
    height: u64,
) -> (Result<Effect, Failure>, Contracted) {
    let accounts = Accounts::new(layer);
    let staking = |run: &dyn Fn(&Validators, &StakingRules, u64) -> Result<(), RegistryError>| {
        let Some(finality) = &rules.staking else {
            return (Err(Failure::NoStaking), Contracted::default());
        };
        let epoch = height / finality.checkpoint_interval.max(1);

        (moved(run(&Validators::new(layer), finality, epoch)), Contracted::default())
    };

    match &signed.tx {
        Tx::Transfer { from, to, amount } => (moved(accounts.transfer(from, to, amount.to_u256())), Contracted::default()),
        Tx::Mint { to, amount } => {
            if rules.minter != Some(sender) {
                return (Err(Failure::NotMinter), Contracted::default());
            }

            (moved(accounts.credit(to, amount.to_u256()).map(|_| ())), Contracted::default())
        }
        Tx::Burn { from, amount } => (moved(accounts.debit(from, amount.to_u256()).map(|_| ())), Contracted::default()),
        Tx::ContractCall {
            to,
            func,
            amount,
            data,
        } => {
            let value = amount.to_u256();

            if let Err(err) = accounts.transfer(&sender, to, value) {
                return (Err(Failure::Registry(err)), Contracted::default());
            }

            let context = CallContext {
                sender,
                contract: *to,
                value,
            };

            match Contracts::new(layer).call(vm, &context, func, data, limits) {
                Ok(outcome) => refreshed(layer, vm, sender, limits, &rules.token_functions, outcome),
                Err(err) => (Err(Failure::Registry(err)), Contracted::default()),
            }
        }
        Tx::Deploy { code } => {
            if vm.engine(code.engine()).is_none() {
                return (Err(Failure::EngineNotAllowed(code.engine())), Contracted::default());
            }

            match Contracts::new(layer).deploy(vm, &sender, signed.nonce, code, limits, &rules.token_functions) {
                Ok((address, hooked)) => (Ok(Effect::Deployed(address)), hooked.into()),
                Err(failed) => failed_hooks(failed),
            }
        }
        Tx::Upgrade { contract, code } => {
            if vm.engine(code.engine()).is_none() {
                return (Err(Failure::EngineNotAllowed(code.engine())), Contracted::default());
            }

            match Contracts::new(layer).upgrade(vm, &sender, contract, code, limits, &rules.token_functions) {
                Ok(hooked) => (Ok(Effect::Upgraded(*contract)), hooked.into()),
                Err(failed) => failed_hooks(failed),
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
                return (Err(Failure::BadEvidence), Contracted::default());
            }

            staking(&|validators, finality, epoch| {
                validators.slash(&sender, &first.vote.voter, epoch, &finality.staking).map(|_| ())
            })
        }
        Tx::ApproveSigners {
            version,
            signers,
            threshold,
            change_threshold,
        } => {
            let Some(rules) = &rules.signers else {
                return (Err(Failure::NoSigners), Contracted::default());
            };
            let proposal = SignerSet::new(signers.clone(), *threshold, *change_threshold);
            let epoch = height / rules.checkpoint_interval.max(1);
            let approved = Signers::new(layer).approve(&sender, *version, proposal, rules.max_signers, epoch, rules.approval_epochs);

            (moved(approved.map(|_| ())), Contracted::default())
        }
        Tx::WithdrawApproval { version } => {
            let Some(rules) = &rules.signers else {
                return (Err(Failure::NoSigners), Contracted::default());
            };
            let epoch = height / rules.checkpoint_interval.max(1);

            (moved(Signers::new(layer).withdraw(&sender, *version, epoch, rules.approval_epochs)), Contracted::default())
        }
    }
}

fn moved(result: Result<(), RegistryError>) -> Result<Effect, Failure> {
    result.map(|()| Effect::Done).map_err(Failure::Registry)
}
