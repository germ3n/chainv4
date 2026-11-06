use {
    crate::crypto::verifiable::VerifyError,
    crate::db::{DbError, State},
    crate::registry::signers::{SignerSet, Signers},
    crate::registry::names::NamesConfig,
    crate::governance::Governance,
    crate::chain::FinalityConfig,
    crate::registry::validators::Staking,
    crate::registry::contracts::{Failed, Hooked},
    crate::registry::multisig::{MultisigBody, MultisigPolicy, Multisigs},
    crate::registry::{Accounts, Contracts, RegistryError},
    crate::structs::address::Address,
    crate::structs::code::Engine,
    crate::structs::event::Event,
    crate::structs::receipt::{Payment, StepEffect, StepOutcome},
    crate::structs::trace::Trace,
    crate::structs::hash::Hash,
    crate::chain::fees::{Charged, Fees, Scoring},
    crate::structs::tx::{SignedTx, Tx, TxCallArgumentType, batch_deploy_nonce},
    crate::structs::u256::u256,
    crate::vm::{BlockAt, CallContext, Limits, MAX_EVENTS, Outcome, Vm, VmError},
    r#macro::document,
    std::fmt,
    std::sync::Arc,
};

#[cfg(feature = "governance")]
use crate::registry::changes::Changes;
#[cfg(feature = "governance")]
use crate::{governance::Change, registry::changes::ChangeVoted};
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
    #[document(summary = "What makes a contract a token or a collection, as genesis says. Governance may have decided other collection standards since: Rules::standards_in reads them.")]
    pub standards: crate::registry::Standards,
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
    #[document(summary = "What contracts run under in the block: host costs, contract limits, the EVM's spec and the DEX's fee, and the transaction and multisig bounds.")]
    pub vm: crate::params::vm::VmRules,
    #[document(summary = "Anchoring's settings in force: genesis's, or the last decided.")]
    pub anchoring: crate::params::anchoring::AnchorSettings,
}

impl Rules {
    #[document(
        parent = "Rules",
        summary = "What makes a contract a token or a collection in a state: genesis's, with the token functions and collection standards governance decided last, once it decided any.",
    )]
    pub fn standards_in(&self, state: &State) -> Result<crate::registry::Standards, crate::db::DbError> {
        #[cfg(feature = "governance")]
        if self.governance.is_some() {
            let changes = Changes::new(state);
            let (tokens, collections) = (changes.token_functions()?, changes.collections()?);

            if tokens.is_some() || collections.is_some() {
                return Ok(crate::registry::Standards {
                    tokens: tokens.map_or_else(|| self.standards.tokens.clone(), Into::into),
                    collections: collections.or_else(|| self.standards.collections.clone()),
                });
            }
        }
        #[cfg(not(feature = "governance"))]
        let _ = state;

        Ok(self.standards.clone())
    }
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
    see_also = "Rules, GovernanceConfig, Tx::VoteChange",
)]
#[derive(Clone)]
pub struct GovernanceRules {
    #[document(summary = "Who may vote, and what it takes.")]
    pub governance: Arc<dyn Governance>,
    #[document(summary = "Its settings: genesis's, or those governance decided for itself last.")]
    pub config: crate::governance::GovernanceConfig,
    #[document(summary = "How many epochs a vote counts for.")]
    pub approval_epochs: u64,
    #[document(summary = "The share of the voters' weight, in thousandths, that must vote on a change before Yes decides it.")]
    pub quorum_permille: u32,
    #[document(summary = "Blocks per epoch, to tell which epoch an approval is made in.")]
    pub checkpoint_interval: u64,
    #[document(summary = "The chain's finality, which a proposed consensus must fit.")]
    pub finality: FinalityConfig,
    #[document(summary = "What genesis set of each setting governance changes: what stands while nothing was decided, which locks compare a change with.")]
    pub genesis: Arc<crate::registry::changes::GenesisSettings>,
}

impl fmt::Debug for GovernanceRules {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GovernanceRules")
            .field("governance", &self.governance.name())
            .field("approval_epochs", &self.approval_epochs)
            .field("quorum_permille", &self.quorum_permille)
            .field("checkpoint_interval", &self.checkpoint_interval)
            .finish_non_exhaustive()
    }
}

impl GovernanceRules {
    #[document(
        parent = "GovernanceRules",
        summary = "The rules for settings: who votes, what it takes, how long votes count and the quorum, under a chain's finality.",
        params = {
            config = "The settings, genesis's or decided.",
            checkpoint_interval = "Blocks per epoch.",
            finality = "The chain's finality from genesis.",
            genesis = "What genesis set of each setting governance changes.",
        },
    )]
    // A node built without either kind of governance has an empty GovernanceConfig, so build
    // never returns and what follows it goes unused.
    #[cfg_attr(not(any(feature = "governance-electorate", feature = "governance-council")), allow(unused_variables))]
    pub fn new(config: &crate::governance::GovernanceConfig, checkpoint_interval: u64, finality: &FinalityConfig, genesis: Arc<crate::registry::changes::GenesisSettings>) -> GovernanceRules {
        GovernanceRules {
            governance: config.build(finality),
            config: config.clone(),
            approval_epochs: config.approval_epochs(),
            quorum_permille: config.quorum_permille(),
            checkpoint_interval,
            finality: finality.clone(),
            genesis,
        }
    }

    #[cfg(feature = "governance")]
    #[document(
        parent = "GovernanceRules",
        summary = "The rules a state counts votes by: these, genesis's, until governance decided settings of its own, and those from the block after.",
    )]
    pub fn in_state(&self, state: &State) -> Result<GovernanceRules, DbError> {
        Ok(match Changes::new(state).governance()? {
            Some(decided) if decided.config != self.config => GovernanceRules::new(&decided.config, self.checkpoint_interval, &self.finality, self.genesis.clone()),
            _ => self.clone(),
        })
    }

    #[cfg(feature = "governance")]
    #[document(
        parent = "GovernanceRules",
        summary = "The lock a change would break in a state: a locked path where its value differs from what stands there, as Changes::in_force gives it for the epoch. None while nothing is locked, or the change keeps every lock.",
    )]
    pub fn broken_lock(&self, state: &State, epoch: u64, change: &Change) -> Result<Option<String>, DbError> {
        let changes = Changes::new(state);
        let locks = changes.locks()?;

        if locks.is_empty() {
            return Ok(None);
        }

        let standing: std::collections::HashMap<&'static str, serde_json::Value> = changes
            .in_force(epoch, &self.genesis, &self.finality, &self.config)?
            .iter()
            .map(|setting| (setting.name(), crate::params::json::change_json(setting)[setting.name()].clone()))
            .collect();
        let stands = |name: &str| standing.get(name).cloned().unwrap_or(serde_json::Value::Null);

        Ok(crate::governance::locks::broken(&locks, &crate::params::json::change_json(change), &stands).map(str::to_string))
    }

    #[cfg(feature = "governance")]
    #[document(parent = "GovernanceRules", summary = "How long votes count, and the quorum.")]
    pub fn ballot(&self) -> crate::registry::changes::BallotRules {
        crate::registry::changes::BallotRules { approval_epochs: self.approval_epochs, quorum_permille: self.quorum_permille }
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
    #[document(summary = "Whether validators may bond: under FFG, or while a switch into it is on its way. After a switch away from FFG, bonds can only be withdrawn, and slashed for votes from before.")]
    pub bonding: bool,
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
    #[document(summary = "A governance vote decided its change, as the chain's decision-th and its kind's version-th.")]
    Decided { decision: u64, kind: crate::governance::ChangeKind, version: u64 },
    #[document(summary = "A governance vote left its change unable to pass, so it is rejected, as the chain's rejection-th, at its kind's version.")]
    Rejected { rejection: u64, kind: crate::governance::ChangeKind, version: u64 },
    #[document(summary = "A batch went through: what each step did, in order.")]
    Batch(Vec<StepOutcome>),
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
    #[document(summary = "A bond while finality is not FFG and no switch into it is on its way.")]
    BondingClosed,
    #[document(summary = "A claim on a mining pool that pays nothing: see ClaimError.")]
    Claim(String),
    #[document(summary = "An anchor whose proof does not show what it must: see prove.")]
    Anchor(String),
    #[document(summary = "A batch's step failed, which undid every step: which one, counted from 0, and why.")]
    Step { step: u32, failure: Box<Failure> },
}

impl Failure {
    /// What a batch's step failed of, or the failure itself: what the rules about failures go by.
    fn cause(&self) -> &Failure {
        match self {
            Failure::Step { failure, .. } => failure.cause(),
            failure => failure,
        }
    }
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
            Failure::BondingClosed => write!(f, "validators bond only under FFG finality, or before a switch into it"),
            Failure::Claim(why) | Failure::Anchor(why) => write!(f, "{why}"),
            Failure::Step { step, failure } => write!(f, "step {} failed, undoing every step: {failure}", step + 1),
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
    #[document(summary = "The coin its contracts paid, in order. Empty when it failed.")]
    pub payments: Vec<Payment>,
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
    #[document(summary = "A multisig's transaction its members do not back, and why: too few of them, a signature by a key that is no member's or that does not check out, or an address that is no multisig. It never takes the multisig's nonce or fee.")]
    Multisig(RegistryError),
    #[document(summary = "It takes more bytes than the chain's max_tx_bytes allows, its raw bytes for one Ethereum-signed.")]
    TooBig { bytes: u64, limit: u64 },
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
            TxError::Multisig(err) => write!(f, "{err}"),
            TxError::TooBig { bytes, limit } => write!(f, "it takes {bytes} bytes, more than the {limit} a transaction may"),
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
struct Charge<'a> {
    fees: &'a Fees,
    intrinsic: u64,
    price: u256,
    reserve: u256,
}

/// Checks a transaction can pay for itself: its gas limit covers its intrinsic cost, its price is
/// enough for the block, and the sender holds its whole gas limit at that price.
fn payment<'a>(fees: &'a Fees, tx: &SignedTx, sender: &Address, accounts: &Accounts, base: u256) -> Result<Charge<'a>, ExecuteError> {
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

    Ok(Charge {
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

/// Runs a transaction whose form and signature were checked already, once its multisig's members
/// back it, for a multisig's own. The nonce and its fee are settled in the block's layer either
/// way; everything else happens in a layer of its own that is kept only on success.
pub(crate) fn execute_verified(
    block: &Arc<State>,
    tx: &SignedTx,
    rules: &Rules,
    vm: &Arc<Vm>,
    context: &BlockContext,
) -> Result<Executed, ExecuteError> {
    authorize(block, tx, context)?;

    execute_authorized(block, tx, rules, vm, context)
}

/// A multisig's own transaction needs its quorum before anything else: one that falls short never
/// takes the multisig's nonce or fee, so nobody else can spend them.
fn authorize(block: &Arc<State>, tx: &SignedTx, context: &BlockContext) -> Result<(), ExecuteError> {
    if let Tx::Multisig { account, tx: inner, signatures } = &tx.tx {
        let body = MultisigBody { account: *account, tx: (**inner).clone(), nonce: tx.nonce, fee: tx.fee };

        match Multisigs::new(block).backing(&body, signatures, &context.genesis) {
            Ok((policy, backed)) if backed >= policy.threshold => {}
            Ok((policy, backed)) => return Err(ExecuteError::Invalid(TxError::Multisig(RegistryError::NotEnoughMembers { backed, needed: policy.threshold }))),
            Err(RegistryError::Db(err)) => return Err(ExecuteError::Db(err)),
            Err(err) => return Err(ExecuteError::Invalid(TxError::Multisig(err))),
        }
    }

    Ok(())
}

/// Runs a transaction as execute_verified does, whoever backs it: what a dry run of a multisig's
/// transaction, before its members sign, runs.
pub(crate) fn execute_authorized(
    block: &Arc<State>,
    tx: &SignedTx,
    rules: &Rules,
    vm: &Arc<Vm>,
    context: &BlockContext,
) -> Result<Executed, ExecuteError> {
    // Refused long before here for a block, whose transactions verified; a dry run's did not.
    if let Some(problem) = tx.batch_problem() {
        return Err(ExecuteError::Invalid(TxError::Invalid(problem)));
    }

    // A transaction over max_tx_bytes never runs: an Ethereum-signed one by its raw bytes, which
    // are what its signer saw.
    let (bytes, ethereum) = match &tx.tx {
        Tx::Ethereum { raw } => (raw.len() as u64, true),
        _ => (crate::chain::tx_bytes(tx), false),
    };

    if let Some(limit) = rules.vm.tx_bytes_limit(ethereum)
        && bytes > limit
    {
        return Err(ExecuteError::Invalid(TxError::TooBig { bytes, limit }));
    }

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

    // Its nonce moved, so every proposal open for it is void.
    if matches!(tx.tx, Tx::Multisig { .. }) {
        Multisigs::new(block).clear(&sender);
    }

    // On a chain with the EVM, any transaction makes its sender known there: coin the EVM sends
    // its alias reaches it from then on. Kept even when the transaction fails, as its nonce is.
    #[cfg(feature = "evm")]
    if vm.runs_evm() {
        crate::vm::evm::remember_alias(block, &sender)?;
    }

    let mut limits = rules.limits;

    // The gas limit pays for fuel too: what is left over the intrinsic cost bounds what its
    // contracts may burn, as well as the chain's own cap.
    if let Some(payment) = &payment {
        accounts.debit(&sender, payment.reserve).map_err(node_fault)?;
        let spare = tx.fee.gas_limit - payment.intrinsic;
        limits.fuel = limits.fuel.min(payment.fees.schedule().fuel_for(spare));
    }

    let layer = Arc::new(State::new(block.clone()));
    let (result, contracted) = run(&layer, &tx.tx, tx.nonce, sender, rules, limits, vm, context);
    let Contracted {
        fuel_used,
        mut events,
        mut trace,
        mut payments,
    } = contracted;

    // A failing database is the node's fault, never the transaction's.
    match result.as_ref().map_err(Failure::cause) {
        Err(Failure::Registry(RegistryError::Db(err)))
        | Err(Failure::Contract(VmError::Storage(err)))
        | Err(Failure::Registry(RegistryError::Vm(VmError::Storage(err)))) => return Err(ExecuteError::Db(err.clone())),
        Ok(_) => block.merge(layer.take_changes()),
        Err(_) => {}
    }

    // Running out of fuel uses all it was allowed, whatever the engine counted at the stop. A
    // batch's step that does has used all the batch was: the steps share one limit.
    let burned = match result.as_ref().map_err(Failure::cause) {
        Err(Failure::Contract(VmError::OutOfFuel)) | Err(Failure::Registry(RegistryError::Vm(VmError::OutOfFuel))) => limits.fuel,
        _ => fuel_used,
    };
    let charged = match payment {
        Some(payment) => settle(block, tx, sender, &payment, burned, result.is_err(), vm, rules, context)?,
        None => Charged::default(),
    };

    if result.is_err() {
        events.clear();
        payments.clear();
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
        payments,
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
    payment: &Charge,
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
        block: block_at(rules, context),
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
    payments: Vec<Payment>,
}

impl From<Hooked> for Contracted {
    fn from(hooked: Hooked) -> Contracted {
        Contracted {
            fuel_used: hooked.fuel_used,
            events: hooked.events,
            trace: hooked.trace,
            payments: hooked.payments,
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
        payments: Vec::new(),
    };

    (Err(Failure::Registry(failed.error)), contracted)
}

/// A call's result, once the contracts it asked to be asked again with tokens.refresh are: what
/// their questions burn and where they go join the call's, and an answer that does not fit fails
/// the transaction.
fn refreshed(
    layer: &Arc<State>,
    vm: &Arc<Vm>,
    call: &CallContext,
    limits: Limits,
    rules: &Rules,
    outcome: Outcome,
) -> (Result<Effect, Failure>, Contracted) {
    let mut contracted = Contracted {
        fuel_used: outcome.fuel_used,
        events: outcome.events,
        trace: outcome.trace,
        payments: outcome.payments,
    };
    let returned = match outcome.result {
        Ok(returned) => returned,
        Err(err) => return (Err(Failure::Contract(err)), contracted),
    };
    match refresh_tokens(layer, vm, call.sender, call.block, limits, rules, &outcome.refresh_tokens, &mut contracted) {
        Ok(()) => (Ok(Effect::Returned(returned)), contracted),
        Err(failure) => (Err(failure), contracted),
    }
}

/// Asks the tokens a call asked to be asked again with tokens.refresh for what the registry keeps
/// of them: what their questions burn and where they go join the call's, out of what it has left,
/// and an answer that does not fit fails the transaction.
#[allow(clippy::too_many_arguments)]
fn refresh_tokens(layer: &Arc<State>, vm: &Arc<Vm>, sender: Address, block: BlockAt, limits: Limits, rules: &Rules, tokens: &[Address], contracted: &mut Contracted) -> Result<(), Failure> {
    if tokens.is_empty() {
        return Ok(());
    }

    let contracts = Contracts::new(layer);
    let standards = rules.standards_in(layer).map_err(|err| Failure::Registry(RegistryError::Db(err)))?;

    for token in tokens {
        let left = Limits {
            fuel: limits.fuel.saturating_sub(contracted.fuel_used),
            ..limits
        };

        match contracts.refresh(vm, token, sender, left, &standards, block) {
            Ok((fuel_used, trace)) => {
                contracted.fuel_used += fuel_used;
                contracted.trace.append(trace);
            }
            Err(failed) => {
                contracted.fuel_used += failed.fuel_used;
                contracted.trace.append(failed.trace);

                return Err(Failure::Registry(failed.error));
            }
        }
    }

    Ok(())
}

/// Runs a batch's steps in order on one layer, which the caller keeps only if all of them went
/// through: so each sees what those before it did, and any failing undoes them all. They share one
/// budget: each is given what the steps before it left of the transaction's fuel, never its own,
/// and what they emit together keeps to what one call may. Their memory limit needs no sharing:
/// what a step's engine held is gone before the next starts, so no moment holds more than the
/// limit.
#[allow(clippy::too_many_arguments)]
fn batch(
    layer: &Arc<State>,
    steps: &[Tx],
    nonce: u64,
    sender: Address,
    rules: &Rules,
    limits: Limits,
    vm: &Arc<Vm>,
    context: &BlockContext,
) -> (Result<Effect, Failure>, Contracted) {
    let mut all = Contracted::default();
    let mut outcomes = Vec::with_capacity(steps.len());
    let mut deploys = 0;

    for (index, step) in steps.iter().enumerate() {
        let failed = |failure| Failure::Step { step: index as u32, failure: Box::new(failure) };
        let at = match step {
            Tx::Deploy { .. } => batch_deploy_nonce(nonce, deploys),
            _ => nonce,
        };
        deploys += u32::from(matches!(step, Tx::Deploy { .. }));
        let left = Limits { fuel: limits.fuel.saturating_sub(all.fuel_used), ..limits };
        let (result, contracted) = run(layer, step, at, sender, rules, left, vm, context);
        let (events, payments) = (contracted.events.len() as u32, contracted.payments.len() as u32);

        all.fuel_used = all.fuel_used.saturating_add(contracted.fuel_used);
        all.events.extend(contracted.events);
        all.payments.extend(contracted.payments);
        all.trace.append(contracted.trace);

        let effect = match result {
            Ok(Effect::Done) => StepEffect::Done,
            Ok(Effect::Deployed(address)) => StepEffect::Deployed(address),
            Ok(Effect::Returned(values)) => StepEffect::Returned(values),
            Ok(Effect::Upgraded(address)) => StepEffect::Upgraded(address),
            // Nothing a batch holds comes to these: if one ever did, the batch stops, not the chain.
            Ok(Effect::Decided { .. } | Effect::Rejected { .. } | Effect::Batch(_)) => return (Err(failed(Failure::Contract(VmError::Runtime("a step did what no step may".to_string())))), all),
            Err(failure) => return (Err(failed(failure)), all),
        };

        if all.events.len() > MAX_EVENTS {
            return (Err(failed(Failure::Contract(VmError::Runtime(format!("a batch's steps emit at most {MAX_EVENTS} events together"))))), all);
        }

        outcomes.push(StepOutcome { effect, events, payments });
    }

    (Ok(Effect::Batch(outcomes)), all)
}

/// Runs a Tx::Anchor: anchoring's own check, on a build that takes anchors.
fn anchor(layer: &Arc<State>, rules: &Rules, context: &BlockContext, sender: &Address, proof: &crate::structs::tx::AnchorProof) -> Result<Effect, Failure> {
    #[cfg(feature = "anchoring")]
    {
        let interval = rules.staking.as_ref().map(|staking| staking.checkpoint_interval).or(rules.signers.map(|signers| signers.checkpoint_interval));
        let prove = crate::anchor::ProveRules { settings: &rules.anchoring, genesis: context.genesis, checkpoint_interval: interval, pooled: rules.pooled_fees, time: context.timestamp };

        match crate::anchor::prove(layer, &prove, sender, proof) {
            Ok(_) => Ok(Effect::Done),
            Err(crate::anchor::ProveError::Db(err)) => Err(Failure::Registry(RegistryError::Db(err))),
            Err(err) => Err(Failure::Anchor(err.to_string())),
        }
    }
    #[cfg(not(feature = "anchoring"))]
    {
        let _ = (layer, rules, context, sender, proof);

        Err(Failure::Anchor("this build takes no anchors".to_string()))
    }
}

#[allow(clippy::too_many_arguments)]
fn run(
    layer: &Arc<State>,
    tx: &Tx,
    nonce: u64,
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

    match tx {
        // A multisig's own transaction runs as the multisig, its backing checked before.
        Tx::Multisig { tx, .. } => run(layer, tx, nonce, sender, rules, limits, vm, context),
        Tx::Batch { steps } => batch(layer, steps, nonce, sender, rules, limits, vm, context),
        Tx::CreateMultisig { members, threshold, testnet } => {
            let address = crate::structs::address::multisig_address(&sender, nonce, rules.hash);
            let policy = MultisigPolicy { members: members.clone(), threshold: *threshold, testnet: *testnet };

            if members.len() > rules.vm.multisig_members as usize {
                return (Err(Failure::Registry(RegistryError::BadMultisig("a multisig has more members than this chain's multisig_members"))), Contracted::default());
            }

            match Multisigs::new(layer).create(&address, policy) {
                Ok(()) => (Ok(Effect::Deployed(address)), Contracted::default()),
                Err(err) => (Err(Failure::Registry(err)), Contracted::default()),
            }
        }
        Tx::ChangeMultisig { .. } if !matches!(sender, Address::Multisig(_)) => (Err(Failure::Registry(RegistryError::OnlyMultisig)), Contracted::default()),
        Tx::ChangeMultisig { members, threshold, testnet } => {
            let policy = MultisigPolicy { members: members.clone(), threshold: *threshold, testnet: *testnet };

            if members.len() > rules.vm.multisig_members as usize {
                return (Err(Failure::Registry(RegistryError::BadMultisig("a multisig has more members than this chain's multisig_members"))), Contracted::default());
            }

            (moved(Multisigs::new(layer).change(&sender, policy)), Contracted::default())
        }
        Tx::ApproveMultisig { account, nonce, fee, tx } => {
            let body = MultisigBody { account: *account, tx: (**tx).clone(), nonce: *nonce, fee: *fee };
            let approved = accounts.get(account).map_err(RegistryError::Db).and_then(|multisig| Multisigs::new(layer).approve(&sender, &body, multisig.nonce, &context.genesis));

            (moved(approved.map(|_| ())), Contracted::default())
        }
        Tx::WithdrawMultisig { account, proposal } => {
            let withdrawn = accounts.get(account).map_err(RegistryError::Db).and_then(|multisig| Multisigs::new(layer).withdraw(&sender, account, proposal, multisig.nonce));

            (moved(withdrawn), Contracted::default())
        }
        // The directory without governance: its admin's, alone.
        Tx::SetDirectory { directory } => (directed(layer, &sender, |state| match directory.problem() {
            Some(problem) => Err(RegistryError::BadChange(problem)),
            None => {
                crate::registry::changes::Changes::new(state).write_directory(directory);

                Ok(())
            }
        }), Contracted::default()),
        Tx::HandDirectory { to } => (directed(layer, &sender, |state| match to {
            Some(Address::Contract(_)) => Err(RegistryError::BadChange("the directory's admin is an account that signs, not a contract")),
            to => {
                crate::registry::directory::write_directory_admin(state, *to);

                Ok(())
            }
        }), Contracted::default()),
        Tx::Claim { height, to, earned, index, path } => match crate::registry::pools::Pools::new(layer, rules.hash).claim(*height, to, *earned, *index, path) {
            Ok(_) => (Ok(Effect::Done), Contracted::default()),
            Err(crate::registry::pools::ClaimError::Db(err)) => (Err(Failure::Registry(RegistryError::Db(err))), Contracted::default()),
            Err(err) => (Err(Failure::Claim(err.to_string())), Contracted::default()),
        },
        Tx::Anchor { proof } => (anchor(layer, rules, context, &sender, proof), Contracted::default()),
        Tx::Transfer { from, to, amount } => (moved(accounts.transfer(from, to, amount.to_u256())), Contracted::default()),
        Tx::Mint { to, amount } => {
            if rules.minter != Some(sender) {
                return (Err(Failure::NotMinter), Contracted::default());
            }

            let minted = accounts.credit(to, amount.to_u256()).and_then(|_| crate::registry::supply::Supply::mint(layer, amount.to_u256()).map_err(RegistryError::Db));

            (moved(minted), Contracted::default())
        }
        Tx::Burn { from, amount } => {
            let burned = accounts.debit(from, amount.to_u256()).and_then(|_| crate::registry::supply::Supply::burn(layer, amount.to_u256()).map_err(RegistryError::Db));

            (moved(burned), Contracted::default())
        }
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
                block: block_at(rules, context),
            };

            match Contracts::new(layer).call(vm, &call, func, data, limits) {
                Ok(outcome) => refreshed(layer, vm, &call, limits, rules, outcome),
                Err(err) => (Err(Failure::Registry(err)), Contracted::default()),
            }
        }
        #[cfg(feature = "evm")]
        Tx::Deploy { code: crate::structs::code::Code::Evm(init) } if vm.runs_evm() => {
            evm(layer, vm, nonce, sender, rules, limits, context, crate::vm::evm::EvmAction::Create { init: init.clone(), value: u256::ZERO }, &[])
        }
        #[cfg(feature = "evm")]
        Tx::EvmCall { to, value, input } if vm.runs_evm() => evm(
            layer,
            vm,
            nonce,
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

            evm(layer, vm, nonce, sender, rules, limits, context, action, &ethereum.access_list)
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

            let standards = match rules.standards_in(layer) {
                Ok(standards) => standards,
                Err(err) => return (Err(Failure::Registry(RegistryError::Db(err))), Contracted::default()),
            };

            match Contracts::new(layer).deploy(vm, &sender, nonce, code, limits, &standards, block_at(rules, context)) {
                Ok((address, hooked)) => (Ok(Effect::Deployed(address)), hooked.into()),
                Err(failed) => failed_hooks(failed),
            }
        }
        Tx::Upgrade { contract, code } => {
            if vm.engine(code.engine()).is_none() {
                return (Err(Failure::EngineNotAllowed(code.engine())), Contracted::default());
            }

            let standards = match rules.standards_in(layer) {
                Ok(standards) => standards,
                Err(err) => return (Err(Failure::Registry(RegistryError::Db(err))), Contracted::default()),
            };

            match Contracts::new(layer).upgrade(vm, &sender, contract, code, limits, &standards, block_at(rules, context)) {
                Ok(hooked) => (Ok(Effect::Upgraded(*contract)), hooked.into()),
                Err(failed) => failed_hooks(failed),
            }
        }
        #[cfg(feature = "staking")]
        Tx::Bond { .. } if rules.staking.as_ref().is_some_and(|staking| !staking.bonding) => (Err(Failure::BondingClosed), Contracted::default()),
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

            // Under FFG the bond goes; under authority finality the signer is thrown out: by the kind
            // the votes' epoch ran, which a switch may have changed since.
            let authority = match &rules.governance {
                Some(governance) => match crate::registry::changes::Changes::new(layer).finality_in(first.vote.target.epoch, &governance.finality) {
                    Ok(config) => config.signer_rules(chain).is_some(),
                    Err(err) => return (Err(Failure::Registry(err.into())), Contracted::default()),
                },
                None => rules.staking.is_none(),
            };

            match &rules.signers {
                Some(signers) if authority => {
                    let epoch = height / signers.checkpoint_interval.max(1);
                    let ejected = Signers::new(layer).eject(&first.vote.voter, epoch, signers.activation_delay, signers.max_signers);

                    (moved(ejected.map(|_| ())), Contracted::default())
                }
                None if authority => (Err(Failure::NoSigners), Contracted::default()),
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
        Tx::VoteChange { version, change, vote } => {
            let Some(governance) = &rules.governance else {
                return (Err(Failure::NoGovernance), Contracted::default());
            };
            let epoch = height / governance.checkpoint_interval.max(1);
            let finality = &governance.finality;
            // A switch of finality's kind on its way holds back every other change of consensus or
            // finality, which are checked against the finality decided last.
            let settled = |problem: &dyn Fn(&FinalityConfig) -> Option<&'static str>| -> Result<Option<&'static str>, DbError> {
                match crate::chain::switch::switching(layer, finality, epoch)? {
                    true => Ok(Some("a switch of finality's kind decided earlier has yet to take effect")),
                    false => Ok(problem(&Changes::new(layer).finality_decided(finality)?)),
                }
            };
            let problem = match change {
                Change::Consensus(consensus) => settled(&|decided| crate::chain::consensus_problem(consensus, Some(decided))),
                Change::Bootnodes(bootnodes) => Ok(crate::governance::bootnodes_problem(bootnodes)),
                Change::Names(_) if rules.names.is_none() => return (Err(Failure::NoNames), Contracted::default()),
                Change::Names(settings) => Ok(settings.problem()),
                Change::TokenFunctions(functions) => Ok(crate::registry::tokens::functions_problem(functions)),
                Change::EvmSpec { from, .. } if *from <= height => Ok(Some("an EVM spec runs from a block ahead of the one deciding it")),
                Change::EvmSpec { spec, .. } if *spec < rules.vm.evm_spec => Ok(Some("an EVM spec is no older than the one in force, so no deployed contract loses an opcode")),
                Change::EvmSpec { spec, .. } => Changes::new(layer).evm_specs().map(|specs| specs.eras.last().is_some_and(|(_, last)| spec < last).then_some("an EVM spec is no older than the one decided last")),
                Change::Dex(dex) => Ok(dex.problem()),
                Change::Anchoring(settings) => Ok(settings.problem()),
                Change::Collections(functions) => Ok(functions.problem()),
                Change::Federation(federation) => Ok(federation.problem()),
                Change::Directory(directory) => Ok(directory.problem()),
                Change::Fees(fees) => Ok(fees.problem()),
                Change::Reward(reward) => Ok(crate::chain::reward_problem(reward)),
                Change::Limits(limits) => Ok(limits.problem().or_else(|| limits.contract.and_then(|contract| rules.vm.limits.lowering_problem(&contract)))),
                Change::Finality(tuned) => settled(&|decided| tuned.retune_problem(decided)),
                Change::SwitchFinality { epoch: named, config } => crate::chain::switch::switch_problem(layer, finality, epoch, *named, config),
                Change::Upgrade(upgrade) => Ok(upgrade.problem(height)),
                Change::Name(name) => Ok(crate::governance::chain_name_problem(name)),
                Change::Governance(config) => Ok(governance.config.change_problem(config)),
                Change::Locks(paths) => match Changes::new(layer).locks() {
                    Ok(standing) => match crate::governance::locks::adding_problem(&standing, paths) {
                        Some(problem) => return (Err(Failure::Registry(RegistryError::BadLocks(problem))), Contracted::default()),
                        None => Ok(None),
                    },
                    Err(err) => Err(err),
                },
            };

            match problem {
                Ok(None) => {}
                Ok(Some(problem)) => return (Err(Failure::Registry(RegistryError::BadChange(problem))), Contracted::default()),
                Err(err) => return (Err(Failure::Registry(err.into())), Contracted::default()),
            }

            // A change that moves what a lock holds is refused at every vote on it, the one that
            // would decide it included, against what stands then: a lock added since a vote was
            // cast holds that change too.
            match governance.broken_lock(layer, epoch, change) {
                Ok(None) => {}
                Ok(Some(lock)) => return (Err(Failure::Registry(RegistryError::Locked(lock))), Contracted::default()),
                Err(err) => return (Err(Failure::Registry(err.into())), Contracted::default()),
            }

            let at = crate::registry::changes::DecidedAt { height, epoch, activation_delay: governance.finality.activation_delay(), time: context.timestamp };
            let voted = Changes::new(layer).vote(&sender, *version, change.clone(), *vote, at, governance.ballot(), &*governance.governance);

            let effect = voted.map(|voted| match voted {
                ChangeVoted::Decided { decision, kind, version } => Effect::Decided { decision, kind, version },
                ChangeVoted::Rejected { rejection, kind, version } => Effect::Rejected { rejection, kind, version },
                ChangeVoted::Counted(_) => Effect::Done,
            });

            (effect.map_err(Failure::Registry), Contracted::default())
        }
        #[cfg(feature = "governance")]
        Tx::ClearVote { version, change } => {
            let Some(rules) = &rules.governance else {
                return (Err(Failure::NoGovernance), Contracted::default());
            };
            let epoch = height / rules.checkpoint_interval.max(1);
            let at = crate::registry::changes::DecidedAt { height, epoch, activation_delay: rules.finality.activation_delay(), time: context.timestamp };

            (moved(Changes::new(layer).clear(&sender, *version, change, at, rules.approval_epochs, &*rules.governance)), Contracted::default())
        }
        // A build without governance opens no chain that has it.
        #[cfg(not(feature = "governance"))]
        Tx::VoteChange { .. } | Tx::ClearVote { .. } => (Err(Failure::NoGovernance), Contracted::default()),
        #[cfg(feature = "names")]
        Tx::Name { name, action } => {
            use crate::registry::names::Names;

            let Some(names) = &rules.names else {
                return (Err(Failure::NoNames), Contracted::default());
            };
            // The block's names hold governance's settings, once it decided any.
            let acted = Names::new(layer).act(&sender, name, action, context.timestamp, height, names, &names.prices);

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
    vm: &Arc<Vm>,
    nonce: u64,
    sender: Address,
    rules: &Rules,
    limits: Limits,
    context: &BlockContext,
    action: crate::vm::evm::EvmAction,
    access_list: &[crate::crypto::ethereum::AccessItem],
) -> (Result<Effect, Failure>, Contracted) {
    use crate::vm::evm::{self, EvmBlock, EvmReturned};

    let fuel_per_gas = fuel_per_gas(rules);
    let block = EvmBlock {
        height: context.height,
        timestamp: context.timestamp,
        producer: context.producer,
        chain_id: evm::chain_id(&context.genesis),
        names: rules.names.as_deref().cloned(),
        hash: rules.hash,
        vm: rules.vm,
    };

    let bridge = evm::Bridge {
        vm: vm.clone(),
        fuel_per_gas,
        stack: Vec::new(),
    };

    match evm::run(layer, &sender, nonce, action, access_list, limits.fuel / fuel_per_gas, limits.memory as u64, block, Some(&bridge)) {
        Ok(ran) => {
            let mut contracted = Contracted {
                fuel_used: ran.gas_used.saturating_mul(fuel_per_gas),
                events: ran.events,
                trace: ran.trace,
                payments: ran.payments,
            };
            let result = match ran.result {
                Ok(EvmReturned::Created(address)) => Ok(Effect::Deployed(address)),
                Ok(EvmReturned::Output(output)) => Ok(Effect::Returned(vec![TxCallArgumentType::Bytes(output)])),
                Err(err) => Err(Failure::Contract(err)),
            };

            // The tokens the native calls it made asked to be asked again, as a native call's are.
            let result = result.and_then(|effect| {
                refresh_tokens(layer, vm, sender, block_at(rules, context), limits, rules, &ran.refresh_tokens, &mut contracted).map(|()| effect)
            });

            (result, contracted)
        }
        Err(err) => (Err(Failure::Registry(RegistryError::Db(err))), Contracted::default()),
    }
}

/// The block as contracts see it: what names expire by, and what an EVM call a contract makes
/// reads.
fn block_at(rules: &Rules, context: &BlockContext) -> BlockAt {
    BlockAt {
        height: context.height,
        time: context.timestamp,
        producer: context.producer,
        fuel_per_gas: fuel_per_gas(rules),
        vm: rules.vm,
    }
}

/// The fuel one EVM gas burns: gas is the chain's gas one for one.
fn fuel_per_gas(rules: &Rules) -> u64 {
    rules.fees.as_ref().map_or(1, |fees| fees.schedule().fuel_for(1).max(1))
}

/// What the directory's admin does, once the sender is it: on a chain with governance, or none
/// left, nobody is.
fn directed(layer: &State, sender: &Address, act: impl FnOnce(&State) -> Result<(), RegistryError>) -> Result<Effect, Failure> {
    match crate::registry::directory::directory_admin(layer) {
        Ok(Some(admin)) if admin == *sender => moved(act(layer)),
        Ok(_) => Err(Failure::Registry(RegistryError::NotDirectoryAdmin)),
        Err(err) => Err(Failure::Registry(RegistryError::Db(err))),
    }
}

fn moved(result: Result<(), RegistryError>) -> Result<Effect, Failure> {
    result.map(|()| Effect::Done).map_err(Failure::Registry)
}
