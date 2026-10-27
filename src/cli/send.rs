use {
    super::{CliResult, amount, deployed_at, lua_bundle, parse_amount, parse_argument, signer},
    crate::structs::address::Address,
    crate::structs::bundle::Bundle,
    crate::structs::code::Code,
    crate::structs::hash::{Hash, HashType},
    crate::structs::hex::to_hex,
    crate::chain::fees::OfferChoice,
    crate::registry::signers::Signer,
    crate::crypto::keypair::{Keypair, PublicKey},
    crate::structs::hex::from_hex,
    crate::structs::tx::{FeeOffer, SignedTx, Tx, TxCallArgumentType},
    crate::structs::u256::u256,
    crate::structs::vote::SignedVote,
    crate::vm::lvm::{annotations, compiler},
    crate::vm::selector,
    serde_json::{Value, json},
    std::path::{Path, PathBuf},
    std::time::{Duration, Instant},
    ureq::Agent,
};

#[cfg(feature = "names")]
use crate::registry::names::{NameAction, NamesConfig, name_problem, price};
#[cfg(feature = "governance")]
use crate::registry::names::prices_problem;

/// How often send asks whether its transaction landed.
const POLL: Duration = Duration::from_millis(500);

#[derive(clap::Args)]
pub struct SendArgs {
    /// The node's HTTP API: http://, or https:// behind TLS.
    #[arg(long, default_value = "http://127.0.0.1:8080")]
    pub node: String,
    #[command(flatten)]
    pub tls: TlsArgs,
    /// The key file to sign with, from keygen. Without one, the well-known development key signs.
    #[arg(long)]
    pub key: Option<PathBuf>,
    /// Return once the node takes the transaction, without waiting for a block.
    #[arg(long)]
    pub no_wait: bool,
    /// How long to wait for the transaction to land, in seconds.
    #[arg(long, default_value_t = 60)]
    pub timeout: u64,
    /// How many blocks must hold the transaction, its own included, before send returns. More
    /// than one guards against a proof-of-work fork taking it back off.
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u64).range(1..))]
    pub confirmations: u64,
    /// Wait until the transaction's block is finalized, not just deep enough.
    #[arg(long = "final", conflicts_with = "no_wait")]
    pub finalized: bool,
    /// The most gas it may use. Defaults, for a call, deploy or upgrade, to what a dry run of it
    /// on the node uses plus a quarter, at most the chain's fuel cap; for anything else, to its
    /// intrinsic cost. A dry run that fails leaves the fuel cap's worth.
    #[arg(long)]
    pub gas_limit: Option<u64>,
    /// Send it even though a dry run on the node says it would fail.
    #[arg(long)]
    pub anyway: bool,
    /// Sign it in its EIP-712 typed form for this chain, as MetaMask and hardware wallets sign it,
    /// rather than chainv4's own. A secp256k1 key alone signs typed data.
    #[arg(long)]
    pub eip712: bool,
    /// The most it pays per gas, in the smallest unit. Defaults to twice the base price plus the
    /// tip under a base fee, and to the lowest bid when senders bid.
    #[arg(long, value_parser = parse_amount)]
    pub max_price: Option<u256>,
    /// Under a base fee, what it adds per gas for the producer. Defaults to 0.
    #[arg(long, value_parser = parse_amount)]
    pub tip: Option<u256>,
    #[command(subcommand)]
    pub tx: TxArgs,
}

#[derive(clap::Subcommand)]
pub enum TxArgs {
    /// Sends funds to an address, or to whatever a name resolves to.
    Transfer {
        /// Who receives them, such as ed25519:0x… or bc1q…, or a name, such as alice.dev.
        #[arg(long)]
        to: Recipient,
        /// A whole amount, in the smallest unit.
        #[arg(long, value_parser = parse_amount)]
        amount: u256,
    },
    /// Creates funds. Only the chain's minter may.
    Mint {
        /// Who receives them.
        #[arg(long)]
        to: Address,
        /// A whole amount, in the smallest unit.
        #[arg(long, value_parser = parse_amount)]
        amount: u256,
    },
    /// Destroys funds the signer holds.
    Burn {
        /// A whole amount, in the smallest unit.
        #[arg(long, value_parser = parse_amount)]
        amount: u256,
    },
    /// Deploys a contract from a Luau source file, a WebAssembly module or EVM bytecode.
    #[command(group(clap::ArgGroup::new("code").required(true)))]
    Deploy {
        /// A Luau source file.
        #[arg(long, group = "code")]
        lua: Option<PathBuf>,
        /// A WebAssembly module, such as one built with the SDK.
        #[arg(long, group = "code")]
        wasm: Option<PathBuf>,
        /// EVM init code, as solc --bin or forge build writes it: hex, or the raw bytes.
        #[arg(long, group = "code")]
        evm: Option<PathBuf>,
        /// The EVM constructor's parameter types, such as "(uint256,address)", for its --arg values.
        #[arg(long, requires = "evm")]
        constructor: Option<String>,
        /// A constructor argument, in --constructor's order. Repeat it for each.
        #[arg(long = "arg", requires = "constructor")]
        constructor_args: Vec<String>,
    },
    /// Replaces a contract's code. The contract's check_perms must allow the signer.
    Upgrade {
        /// The contract, such as contract:0x….
        #[arg(long)]
        contract: Address,
        /// The new code: a Luau source file.
        #[arg(long, conflicts_with = "wasm", required_unless_present = "wasm")]
        lua: Option<PathBuf>,
        /// The new code: a WebAssembly module.
        #[arg(long)]
        wasm: Option<PathBuf>,
    },
    #[cfg(feature = "staking")]
    /// Bonds funds from the signer to become a finality validator, voting with --voter.
    Bond {
        /// The voting key, such as the address `chainv4 keygen` prints for a validator key file.
        #[arg(long)]
        voter: PublicKey,
        /// A whole amount, in the smallest unit; at least the chain's minimum bond.
        #[arg(long, value_parser = parse_amount)]
        amount: u256,
    },
    #[cfg(feature = "staking")]
    /// Stops a validator the signer bonded from voting.
    Unbond {
        #[arg(long)]
        voter: PublicKey,
    },
    #[cfg(feature = "staking")]
    /// Pays an unbonded validator's bond back to the signer, once it unlocks.
    Withdraw {
        #[arg(long)]
        voter: PublicKey,
    },
    /// Reports two conflicting votes by one validator, as hex from GET /evidence or the node's log.
    Slash {
        #[arg(long, value_parser = parse_vote)]
        first: Box<SignedVote>,
        #[arg(long, value_parser = parse_vote)]
        second: Box<SignedVote>,
    },
    /// Approves a new set of authority signers, made from the decided one: --add and --remove
    /// change who signs, and the thresholds stay unless given. Sign with a signer's account key.
    ApproveSigners {
        /// A key to make a signer, as KEY, or KEY@ACCOUNT to have another account speak for it,
        /// approving changes and taking its rewards. Repeat for several.
        #[arg(long)]
        add: Vec<String>,
        /// A signer to drop. Repeat for several.
        #[arg(long)]
        remove: Vec<PublicKey>,
        /// How many of the new signers justify a checkpoint. Defaults to the decided set's.
        #[arg(long)]
        threshold: Option<u32>,
        /// How many of the new signers must approve the next change. Defaults to the decided set's.
        #[arg(long)]
        change_threshold: Option<u32>,
        /// The version of the decided set this changes. Defaults to the one the node reports.
        #[arg(long)]
        version: Option<u64>,
    },
    /// Takes back the signer's approval of a change of authority signers. Sign with that signer's
    /// key.
    WithdrawApproval {
        /// The version of the decided set the approval was for. Defaults to the one the node
        /// reports.
        #[arg(long)]
        version: Option<u64>,
    },
    #[cfg(feature = "governance")]
    /// Approves a change under the chain's governance: a new consensus, as a genesis file's
    /// consensus object, or new bootnodes. Sign with an account governance lets approve: a
    /// validator's or signer's account under the electorate, a member's under a council.
    #[command(group = clap::ArgGroup::new("change").required(true).args(["consensus", "bootnodes", "name_prices", "collections", "federation", "directory"]))]
    ApproveChange {
        /// The new consensus as JSON, such as '{"type":"staked","slot_ms":2000,"schedule":"draw"}'
        /// or '{"type":"pow","hash":"blake3","target_block_ms":2000,"initial_difficulty":"1000","min_difficulty":"1000"}'.
        #[arg(long)]
        consensus: Option<String>,
        /// The new bootnodes, replacing the chain's: libp2p addresses separated by commas, such as
        /// /ip4/203.0.113.7/udp/4001/quic-v1, or none to leave the chain with none.
        #[arg(long, value_delimiter = ',')]
        bootnodes: Option<Vec<String>>,
        /// New prices for a year of a name, in the smallest unit, separated by commas: the first
        /// for names of 3 characters, the next for 4, and the last for every length from its own.
        #[arg(long, value_delimiter = ',', value_parser = parse_amount)]
        name_prices: Option<Vec<u256>>,
        /// What makes a collection: standard, for the NFT and multi-token standards' own
        /// functions, or JSON such as '{"nft":["owner_of",…],"multi":["balance_of",…]}'. Turns the
        /// collection registry on for a chain whose genesis kept none.
        #[arg(long)]
        collections: Option<String>,
        /// A new federation for the bridges, as JSON: '{"signers":["0x02…",…],"threshold":2,
        /// "chain_depth":20,"evm_depth":12,"bitcoin_depth":6}'.
        #[arg(long)]
        federation: Option<String>,
        /// A new directory, the contracts the chain announces, as JSON: '{"gateway":"contract:0x…",
        /// "btc_gateway":null,"evm_gateways":[{"chain_id":1,"gateway":"0x…"}],"swap":"contract:0x…",
        /// "dex":null,"dex_router":null,"nft_factory":null}', EVM chains in the order of their ids.
        #[arg(long)]
        directory: Option<String>,
        /// The version of the decision this changes. Defaults to the one the node reports.
        #[arg(long)]
        version: Option<u64>,
    },
    #[cfg(feature = "governance")]
    /// Takes back the sender's approval of a change under governance.
    WithdrawChange {
        /// The version of the decision the approval was for. Defaults to the one the node reports.
        #[arg(long)]
        version: Option<u64>,
    },
    /// Calls a function on a contract.
    Call {
        /// The contract, such as contract:0x….
        #[arg(long)]
        to: Address,
        /// The exported function's name.
        #[arg(long)]
        function: String,
        /// What to send to the contract with the call.
        #[arg(long, default_value = "0", value_parser = parse_amount)]
        amount: u256,
        /// Arguments as type:value, such as i64:5, string:hi or address:ed25519:0x….
        #[arg(value_parser = parse_argument)]
        args: Vec<TxCallArgumentType>,
    },
    /// Registers, renews, gives away or changes a name, such as alice.dev.
    #[cfg(feature = "names")]
    Name {
        #[command(subcommand)]
        action: NameArgs,
    },
    /// Calls an EVM contract, or pays an EVM address, with Solidity calldata.
    EvmCall {
        /// The contract: 0x and 40 hex digits, or any address, such as secp256k1:0x….
        #[arg(long, value_parser = evm_address)]
        to: Address,
        /// The function's signature, such as "bump(uint256)", for the arguments after it.
        #[arg(long, conflicts_with = "data")]
        function: Option<String>,
        /// Calldata in hex, as cast calldata writes it, instead of --function and its arguments.
        #[arg(long)]
        data: Option<String>,
        /// What to send to the contract with the call, in the smallest unit.
        #[arg(long, default_value = "0", value_parser = parse_amount)]
        value: u256,
        /// Only run it on the node and print what it returns, as eth_call does: nothing is sent.
        #[arg(long)]
        read: bool,
        /// The function's arguments, in its signature's order: numbers, true or false, addresses,
        /// hex for bytes, text for strings, and JSON for arrays and tuples, such as [1, 2].
        args: Vec<String>,
    },
}

/// What a name transaction does. Names may be given with the chain's suffix or without it.
#[cfg(feature = "names")]
#[derive(clap::Subcommand)]
pub enum NameArgs {
    /// Registers a name nobody holds, to the signer and pointing at it. Its price is burned.
    Register {
        /// The name, such as alice.dev or alice.
        name: String,
        /// How many years to hold it.
        #[arg(long, default_value_t = 1)]
        years: u32,
    },
    /// Holds the signer's name for more years, from when it expires.
    Renew {
        name: String,
        #[arg(long, default_value_t = 1)]
        years: u32,
    },
    /// Gives a name or subname to another account.
    Transfer {
        name: String,
        /// The new owner.
        #[arg(long)]
        to: Address,
    },
    /// Points a name at an address, or with --none at nothing.
    Target {
        name: String,
        /// The address it resolves to.
        #[arg(long, conflicts_with = "none", required_unless_present = "none")]
        to: Option<Address>,
        /// Resolve to nothing.
        #[arg(long)]
        none: bool,
    },
    /// Sets one of a name's text records, such as url, avatar or description; without a value,
    /// clears it.
    Text {
        name: String,
        key: String,
        value: Option<String>,
    },
    /// Creates a subname, LABEL.NAME, for nothing, owned by and pointing at an account: the signer
    /// unless --owner says otherwise. Creating it again takes it back.
    Subname {
        name: String,
        label: String,
        #[arg(long)]
        owner: Option<Address>,
    },
    /// Makes a name pointing at the signer its primary name, which the API shows for its address;
    /// without a name, clears it.
    Primary { name: Option<String> },
}

/// Whom a transfer goes to: an address, or a name the node resolves.
#[derive(Clone, Debug, PartialEq)]
pub enum Recipient {
    Address(Address),
    Name(String),
}

impl From<Address> for Recipient {
    fn from(address: Address) -> Recipient {
        Recipient::Address(address)
    }
}

impl std::str::FromStr for Recipient {
    type Err = String;

    fn from_str(text: &str) -> Result<Recipient, String> {
        match text.parse::<Address>() {
            Ok(address) => Ok(Recipient::Address(address)),
            Err(_) if !text.contains(':') && text.contains('.') => Ok(Recipient::Name(text.to_string())),
            Err(err) => Err(format!("{text}: {err}; or give a name, such as alice.dev")),
        }
    }
}

/// The chain's names, from the node.
#[cfg(feature = "names")]
fn chain_names(agent: &Agent, node: &str) -> CliResult<NamesConfig> {
    let answer = ok(agent.get(format!("{node}/names")).call())?;

    Ok(NamesConfig {
        suffix: answer["suffix"].as_str().ok_or("the node sent no suffix")?.to_string(),
        prices: super::genesis::prices_from_json(&answer["prices"], "prices")?,
        grace_ms: answer["grace_ms"].as_u64().ok_or("the node sent no grace_ms")?,
        max_years: answer["max_years"].as_u64().and_then(|years| u32::try_from(years).ok()).ok_or("the node sent no max_years")?,
    })
}

/// A name as transactions carry it: without the suffix, and checked here so a mistake is named
/// before anything is sent.
#[cfg(feature = "names")]
fn name_of(names: &NamesConfig, text: &str) -> CliResult<String> {
    let name = names.strip(text);

    match name_problem(&name) {
        Some(problem) => Err(format!("{text} is not a name: {problem}").into()),
        None => Ok(name),
    }
}

/// What a name resolves to now, from the node.
#[cfg(feature = "names")]
pub(super) fn resolve(agent: &Agent, node: &str, name: &str) -> CliResult<Address> {
    let answer = ok(agent.get(format!("{node}/names/{name}")).call())?;
    let status = answer["status"].as_str().unwrap_or("unknown");
    let target = answer["resolves_to"].as_str().ok_or_else(|| format!("{name} resolves to nothing: it is {status}"))?;

    Ok(target.parse()?)
}

/// A name transaction, saying first what a registration or renewal burns.
#[cfg(feature = "names")]
fn name_transaction(action: NameArgs, sender: Address, agent: &Agent, node: &str) -> CliResult<Tx> {
    let names = chain_names(agent, node)?;
    let (name, action) = match action {
        NameArgs::Primary { name } => {
            let name = name.map(|name| name_of(&names, &name)).transpose()?;

            return Ok(Tx::PrimaryName { name });
        }
        NameArgs::Register { name, years } => (name, NameAction::Register { years }),
        NameArgs::Renew { name, years } => (name, NameAction::Renew { years }),
        NameArgs::Transfer { name, to } => (name, NameAction::Transfer { to }),
        NameArgs::Target { name, to, .. } => (name, NameAction::Target { target: to }),
        NameArgs::Text { name, key, value } => (name, NameAction::Text { key, value }),
        NameArgs::Subname { name, label, owner } => (name, NameAction::Subname { label, owner: owner.unwrap_or(sender) }),
    };
    let name = name_of(&names, &name)?;

    if let Some(problem) = action.problem() {
        return Err(problem.into());
    }

    if let NameAction::Register { years } | NameAction::Renew { years } = action {
        let cost = price(&names.prices, &name, years).ok_or("the price overflows")?;
        let plural = if years == 1 { "" } else { "s" };

        println!("{} {} for {years} year{plural}: {cost} burned", action.kind(), names.display(&name));
    }

    Ok(Tx::Name { name, action })
}

/// An address as --to takes it for the EVM.
#[cfg(feature = "evm")]
fn evm_address(text: &str) -> Result<Address, String> {
    super::evm::evm_address(text)
}

#[cfg(not(feature = "evm"))]
fn evm_address(text: &str) -> Result<Address, String> {
    text.parse().map_err(|err| format!("{text}: {err}"))
}

/// Signs a transaction with the signer's next nonce, submits it, and unless told not to, waits
/// until enough blocks hold it and prints what it did. A transaction that ran but failed is an
/// error, so scripts can tell.
/// A transaction signed as EIP-712 typed data for the chain the node's status names.
#[cfg(feature = "secp256k1")]
fn typed(status: &Value, tx: Tx, nonce: u64, fee: FeeOffer, keypair: &Keypair) -> CliResult<SignedTx> {
    let hash: HashType = status["hash"].as_str().and_then(|name| name.parse().ok()).ok_or("the node sent no hash")?;
    let genesis = status["genesis"]
        .as_str()
        .and_then(from_hex)
        .and_then(|bytes| Hash::from_bytes(hash, &bytes))
        .ok_or("the node sent no genesis id that reads")?;

    Ok(SignedTx::sign_eip712(tx, nonce, fee, keypair, &genesis).ok_or("--eip712 needs a secp256k1 key: typed data is Ethereum's")?)
}

#[cfg(not(feature = "secp256k1"))]
fn typed(_: &Value, _: Tx, _: u64, _: FeeOffer, _: &Keypair) -> CliResult<SignedTx> {
    Err("--eip712 needs a secp256k1 key, and this build has no secp256k1".into())
}

pub fn send(args: SendArgs) -> CliResult<()> {
    let keypair = signer(args.key.as_deref())?;
    let sender = keypair.public().address();
    let node = args.node.trim_end_matches('/');
    let agent = agent(&args.tls)?;
    let account = ok(agent.get(format!("{node}/accounts/{sender}")).call())?;
    let nonce = account["nonce"].as_u64().ok_or("the node sent no nonce")?;
    let status = ok(agent.get(format!("{node}/status")).call())?;
    let reads = matches!(args.tx, TxArgs::EvmCall { read: true, .. });
    let tx = transaction(args.tx, sender, &agent, node)?;
    let deploys = matches!(tx, Tx::Deploy { .. });
    let deploys_evm = matches!(tx, Tx::Deploy { code: Code::Evm(_) });
    let chosen = OfferChoice {
        gas_limit: args.gas_limit,
        max_price: args.max_price,
        tip: args.tip,
    };
    let probe = SignedTx::sign(tx.clone(), nonce, FeeOffer::NONE, &keypair);
    let (fee, failure, estimate) = offer(&agent, node, &probe, chosen)?;

    if reads {
        println!("{}", read(&estimate)?);

        return Ok(());
    }

    if let Some(error) = failure {
        if !args.anyway {
            return Err(format!("a dry run says this would fail: {error}. Send it anyway with --anyway").into());
        }

        eprintln!("a dry run says this would fail: {error}; sending it anyway");
    }

    let signed = match args.eip712 {
        true => typed(&status, tx, nonce, fee, &keypair)?,
        false => SignedTx::sign(tx, nonce, fee, &keypair),
    };
    let body = json!({ "tx": to_hex(&wincode::serialize(&signed)?) }).to_string();
    let submitted = ok(agent
        .post(format!("{node}/txs"))
        .header("content-type", "application/json")
        .send(body))?;
    let id = submitted["id"].as_str().ok_or("the node sent no transaction id")?;

    println!("submitted {id}");

    if deploys_evm {
        #[cfg(feature = "evm")]
        println!("contract {}", crate::vm::evm::create_address(&sender, nonce));
    } else if deploys {
        // Contract addresses are derived with the chain's hash, which the node says.
        let hash: HashType = status["hash"]
            .as_str()
            .ok_or("the node did not say its chain's hash")?
            .parse()?;
        println!("contract {}", deployed_at(&sender, nonce, hash));
    }

    if args.no_wait {
        return Ok(());
    }

    let wanted = Wanted {
        confirmations: args.confirmations,
        finalized: args.finalized,
    };
    let receipt = wait(&agent, node, id, wanted, Duration::from_secs(args.timeout))?;
    report(&receipt)
}

/// A signer as --add takes it: KEY, or KEY@ACCOUNT.
fn parse_signer(text: &str) -> CliResult<Signer> {
    let (key, account) = match text.rsplit_once('@') {
        Some((key, account)) => (key, Some(account)),
        None => (text, None),
    };
    let key: PublicKey = key.parse().map_err(|_| format!("{key} is not a public key"))?;
    let account = match account {
        Some(account) => account.parse().map_err(|_| format!("{account} is not an address"))?,
        None => key.address(),
    };

    Ok(Signer { key, account })
}

/// What to offer for a transaction, as the node works it out at POST /offer for a probe signed
/// with none: the flags given, and for the rest what covers it, its gas limit from a dry run. Also
/// why that dry run failed, if it did, and the dry run itself.
fn offer(agent: &Agent, node: &str, probe: &SignedTx, chosen: OfferChoice) -> CliResult<(FeeOffer, Option<String>, Value)> {
    let mut body = json!({ "tx": to_hex(&wincode::serialize(probe)?) });

    if let Some(gas_limit) = chosen.gas_limit {
        body["gas_limit"] = gas_limit.into();
    }

    if let Some(max_price) = chosen.max_price {
        body["max_price"] = max_price.to_string().into();
    }

    if let Some(tip) = chosen.tip {
        body["tip"] = tip.to_string().into();
    }

    let answer = ok(agent.post(format!("{node}/offer")).header("content-type", "application/json").send(body.to_string()))?;
    let wide = |name: &str| -> CliResult<u256> { Ok(parse_amount(answer[name].as_str().ok_or_else(|| format!("the node sent no {name}"))?)?) };
    let fee = FeeOffer {
        gas_limit: answer["gas_limit"].as_u64().ok_or("the node sent no gas_limit")?,
        max_price: wide("max_price")?,
        tip: wide("tip")?,
    };
    let failure = match answer["estimate"]["status"].as_str() {
        Some("failed") => Some(answer["estimate"]["error"].as_str().unwrap_or("no reason given").to_string()),
        _ => None,
    };

    Ok((fee, failure, answer["estimate"].clone()))
}

/// What an EVM call's dry run returned, for evm-call --read, and the gas it used. A revert is an
/// error, with its reason.
pub(super) fn read(estimate: &Value) -> CliResult<String> {
    if estimate["status"] == "failed" {
        return Err(estimate["error"].as_str().unwrap_or("it failed, for no reason given").to_string().into());
    }

    let output = estimate["returned"][0]["value"].as_str().and_then(from_hex).unwrap_or_default();

    #[cfg(feature = "evm")]
    let shown = super::evm::shown(&output);
    #[cfg(not(feature = "evm"))]
    let shown = to_hex(&output);

    Ok(format!("{shown}\ngas {}", estimate["gas_used"]))
}

/// What a transaction's block must reach before send returns.
#[derive(Clone, Copy)]
struct Wanted {
    confirmations: u64,
    finalized: bool,
}

/// A signed vote, encoded in hex as GET /evidence shows it.
fn parse_vote(text: &str) -> Result<Box<SignedVote>, String> {
    let bytes = from_hex(text).ok_or_else(|| "a vote is hex".to_string())?;

    wincode::deserialize(&bytes).map(Box::new).map_err(|_| "not an encoded vote".to_string())
}

/// Reads contract code from whichever file was given.
fn code(lua: Option<PathBuf>, wasm: Option<PathBuf>) -> CliResult<Code> {
    match (lua, wasm) {
        (Some(path), _) => Ok(Code::Lua(lua_source(&path)?)),
        (None, Some(path)) => Ok(Code::Wasm(std::fs::read(&path).map_err(|err| format!("cannot read {}: {err}", path.display()))?)),
        (None, None) => Err("give --lua or --wasm".into()),
    }
}

/// EVM init code from a file, hex as solc writes it or the raw bytes, with its constructor's
/// arguments after it.
fn evm_code(path: &Path, constructor: Option<&str>, args: &[String]) -> CliResult<Code> {
    let bytes = std::fs::read(path).map_err(|err| format!("cannot read {}: {err}", path.display()))?;
    let code = match std::str::from_utf8(&bytes).ok().and_then(|text| from_hex(text.trim())) {
        Some(decoded) => decoded,
        None => bytes,
    };
    let code = match constructor {
        #[cfg(feature = "evm")]
        Some(types) => [code, super::evm::encode(types, args)?].concat(),
        #[cfg(not(feature = "evm"))]
        Some(types) => return Err(format!("this build has no EVM to encode {types} and {} arguments", args.len()).into()),
        None => code,
    };

    if code.is_empty() {
        return Err(format!("{} holds no code", path.display()).into());
    }

    Ok(Code::Evm(code))
}

/// A Lua contract's source: the file, with every file it includes put in, compiled here first so a
/// mistake names its file and line before anything is sent.
pub(super) fn lua_source(path: &Path) -> CliResult<String> {
    let bundle = lua_bundle(path)?;

    if bundle.files.len() > 1 {
        println!("put together {} with {}: {} bytes", bundle.files[0], bundle.files[1..].join(", "), bundle.source.len());
    }

    check(&bundle)?;

    Ok(bundle.source)
}

/// Whether a bundle compiles and its layout reads, as a node will check it, with any mistake
/// placed in the file it came from.
fn check(bundle: &Bundle) -> Result<(), String> {
    let explain = |err: crate::vm::VmError| bundle.explain(&err.to_string());
    compiler::compile(&bundle.source).map_err(explain)?;
    annotations::layout(&bundle.source).map_err(explain)?;

    Ok(())
}

/// Asks the node about the transaction until its block is deep enough, or final when asked, or
/// the time runs out.
fn wait(agent: &Agent, node: &str, id: &str, wanted: Wanted, timeout: Duration) -> CliResult<Value> {
    let deadline = Instant::now() + timeout;

    loop {
        let (status, receipt) = fetch(agent.get(format!("{node}/txs/{id}")).call())?;
        let deep = receipt["confirmations"].as_u64().unwrap_or(0) >= wanted.confirmations;
        let finalized = !wanted.finalized || receipt["final"].as_bool() == Some(true);

        match status {
            200 if deep && finalized => return Ok(receipt),
            // 404 means not in a block yet; 200 means in one, but not deep or final enough.
            200 | 404 => {}
            _ => return Err(refusal(status, &receipt)),
        }

        if Instant::now() >= deadline {
            let goal = match wanted.finalized {
                true => "final".to_string(),
                false => format!("in a block with {} confirmations", wanted.confirmations),
            };

            return Err(format!("{id} was not {goal} after {}s; it may still be", timeout.as_secs()).into());
        }

        std::thread::sleep(POLL);
    }
}

/// Prints where the transaction landed, what it did and what it emitted.
fn report(receipt: &Value) -> CliResult<()> {
    let confirmations = receipt["confirmations"].as_u64().unwrap_or(0);

    println!(
        "included in block {} {} ({confirmations} confirmation{}{})",
        receipt["height"],
        receipt["block"].as_str().unwrap_or("?"),
        if confirmations == 1 { "" } else { "s" },
        if receipt["final"].as_bool() == Some(true) { ", final" } else { "" }
    );

    match receipt["status"].as_str() {
        Some("deployed") => println!("deployed {}", receipt["contract"].as_str().unwrap_or("?")),
        Some("upgraded") => println!("upgraded {}", receipt["contract"].as_str().unwrap_or("?")),
        Some("returned") => println!("returned {}", receipt["values"]),
        Some("failed") => {}
        _ => println!("done"),
    }

    if let Some(fuel) = receipt["fuel_used"].as_u64().filter(|fuel| *fuel > 0) {
        println!("fuel used {fuel}");
    }

    if let Some(gas) = receipt["gas_used"].as_u64().filter(|gas| *gas > 0) {
        println!("gas used {gas}, fee {}", receipt["fee"].as_str().unwrap_or("?"));
    }

    for event in receipt["events"].as_array().into_iter().flatten() {
        println!(
            "event {} from {} {}",
            event["name"].as_str().unwrap_or("?"),
            event["contract"].as_str().unwrap_or("?"),
            event["data"]
        );
    }

    match receipt["status"].as_str() {
        Some("failed") => Err(format!("the transaction failed: {}", receipt["error"].as_str().unwrap_or("?")).into()),
        _ => Ok(()),
    }
}

fn transaction(tx: TxArgs, sender: Address, agent: &Agent, node: &str) -> CliResult<Tx> {
    let tx = match tx {
        TxArgs::Transfer { to, amount: value } => Tx::Transfer {
            to: match to {
                Recipient::Address(address) => address,
                #[cfg(feature = "names")]
                Recipient::Name(name) => {
                    let address = resolve(agent, node, &name)?;
                    println!("{name} resolves to {address}");

                    address
                }
                #[cfg(not(feature = "names"))]
                Recipient::Name(name) => return Err(format!("this build has no names to resolve {name} with").into()),
            },
            from: sender,
            amount: amount(value),
        },
        TxArgs::Mint { to, amount: value } => Tx::Mint { to, amount: amount(value) },
        TxArgs::Burn { amount: value } => Tx::Burn {
            from: sender,
            amount: amount(value),
        },
        TxArgs::Deploy {
            evm: Some(path),
            constructor,
            constructor_args,
            ..
        } => Tx::Deploy {
            code: evm_code(&path, constructor.as_deref(), &constructor_args)?,
        },
        TxArgs::Deploy { lua, wasm, .. } => Tx::Deploy { code: code(lua, wasm)? },
        TxArgs::Upgrade { contract, lua, wasm } => Tx::Upgrade {
            contract,
            code: code(lua, wasm)?,
        },
        #[cfg(feature = "staking")]
        TxArgs::Bond { voter, amount: value } => Tx::Bond { voter, amount: amount(value) },
        #[cfg(feature = "staking")]
        TxArgs::Unbond { voter } => Tx::Unbond { voter },
        #[cfg(feature = "staking")]
        TxArgs::Withdraw { voter } => Tx::Withdraw { voter },
        TxArgs::Slash { first, second } => Tx::Slash { first, second },
        TxArgs::ApproveSigners {
            add,
            remove,
            threshold,
            change_threshold,
            version,
        } => {
            let current = ok(agent.get(format!("{node}/signers")).call())?;
            let decided = &current["decided"];
            let number = |value: &Value, what: &str| -> CliResult<u64> {
                value.as_u64().ok_or_else(|| format!("the node sent no {what}").into())
            };
            let mut signers: Vec<Signer> = decided["signers"]
                .as_array()
                .ok_or("the node sent no signers")?
                .iter()
                .map(|signer| {
                    let key = signer["key"].as_str().and_then(|text| text.parse().ok());
                    let account = signer["account"].as_str().and_then(|text| text.parse().ok());

                    key.zip(account).map(|(key, account)| Signer { key, account }).ok_or("the node sent a bad signer")
                })
                .collect::<Result<_, _>>()?;

            for key in &remove {
                if !signers.iter().any(|signer| signer.key == *key) {
                    return Err(format!("{key} is not a signer").into());
                }
            }

            signers.retain(|signer| !remove.contains(&signer.key));

            for text in add {
                let signer = parse_signer(&text)?;

                if signers.iter().any(|known| known.key == signer.key) {
                    return Err(format!("{} is a signer already", signer.key).into());
                }

                signers.push(signer);
            }

            let threshold = match threshold {
                Some(threshold) => threshold,
                None => u32::try_from(number(&decided["threshold"], "threshold")?)?,
            };
            let change_threshold = match change_threshold {
                Some(threshold) => threshold,
                None => u32::try_from(number(&decided["change_threshold"], "change threshold")?)?,
            };
            let version = match version {
                Some(version) => version,
                None => number(&current["version"], "version")?,
            };
            println!(
                "approving {} signers, threshold {threshold}, change threshold {change_threshold}, from version {version}",
                signers.len()
            );

            Tx::ApproveSigners {
                version,
                signers,
                threshold,
                change_threshold,
            }
        }
        TxArgs::WithdrawApproval { version } => {
            let version = match version {
                Some(version) => version,
                None => ok(agent.get(format!("{node}/signers")).call())?["version"]
                    .as_u64()
                    .ok_or("the node sent no version")?,
            };
            println!("withdrawing this signer's approval for version {version}");

            Tx::WithdrawApproval { version }
        }
        #[cfg(feature = "governance")]
        TxArgs::ApproveChange {
            consensus,
            bootnodes,
            name_prices,
            collections,
            federation,
            directory,
            version,
        } => {
            let change = match (consensus, bootnodes, name_prices, collections, federation, directory) {
                (Some(consensus), _, _, _, _, _) => {
                    let value: Value = serde_json::from_str(&consensus).map_err(|err| format!("--consensus is not JSON: {err}"))?;
                    let (consensus, window) = super::genesis::consensus_from_json(&value, "consensus")?;

                    if window.is_some() {
                        return Err("--consensus: a change keeps the chain's window; leave it out".into());
                    }

                    crate::governance::Change::Consensus(consensus)
                }
                (None, Some(bootnodes), _, _, _, _) => {
                    let bootnodes: Vec<String> = match bootnodes.as_slice() {
                        [none] if none == "none" => Vec::new(),
                        _ => bootnodes.into_iter().map(|address| address.trim().to_string()).collect(),
                    };

                    if let Some(problem) = crate::governance::bootnodes_problem(&bootnodes) {
                        return Err(format!("--bootnodes: {problem}").into());
                    }

                    crate::governance::Change::Bootnodes(bootnodes)
                }
                (None, None, Some(prices), _, _, _) => {
                    if let Some(problem) = prices_problem(&prices) {
                        return Err(format!("--name-prices: {problem}").into());
                    }

                    crate::governance::Change::NamePrices(prices)
                }
                (None, None, None, Some(collections), _, _) => {
                    let functions = super::genesis::collections_from_text(&collections).map_err(|why| format!("--collections: {why}"))?;

                    crate::governance::Change::Collections(functions)
                }
                (None, None, None, None, Some(federation), _) => {
                    crate::governance::Change::Federation(super::genesis::federation_from_text(&federation).map_err(|why| format!("--federation: {why}"))?)
                }
                (None, None, None, None, None, Some(directory)) => {
                    crate::governance::Change::Directory(super::genesis::directory_from_text(&directory).map_err(|why| format!("--directory: {why}"))?)
                }
                (None, None, None, None, None, None) => unreachable!("clap requires one"),
            };

            let version = match version {
                Some(version) => version,
                None => ok(agent.get(format!("{node}/governance")).call())?["version"]
                    .as_u64()
                    .ok_or("the node sent no version")?,
            };
            match &change {
                crate::governance::Change::Consensus(consensus) => println!("approving a change to {} consensus, from version {version}", consensus.name()),
                crate::governance::Change::Bootnodes(bootnodes) => println!("approving {} bootnode{}, from version {version}", bootnodes.len(), if bootnodes.len() == 1 { "" } else { "s" }),
                crate::governance::Change::NamePrices(prices) => println!("approving {} name price{}, from version {version}", prices.len(), if prices.len() == 1 { "" } else { "s" }),
                crate::governance::Change::Collections(functions) => println!(
                    "approving what makes a collection: {} NFT and {} multi-token functions, from version {version}",
                    functions.nft.len(),
                    functions.multi.len()
                ),
                crate::governance::Change::Federation(federation) => println!(
                    "approving a federation of {} of {} signers, from version {version}",
                    federation.threshold,
                    federation.signers.len()
                ),
                crate::governance::Change::Directory(directory) => {
                    let named = [("gateway", directory.gateway), ("Bitcoin gateway", directory.btc_gateway), ("swap", directory.swap), ("DEX", directory.dex), ("NFT factory", directory.nft_factory)];
                    let mut listed: Vec<String> = named.iter().filter_map(|(what, address)| address.map(|address| format!("the {what} {address}"))).collect();
                    listed.extend(directory.dex_router.map(|router| format!("the DEX router {}", crate::structs::hex::to_hex(&router))));
                    listed.extend(directory.evm_gateways.iter().map(|evm| format!("Gateway.sol {} on chain {}", crate::structs::hex::to_hex(&evm.gateway), evm.chain_id)));
                    let listed = if listed.is_empty() { "nothing".to_string() } else { listed.join(", ") };
                    println!("approving a directory of {listed}, from version {version}");
                }
            }

            Tx::ApproveChange { version, change }
        }
        #[cfg(feature = "governance")]
        TxArgs::WithdrawChange { version } => {
            let version = match version {
                Some(version) => version,
                None => ok(agent.get(format!("{node}/governance")).call())?["version"]
                    .as_u64()
                    .ok_or("the node sent no version")?,
            };
            println!("withdrawing this account's approval of a change from version {version}");

            Tx::WithdrawChange { version }
        }
        #[cfg(feature = "names")]
        TxArgs::Name { action } => name_transaction(action, sender, agent, node)?,
        TxArgs::Call {
            to,
            function,
            amount: value,
            args,
        } => Tx::ContractCall {
            to,
            func: Hash::Xxh3_64(selector(&function)),
            amount: amount(value),
            data: args,
        },
        TxArgs::EvmCall {
            to,
            function,
            data,
            value,
            args,
            ..
        } => Tx::EvmCall {
            to,
            value: amount(value),
            input: evm_input(function.as_deref(), data.as_deref(), &args)?,
        },
    };

    Ok(tx)
}

/// An EVM call's calldata: the function and its arguments encoded, or the hex given. Neither is
/// a plain payment.
fn evm_input(function: Option<&str>, data: Option<&str>, args: &[String]) -> CliResult<Vec<u8>> {
    match (function, data) {
        #[cfg(feature = "evm")]
        (Some(signature), _) => Ok(super::evm::call(signature, args)?),
        #[cfg(not(feature = "evm"))]
        (Some(signature), _) => Err(format!("this build has no EVM to encode {signature}; give --data").into()),
        (None, Some(hex)) if args.is_empty() => Ok(from_hex(hex).ok_or_else(|| format!("--data {hex} is not hex"))?),
        (None, Some(_)) => Err("--data is the whole calldata: arguments go with --function".into()),
        (None, None) if args.is_empty() => Ok(Vec::new()),
        (None, None) => Err("give --function for the arguments".into()),
    }
}

type Response = Result<ureq::http::Response<ureq::Body>, ureq::Error>;

/// How to trust a node's HTTPS certificate: Mozilla's roots by default, as a browser would.
#[derive(clap::Args, Clone, Debug, Default)]
pub struct TlsArgs {
    /// Trust the certificates in this PEM file instead of the usual roots: a node's self-signed
    /// one (docker/nginx makes it as fullchain.pem), or a private CA's.
    #[arg(long, value_name = "FILE")]
    pub ca_cert: Option<PathBuf>,
    /// Do not check the node's certificate at all. Anyone between you and the node could read and
    /// change what you send: only for trying a node out on a network you trust.
    #[arg(long, conflicts_with = "ca_cert")]
    pub insecure: bool,
}

/// A client for a node's API: errors come back as answers, so their message can be shown.
pub(super) fn agent(tls: &TlsArgs) -> CliResult<Agent> {
    pooled(tls, 3)
}

/// A client keeping up to this many connections to a node open between requests, as many as
/// requests go at once, so none is closed and opened again: what a client sending many at once,
/// such as bench load, needs, or its machine runs out of ports.
pub(super) fn pooled(tls: &TlsArgs, connections: usize) -> CliResult<Agent> {
    let mut config = ureq::tls::TlsConfig::builder();

    if let Some(path) = &tls.ca_cert {
        let pem = std::fs::read(path).map_err(|err| format!("--ca-cert {}: {err}", path.display()))?;
        let certs = ureq::tls::parse_pem(&pem)
            .filter_map(|item| match item {
                Ok(ureq::tls::PemItem::Certificate(cert)) => Some(Ok(cert)),
                Ok(_) => None,
                Err(err) => Some(Err(err)),
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| format!("--ca-cert {}: {err}", path.display()))?;

        if certs.is_empty() {
            return Err(format!("--ca-cert {} holds no certificate", path.display()).into());
        }

        config = config.root_certs(ureq::tls::RootCerts::new_with_certs(&certs));
    }

    if tls.insecure {
        log::warn!("--insecure: the node's certificate is not checked");
        config = config.disable_verification(true);
    }

    Ok(Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(30)))
        .max_idle_connections(connections.max(3))
        .max_idle_connections_per_host(connections.max(3))
        .tls_config(config.build())
        .build()
        .into())
}

/// The JSON a request came back with, or the node's error.
/// A contract the chain announces in its directory, by its key in GET /status's directory, such as
/// swap or gateway: what the tools use when none is given, never a name someone could register.
pub(super) fn announced(agent: &Agent, node: &str, key: &str) -> CliResult<Option<Address>> {
    match ok(agent.get(format!("{node}/status")).call())?["directory"][key].as_str() {
        Some(text) => Ok(Some(text.parse().map_err(|_| format!("the chain announces a {key} that does not read: {text}"))?)),
        None => Ok(None),
    }
}

pub(super) fn ok(response: Response) -> CliResult<Value> {
    match fetch(response)? {
        (200..=299, json) => Ok(json),
        (status, json) => Err(refusal(status, &json)),
    }
}

/// The status and JSON a request came back with, whatever the status.
pub(super) fn fetch(response: Response) -> CliResult<(u16, Value)> {
    let mut response = response.map_err(|err| format!("cannot reach the node: {err}"))?;
    let status = response.status().as_u16();
    let text = response.body_mut().read_to_string()?;
    let json = serde_json::from_str(&text).map_err(|_| format!("the node answered {status}: {text}"))?;

    Ok((status, json))
}

fn refusal(status: u16, json: &Value) -> Box<dyn std::error::Error> {
    let message = json["error"].as_str().map_or_else(|| json.to_string(), str::to_string);

    format!("the node refused ({status}): {message}").into()
}
