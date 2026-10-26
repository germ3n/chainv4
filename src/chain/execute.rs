use {
    crate::crypto::verifiable::VerifyError,
    crate::db::{DbError, State},
    crate::registry::signers::{SignerSet, Signers},
    crate::registry::names::NamesConfig,
    crate::governance::Governance,
    crate::chain::FinalityConfig,
    crate::registry::validators::Staking,
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

#[cfg(any(feature = "governance", feature = "names"))]
use crate::registry::changes::Changes;
#[cfg(feature = "governance")]
use crate::governance::Change;
#[cfg(feature = "staking")]
use crate::registry::validators::Validators;

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
    #[document(summary = "What changes under governance run under, or None on a chain without governance, where they fail.")]
    pub governance: Option<GovernanceRules>,
    #[document(summary = "Whether the burned share of every fee goes to REWARD_POOL, for vote rewards, rather than vanishing.")]
    pub pooled_fees: bool,
    #[document(summary = "The chain's names, or None on a chain without them, where name transactions fail.")]
    pub names: Option<Arc<NamesConfig>>,
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
    #[document(summary = "How many epochs ahead each epoch's signers are fixed, which ejecting a cheater reaches into.")]
    pub activation_delay: u64,
    #[document(summary = "The chain's genesis id, which votes, and so evidence, are signed over.")]
    pub chain: Hash,
}

#[document(
    summary = "The settings changes run under, on a chain with governance.",
    category = "Chain",
    see_also = "Rules, GovernanceConfig, Tx::ApproveChange",
)]
#[derive(Clone)]
pub struct GovernanceRules {
    #[document(summary = "Who may approve, and what it takes.")]
    pub governance: Arc<dyn Governance>,
    #[document(summary = "How many epochs an approval counts for.")]
    pub approval_epochs: u64,
    #[document(summary = "Blocks per epoch, to tell which epoch an approval is made in.")]
    pub checkpoint_interval: u64,
    #[document(summary = "The chain's finality, which a proposed consensus must fit.")]
    pub finality: FinalityConfig,
}

impl fmt::Debug for GovernanceRules {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GovernanceRules")
            .field("governance", &self.governance.name())
            .field("approval_epochs", &self.approval_epochs)
            .field("checkpoint_interval", &self.checkpoint_interval)
            .finish_non_exhaustive()
    }
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
    #[document(summary = "A change approved or withdrawn on a chain without governance.")]
    NoGovernance,
    #[document(summary = "Slashing evidence whose signatures do not check out.")]
    BadEvidence,
    #[document(summary = "A name transaction, or a change of name prices, on a chain without names.")]
    NoNames,
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
            Failure::NoGovernance => write!(f, "this chain has no governance"),
            Failure::BadEvidence => write!(f, "the evidence's signatures do not check out"),
            Failure::NoNames => write!(f, "this chain has no names"),
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
    #[document(summary = "The block's timestamp, in milliseconds since the Unix epoch: what EVM's TIMESTAMP reads, in seconds.")]
    pub timestamp: u64,
    #[document(summary = "Who produced it, and so gets its fees.")]
    pub producer: Address,
    #[document(summary = "Its base price of gas.")]
    pub base_price: u256,
    #[document(summary = "The chain's genesis id, which EIP-712 signatures are bound to.")]
    pub genesis: Hash,
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
    /// The burned share of its fee, which goes to the reward pool on a chain that keeps one.
    pub(crate) burned: u256,
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
    tx.verify_on(&context.genesis).map_err(|err| ExecuteError::Invalid(TxError::Invalid(err)))?;

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
    let (result, contracted) = run(&layer, tx, sender, rules, limits, vm, context);
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
        burned: charged.burned,
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
        time: context.timestamp,
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
    call: &CallContext,
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

        match contracts.refresh(vm, token, call.sender, left, token_functions, call.time) {
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
    context: &BlockContext,
) -> (Result<Effect, Failure>, Contracted) {
    let height = context.height;
    let accounts = Accounts::new(layer);
    #[cfg(feature = "staking")]
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
        Tx::ContractCall { to: Address::Evm(_), .. } => (
            Err(Failure::Contract(VmError::Runtime("an EVM contract is called with an EVM call, with its calldata".to_string()))),
            Contracted::default(),
        ),
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

            let call = CallContext {
                sender,
                contract: *to,
                value,
                time: context.timestamp,
            };

            match Contracts::new(layer).call(vm, &call, func, data, limits) {
                Ok(outcome) => refreshed(layer, vm, &call, limits, &rules.token_functions, outcome),
                Err(err) => (Err(Failure::Registry(err)), Contracted::default()),
            }
        }
        #[cfg(feature = "evm")]
        Tx::Deploy { code: crate::structs::code::Code::Evm(init) } if vm.runs_evm() => {
            evm(layer, signed, sender, rules, limits, context, crate::vm::evm::EvmAction::Create { init: init.clone(), value: u256::ZERO }, &[])
        }
        #[cfg(feature = "evm")]
        Tx::EvmCall { to, value, input } if vm.runs_evm() => evm(
            layer,
            signed,
            sender,
            rules,
            limits,
            context,
            crate::vm::evm::EvmAction::Call { to: *to, value: value.to_u256(), input: input.clone() },
            &[],
        ),
        // An Ethereum transaction runs as Ethereum runs it: a call, or a create when it has no to.
        #[cfg(feature = "evm")]
        Tx::Ethereum { raw } if vm.runs_evm() => {
            // verify_on read it before any block runs it; a dry run's probe may not have been.
            let Ok(ethereum) = crate::crypto::ethereum::decode(raw) else {
                return (Err(Failure::Contract(VmError::Runtime("not an Ethereum transaction".to_string()))), Contracted::default());
            };
            let action = match ethereum.to {
                None => crate::vm::evm::EvmAction::Create { init: ethereum.input, value: ethereum.value },
                Some(to) => crate::vm::evm::EvmAction::Call { to: Address::Evm(to), value: ethereum.value, input: ethereum.input },
            };

            evm(layer, signed, sender, rules, limits, context, action, &ethereum.access_list)
        }
        Tx::EvmCall { .. } | Tx::Ethereum { .. } => (Err(Failure::EngineNotAllowed(Engine::Evm)), Contracted::default()),
        Tx::Upgrade { code, .. } | Tx::Deploy { code } if code.engine() == Engine::Evm && !vm.runs_evm() => {
            (Err(Failure::EngineNotAllowed(Engine::Evm)), Contracted::default())
        }
        Tx::Upgrade { contract: Address::Evm(_), .. } | Tx::Upgrade { code: crate::structs::code::Code::Evm(_), .. } => (
            Err(Failure::Contract(VmError::Runtime("EVM contracts are upgraded as Ethereum's are, through a proxy, not by Upgrade".to_string()))),
            Contracted::default(),
        ),
        Tx::Deploy { code } => {
            if vm.engine(code.engine()).is_none() {
                return (Err(Failure::EngineNotAllowed(code.engine())), Contracted::default());
            }

            match Contracts::new(layer).deploy(vm, &sender, signed.nonce, code, limits, &rules.token_functions, context.timestamp) {
                Ok((address, hooked)) => (Ok(Effect::Deployed(address)), hooked.into()),
                Err(failed) => failed_hooks(failed),
            }
        }
        Tx::Upgrade { contract, code } => {
            if vm.engine(code.engine()).is_none() {
                return (Err(Failure::EngineNotAllowed(code.engine())), Contracted::default());
            }

            match Contracts::new(layer).upgrade(vm, &sender, contract, code, limits, &rules.token_functions, context.timestamp) {
                Ok(hooked) => (Ok(Effect::Upgraded(*contract)), hooked.into()),
                Err(failed) => failed_hooks(failed),
            }
        }
        #[cfg(feature = "staking")]
        Tx::Bond { voter, amount } => staking(&|validators, finality, epoch| {
            validators.bond(&sender, voter, amount.to_u256(), epoch, &finality.staking)
        }),
        #[cfg(feature = "staking")]
        Tx::Unbond { voter } => staking(&|validators, finality, epoch| {
            validators.unbond(&sender, voter, epoch, &finality.staking)
        }),
        #[cfg(feature = "staking")]
        Tx::Withdraw { voter } => staking(&|validators, _, epoch| validators.withdraw(&sender, voter, epoch).map(|_| ())),
        // A build without staking opens no chain whose validators bond.
        #[cfg(not(feature = "staking"))]
        Tx::Bond { .. } | Tx::Unbond { .. } | Tx::Withdraw { .. } => (Err(Failure::NoStaking), Contracted::default()),
        Tx::Slash { first, second } => {
            let chain = match (&rules.staking, &rules.signers) {
                (Some(finality), _) => finality.chain,
                (None, Some(signers)) => signers.chain,
                (None, None) => return (Err(Failure::NoStaking), Contracted::default()),
            };

            if first.verify(&chain).is_err() || second.verify(&chain).is_err() {
                return (Err(Failure::BadEvidence), Contracted::default());
            }

            // Under FFG the bond goes; under authority finality the signer is thrown out.
            match &rules.signers {
                Some(signers) if rules.staking.is_none() => {
                    let epoch = height / signers.checkpoint_interval.max(1);
                    let ejected = Signers::new(layer).eject(&first.vote.voter, epoch, signers.activation_delay, signers.max_signers);

                    (moved(ejected.map(|_| ())), Contracted::default())
                }
                #[cfg(feature = "staking")]
                _ => staking(&|validators, finality, epoch| {
                    validators.slash(&sender, &first.vote.voter, epoch, &finality.staking).map(|_| ())
                }),
                #[cfg(not(feature = "staking"))]
                _ => (Err(Failure::NoStaking), Contracted::default()),
            }
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
        #[cfg(feature = "governance")]
        Tx::ApproveChange { version, change } => {
            let Some(governance) = &rules.governance else {
                return (Err(Failure::NoGovernance), Contracted::default());
            };
            let problem = match change {
                Change::Consensus(consensus) => crate::chain::consensus_problem(consensus, Some(&governance.finality)),
                Change::Bootnodes(bootnodes) => crate::governance::bootnodes_problem(bootnodes),
                Change::NamePrices(_) if rules.names.is_none() => return (Err(Failure::NoNames), Contracted::default()),
                Change::NamePrices(prices) => crate::registry::names::prices_problem(prices),
            };

            if let Some(problem) = problem {
                return (Err(Failure::Registry(RegistryError::BadChange(problem))), Contracted::default());
            }

            let epoch = height / governance.checkpoint_interval.max(1);
            let approved = Changes::new(layer).approve(&sender, *version, change.clone(), epoch, governance.approval_epochs, &*governance.governance);

            (moved(approved.map(|_| ())), Contracted::default())
        }
        #[cfg(feature = "governance")]
        Tx::WithdrawChange { version } => {
            let Some(rules) = &rules.governance else {
                return (Err(Failure::NoGovernance), Contracted::default());
            };
            let epoch = height / rules.checkpoint_interval.max(1);

            (moved(Changes::new(layer).withdraw(&sender, *version, epoch, rules.approval_epochs, &*rules.governance)), Contracted::default())
        }
        // A build without governance opens no chain that has it.
        #[cfg(not(feature = "governance"))]
        Tx::ApproveChange { .. } | Tx::WithdrawChange { .. } => (Err(Failure::NoGovernance), Contracted::default()),
        #[cfg(feature = "names")]
        Tx::Name { name, action } => {
            use crate::registry::names::Names;

            let Some(names) = &rules.names else {
                return (Err(Failure::NoNames), Contracted::default());
            };
            // Governance's prices, once it decided any, else genesis's.
            let acted = Changes::new(layer)
                .name_prices()
                .map_err(RegistryError::Db)
                .and_then(|prices| Names::new(layer).act(&sender, name, action, context.timestamp, height, names, prices.as_deref().unwrap_or(&names.prices)));

            (moved(acted.map(|_| ())), Contracted::default())
        }
        #[cfg(feature = "names")]
        Tx::PrimaryName { name } => {
            let Some(names) = &rules.names else {
                return (Err(Failure::NoNames), Contracted::default());
            };

            (moved(crate::registry::names::Names::new(layer).set_primary(&sender, name.as_deref(), context.timestamp, names)), Contracted::default())
        }
        // A build without names opens no chain that has them.
        #[cfg(not(feature = "names"))]
        Tx::Name { .. } | Tx::PrimaryName { .. } => (Err(Failure::NoNames), Contracted::default()),
    }
}

/// Runs an EVM create or call: gas is the chain's gas one for one, so it burns fuel at the chain's
/// rate, and the EVM may use as much as the fuel left allows.
#[cfg(feature = "evm")]
#[allow(clippy::too_many_arguments)]
fn evm(
    layer: &Arc<State>,
    signed: &SignedTx,
    sender: Address,
    rules: &Rules,
    limits: Limits,
    context: &BlockContext,
    action: crate::vm::evm::EvmAction,
    access_list: &[crate::crypto::ethereum::AccessItem],
) -> (Result<Effect, Failure>, Contracted) {
    use crate::vm::evm::{self, EvmBlock, EvmReturned};

    let fuel_per_gas = rules.fees.as_ref().map_or(1, |fees| fees.schedule().fuel_for(1).max(1));
    let block = EvmBlock {
        height: context.height,
        timestamp: context.timestamp,
        producer: context.producer,
        chain_id: evm::chain_id(&context.genesis),
        names: rules.names.as_deref().cloned(),
    };

    match evm::run(layer, &sender, signed.nonce, action, access_list, limits.fuel / fuel_per_gas, limits.memory as u64, block) {
        Ok(ran) => {
            let contracted = Contracted {
                fuel_used: ran.gas_used.saturating_mul(fuel_per_gas),
                events: ran.events,
                trace: Default::default(),
            };
            let result = match ran.result {
                Ok(EvmReturned::Created(address)) => Ok(Effect::Deployed(address)),
                Ok(EvmReturned::Output(output)) => Ok(Effect::Returned(vec![TxCallArgumentType::Bytes(output)])),
                Err(err) => Err(Failure::Contract(err)),
            };

            (result, contracted)
        }
        Err(err) => (Err(Failure::Registry(RegistryError::Db(err))), Contracted::default()),
    }
}

fn moved(result: Result<(), RegistryError>) -> Result<Effect, Failure> {
    result.map(|()| Effect::Done).map_err(Failure::Registry)
}
