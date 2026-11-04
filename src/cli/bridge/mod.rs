//! `chainv4 bridge`: the bridges from the command line (docs/bridges.md). deploy sets up the
//! gateway here, contracts/lua/gateway.lua with its templates, and deploy-evm Gateway.sol on an
//! EVM chain under the federation in force; peer joins the two; send moves the coin, a token or an
//! NFT either way. sign is a federation signer's pass, signing what left either side once it is
//! final; relay takes what the board has enough signatures for to the EVM chain; watch does both
//! over and over. refund takes a transfer refused for good back to who sent it, a signer's operator
//! at a time ([refund]). btc does Bitcoin's side ([btc]).

pub(crate) mod btc;
mod refund;

use {
    super::send::{SendArgs, TlsArgs, TxArgs, agent, ok, send},
    super::swap::rpc::{Rpc, from_key},
    super::{CliResult, deployed_at, parse_amount, signer},
    crate::abi::{AbiType, AbiValue, decode_values, encode_values},
    crate::bridge::{AssetKind, GATEWAY_CODE, Transfer, WRAPPED_CODES, gateway_constructor, send_input},
    crate::crypto::hasher::Hasher,
    crate::crypto::keypair::PrivateKey,
    crate::structs::address::Address,
    crate::structs::hash::HashType,
    crate::structs::hex::{from_hex, to_hex},
    crate::structs::tx::TxCallArgumentType,
    crate::structs::u256::u256,
    crate::swap::evm::{Call, parse_address},
    serde_json::{Value, json},
    std::path::PathBuf,
};

const GATEWAY_LUA: &str = include_str!("../../../contracts/lua/gateway.lua");
const WRAPPED_LUA: &str = include_str!("../../../contracts/lua/wrapped.lua");
const NFT_LUA: &str = include_str!("../../../contracts/lua/nft.lua");
const MULTI_LUA: &str = include_str!("../../../contracts/lua/multi.lua");

/// How many blocks of an EVM chain's logs are asked for at once, under nodes' limits.
const LOG_WINDOW: u64 = 5_000;

#[derive(clap::Args)]
pub struct BridgeArgs {
    #[command(subcommand)]
    pub action: BridgeAction,
    /// The node's HTTP API.
    #[arg(long, global = true, default_value = "http://127.0.0.1:8080")]
    pub node: String,
    #[command(flatten)]
    pub tls: TlsArgs,
    /// The key file to sign with here: the development key when none is given.
    #[arg(long, global = true)]
    pub key: Option<PathBuf>,
    /// The gateway here, rather than the one the chain's directory names.
    #[arg(long, global = true)]
    pub gateway: Option<Address>,
    /// EVM chains' JSON-RPC URLs, each asked its chain id; separated by commas or given again.
    #[arg(long, global = true, value_delimiter = ',')]
    pub eth_rpc: Vec<String>,
    /// A secp256k1 key file for EVM chains: what pays their gas, and a signer's federation key.
    #[arg(long, global = true)]
    pub evm_key: Option<PathBuf>,
    /// The Bitcoin gateway here, contracts/lua/btc_gateway.lua, for btc's commands.
    #[arg(long, global = true)]
    pub btc_gateway: Option<Address>,
    /// An Esplora API, such as https://mempool.space/api: where btc's commands read Bitcoin.
    #[arg(long, global = true)]
    pub esplora: Option<String>,
    /// A Gateway.sol this operator vouches for on a chain the directory lists none on, as
    /// CHAIN:0x…; repeat for each. A signer signs for no other chain, as whatever the gateway's
    /// owner set there could say anything left it. On a chain with a directory, listing it there is
    /// the better way; this is for one without. It must match the gateway's peer.
    #[arg(long = "evm-gateway", global = true, value_parser = parse_evm_gateway)]
    pub evm_gateways: Vec<(u64, [u8; 20])>,
    /// The most a Bitcoin release may pay for this signer to sign it, in satoshis a virtual byte.
    /// Unless told: 3 times the fastest rate Esplora says, at least 25; with Esplora out of reach,
    /// 200. What is past it is logged and left unsigned, and a pass votes to cancel it, unless all it
    /// has to go by is that fixed 200.
    #[arg(long, global = true)]
    pub max_fee_rate: Option<u64>,
    /// How long, by the chain's block time, a Bitcoin release may stay short of its signatures before
    /// a signer's pass votes to cancel it, and stay unmined with all of them before it warns: 6h, 90m,
    /// 30s. The federation's own policy, btc.cancel_after_ms, wins once governance names one, so
    /// every signer waits alike. Reading a release's age takes a node with --index-events.
    #[arg(long, global = true, default_value = "6h", value_parser = parse_age)]
    pub cancel_after: u64,
    /// How long, by the chain's block time, a Bitcoin release with all its signatures may go unmined
    /// before a signer's pass bumps it, should it pay well under what the network asks, and its bump
    /// again before it bumps that: 1h, 90m, 30s. The federation's own policy, btc.bump_after_ms,
    /// wins once governance names one.
    #[arg(long, global = true, default_value = "1h", value_parser = parse_age)]
    pub bump_after: u64,
}

/// A length of time as milliseconds, from a number and its unit: s, m, h or d.
fn parse_age(text: &str) -> Result<u64, String> {
    let wrong = || format!("{text} is not a time, such as 6h, 90m or 30s");
    let number = text.trim_end_matches(|letter: char| !letter.is_ascii_digit());
    let each = match &text[number.len()..] {
        "s" => 1_000,
        "m" => 60_000,
        "h" => 3_600_000,
        "d" => 86_400_000,
        _ => return Err(wrong()),
    };

    number.parse::<u64>().ok().and_then(|number| number.checked_mul(each)).ok_or_else(wrong)
}

fn parse_evm_gateway(text: &str) -> Result<(u64, [u8; 20]), String> {
    let wrong = || format!("{text} is not CHAIN:0x…, such as 11155111:0x5fbd…");
    let (chain, gateway) = text.split_once(':').ok_or_else(wrong)?;

    Ok((chain.parse().map_err(|_| wrong())?, from_hex(gateway).and_then(|bytes| bytes.try_into().ok()).ok_or_else(wrong)?))
}

#[derive(clap::Subcommand)]
pub enum BridgeAction {
    /// Deploys and sets up the gateway here, with its templates, printing where.
    Deploy,
    /// Deploys Gateway.sol, with its templates, on the EVM chain of the one --eth-rpc, under the
    /// federation in force here, printing where.
    DeployEvm {
        /// What the chain's coin is called, such as Ether.
        #[arg(long, default_value = "Ether")]
        coin_name: String,
        #[arg(long, default_value = "ETH")]
        coin_symbol: String,
    },
    /// Joins the gateway here and the one on the EVM chain of the one --eth-rpc: each the other's
    /// peer. Signed here with --key and there with --evm-key, each its gateway's owner.
    Peer {
        /// Gateway.sol on that chain: 0x and 40 hex digits.
        evm_gateway: String,
    },
    /// Sends the coin, a token or an NFT to another chain: from here to the EVM chain of the one
    /// --eth-rpc, or with --from-evm from there to here. Lets the gateway move it first.
    Send {
        /// coin, token, nft or multi.
        kind: String,
        /// Who gets it there: 0x and 40 hex digits on an EVM chain, an address here.
        recipient: String,
        /// How much, in smallest units: 1 for an NFT.
        #[arg(value_parser = parse_amount)]
        amount: u256,
        /// The token or collection, for anything but the coin: an address here, 0x… there.
        #[arg(long)]
        asset: Option<String>,
        /// An NFT's or multi-token id's id.
        #[arg(long, value_parser = parse_amount, default_value = "0")]
        id: u256,
        /// From the EVM chain to here, signed with --evm-key.
        #[arg(long)]
        from_evm: bool,
    },
    /// A federation signer's pass: signs every transfer that left here, or an EVM chain given by
    /// --eth-rpc, once it is final, posting each signature to the board here. --evm-key is the
    /// signer's key; --key pays the posts.
    Sign {
        /// Where to start reading EVM chains' logs.
        #[arg(long, default_value_t = 0)]
        from_block: u64,
    },
    /// Delivers to each EVM chain given what left here for it with enough signatures on the
    /// board, paid with --evm-key.
    Relay,
    /// Takes a transfer refused for good at its destination back to who sent it, on the word of the
    /// federation's threshold, each signer's operator running this with its --evm-key. It reads both
    /// chains and does the next step: signs to void it at its destination; once that is final, signs
    /// to refund it at its source. Run it again for the next. Asks for yes, unless --yes.
    Refund {
        /// The nonce it left its source with.
        nonce: u64,
        /// It left the EVM chain of the one --eth-rpc for here, rather than here for there.
        #[arg(long)]
        from_evm: bool,
        /// Why, as this operator knows it, to say beside what the chains say.
        #[arg(long)]
        why: Option<String>,
        #[arg(long)]
        yes: bool,
    },
    /// Who sent a transfer names where its refund goes, as a contract that cannot take it might:
    /// signed here with --key, or with --from-evm there with --evm-key.
    RefundTo {
        nonce: u64,
        /// An address here, or 0x and 40 hex digits there.
        recipient: String,
        #[arg(long)]
        from_evm: bool,
    },
    /// Bitcoin's side: the light client contract, deposits minting wBTC, and releases.
    Btc {
        #[command(subcommand)]
        action: btc::BtcAction,
    },
    /// sign and relay, then again after a pause, until stopped. With --esplora, Bitcoin's pass too:
    /// btc pass.
    Watch {
        #[arg(long, default_value_t = 0)]
        from_block: u64,
        /// Seconds between passes.
        #[arg(long, default_value_t = 15)]
        every: u64,
        /// Seconds each signer after the first waits, by its turn, before delivering what is ready
        /// to an EVM chain: long enough for the one before it to be mined there, so one signer pays
        /// for a delivery and the rest step in only should it be down.
        #[arg(long, default_value_t = 90)]
        relay_turn: u64,
    },
}

/// Which turn a signer at this place among `signers` takes in delivering transfer `nonce` to an
/// EVM chain, from 0: the first turn goes round the signers nonce by nonce, so they share the gas,
/// and one that is no signer's (place `signers`) goes after them all.
fn relay_turn(place: u64, signers: u64, nonce: u64) -> u64 {
    if signers == 0 || place >= signers {
        return signers;
    }

    (place + signers - nonce % signers) % signers
}

/// How many times a request the node turns away as too many is asked again, a second, then two,
/// then four after the one before: a signer's pass reads the gateway a good many times, past what
/// a public node's limits let through at once, and one read turned away must not fail its step.
const SLOWED_TRIES: u32 = 4;

/// Asks, and asks again after a pause while the node answers 429, too many requests.
fn slowed<T>(mut wait: impl FnMut(std::time::Duration), mut ask: impl FnMut() -> CliResult<T>) -> CliResult<T> {
    let mut tried = 0;

    loop {
        match ask() {
            Err(err) if tried + 1 < SLOWED_TRIES && err.to_string().starts_with("the node refused (429)") => {
                wait(std::time::Duration::from_secs(1 << tried));
                tried += 1;
            }
            answer => return answer,
        }
    }
}

/// The most passes a transfer refused sits out before it is tried again.
const MOST_SAT_OUT: u64 = 64;

/// A transfer a pass could not sign or deliver.
struct Refusal {
    why: String,
    /// How many passes it sits out after this try: twice as many each time.
    sits: u64,
    /// The pass it is tried again in.
    next: u64,
}

/// What a signer or relayer running over and over remembers of what was refused. A transfer may be
/// refused for good, and one that is must not stop what comes after it, nor be tried at full cost
/// every pass forever; it may also be refused for now, by a node that was away or a recipient that
/// later takes it, so it is tried less and less often rather than never, and stays listed.
#[derive(Default)]
pub(crate) struct Refusals {
    pass: u64,
    held: std::collections::BTreeMap<String, Refusal>,
    /// Transfers in Gateway.sol's logs for another gateway than this one, as a chain's reset leaves
    /// them: by chain and nonce. They never come to be for this one.
    others: std::collections::HashSet<(u64, u64)>,
    /// What is done for good, by gateway here and what was done: signed by this signer, or
    /// delivered or voided at its destination. Never read again, so a pass reads only what is
    /// still to do however many transfers came before.
    done: std::collections::HashSet<(Address, String)>,
}

impl Refusals {
    fn due(&self, what: &str) -> bool {
        self.held.get(what).is_none_or(|refusal| refusal.next <= self.pass)
    }

    fn is_done(&self, gateway: &Address, what: &str) -> bool {
        self.done.contains(&(*gateway, what.to_string()))
    }

    fn settle(&mut self, what: String, result: CliResult<()>) {
        let Err(why) = result else {
            self.held.remove(&what);

            return;
        };
        let sits = self.held.get(&what).map_or(1, |refusal| (refusal.sits * 2).min(MOST_SAT_OUT));
        eprintln!("{what}: {why}; trying it again in {sits} passes");
        self.held.insert(what, Refusal { why: why.to_string(), sits, next: self.pass + sits + 1 });
    }

    /// What is refused for now, for the end of a pass.
    fn listed(&self) -> Vec<String> {
        self.held.iter().map(|(what, refusal)| format!("{what}: {}", refusal.why)).collect()
    }

    fn next_pass(&mut self) {
        self.pass += 1;
    }
}

/// What a command reaches the node with.
pub(crate) struct Bridge {
    agent: ureq::Agent,
    node: String,
    args: BridgeArgs,
    refused: std::cell::RefCell<Refusals>,
    btc_done: std::cell::Cell<btc::BtcDone>,
    /// Under watch, the turns in delivering to an EVM chain. Without them, as relay run by hand is,
    /// a transfer is delivered at once.
    turns: Option<Turns>,
}

/// How long each turn in delivering to an EVM chain lasts, and when this signer first found each
/// transfer ready, by chain and nonce.
struct Turns {
    turn: std::time::Duration,
    ready: std::cell::RefCell<std::collections::HashMap<(u64, u64), std::time::Instant>>,
}

fn kind_of(text: &str) -> CliResult<AssetKind> {
    match text {
        "coin" => Ok(AssetKind::Coin),
        "token" => Ok(AssetKind::Token),
        "nft" => Ok(AssetKind::Nft),
        "multi" => Ok(AssetKind::Multi),
        other => Err(format!("{other} is not a kind: coin, token, nft or multi").into()),
    }
}

fn selector(signature: &str) -> Vec<u8> {
    Hasher::digest(HashType::Keccak256, signature.as_bytes()).to_bytes()[..4].to_vec()
}

fn calldata(signature: &str, values: &[AbiValue]) -> CliResult<Vec<u8>> {
    let (_, types) = crate::abi::parse_signature(signature)?;

    Ok([selector(signature), encode_values(&types, values)?].concat())
}

pub fn bridge(args: BridgeArgs) -> CliResult<()> {
    let bridge = Bridge::new(args)?;

    match &bridge.args.action {
        BridgeAction::Deploy => bridge.deploy(),
        BridgeAction::DeployEvm { coin_name, coin_symbol } => bridge.deploy_evm(coin_name, coin_symbol),
        BridgeAction::Peer { evm_gateway } => bridge.peer(&parse_address(evm_gateway)?),
        BridgeAction::Send { kind, recipient, amount, asset, id, from_evm } => bridge.send_one(kind_of(kind)?, recipient, *amount, asset.as_deref(), *id, *from_evm),
        BridgeAction::Sign { from_block } => bridge.sign(*from_block),
        BridgeAction::Relay => bridge.relay(),
        BridgeAction::Refund { nonce, from_evm, why, yes } => bridge.refund(*nonce, *from_evm, why.as_deref(), *yes, &mut std::io::stdin().lock()),
        BridgeAction::RefundTo { nonce, recipient, from_evm } => bridge.refund_to(*nonce, recipient, *from_evm),
        BridgeAction::Btc { action } => bridge.btc(action),
        BridgeAction::Watch { from_block, every, .. } => loop {
            bridge.pass(*from_block);
            std::thread::sleep(std::time::Duration::from_secs(*every));
        },
    }
}

impl Bridge {
    pub(crate) fn new(args: BridgeArgs) -> CliResult<Bridge> {
        let turns = match &args.action {
            BridgeAction::Watch { relay_turn, .. } => Some(Turns { turn: std::time::Duration::from_secs(*relay_turn), ready: Default::default() }),
            _ => None,
        };

        Ok(Bridge { agent: agent(&args.tls)?, node: args.node.trim_end_matches('/').to_string(), args, refused: Default::default(), btc_done: Default::default(), turns })
    }

    /// What this signer or relayer holds refused, each with why.
    #[cfg(test)]
    pub(crate) fn refused_now(&self) -> Vec<String> {
        self.refused.borrow().listed()
    }

    /// One of watch's passes: sign, and relay whatever became of signing, as what is ready to
    /// deliver has nothing to do with what could not be signed. What is refused stays listed.
    pub(crate) fn pass(&self, from_block: u64) {
        if let Err(err) = self.sign(from_block) {
            eprintln!("signing: {err}");
        }

        if let Err(err) = self.relay() {
            eprintln!("relaying: {err}");
        }

        if self.args.esplora.is_some()
            && let Err(err) = self.btc_pass()
        {
            eprintln!("the Bitcoin pass: {err}");
        }

        let mut refused = self.refused.borrow_mut();

        for line in refused.listed() {
            eprintln!("still refused: {line}");
        }

        refused.next_pass();
    }

    /// Settles what a pass tried: refused, it is listed and tried again later; done for good, it is
    /// never read again while this runs; neither, as one still short of signatures, it comes up next pass.
    fn settle_or_done(&self, gateway: &Address, what: String, result: CliResult<bool>) {
        let mut refused = self.refused.borrow_mut();

        match result {
            Ok(true) => {
                refused.settle(what.clone(), Ok(()));
                refused.done.insert((*gateway, what));
            }
            Ok(false) => refused.settle(what, Ok(())),
            Err(err) => refused.settle(what, Err(err)),
        }
    }

    fn tx(&self, tx: TxArgs) -> CliResult<()> {
        send(SendArgs {
            node: self.node.clone(),
            tls: TlsArgs { ca_cert: self.args.tls.ca_cert.clone(), insecure: self.args.tls.insecure },
            key: self.args.key.clone(),
            no_wait: false,
            timeout: 120,
            confirmations: 1,
            finalized: false,
            gas_limit: None,
            anyway: false,
            genesis: None,
            eip712: false,
            max_price: None,
            tip: None,
            multisig: None,
            out: None,
            tx,
        })
    }

    fn call(&self, to: Address, function: &str, amount: u256, args: Vec<TxCallArgumentType>) -> CliResult<()> {
        self.tx(TxArgs::Call { to, function: function.to_string(), amount, args })
    }

    fn status(&self) -> CliResult<Value> {
        slowed(std::thread::sleep, || ok(self.agent.get(format!("{}/status", self.node)).call()))
    }

    /// A read-only call's values, each as the API writes it: {type, value}.
    fn read(&self, contract: &Address, function: &str, args: Value) -> CliResult<Vec<Value>> {
        let body = json!({ "contract": contract.to_string(), "function": function, "args": args }).to_string();
        let answer = slowed(std::thread::sleep, || ok(self.agent.post(format!("{}/call", self.node)).header("content-type", "application/json").send(&body)))?;

        match answer["status"].as_str() {
            Some("returned") => Ok(answer["values"].as_array().cloned().unwrap_or_default()),
            _ => Err(format!("{function}: {}", answer["error"].as_str().unwrap_or("failed")).into()),
        }
    }

    fn deploy_lua(&self, me: &Address, name: &str, source: &str) -> CliResult<Address> {
        let account = ok(self.agent.get(format!("{}/accounts/{me}", self.node)).call())?;
        let at = deployed_at(me, account["nonce"].as_u64().unwrap_or(0), HashType::default_chain());
        let dir = std::env::temp_dir().join(format!("chainv4-bridge-{}-{:?}", std::process::id(), std::thread::current().id()));
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(name);
        std::fs::write(&path, source)?;
        let sent = self.tx(TxArgs::Deploy { lua: Some(path), wasm: None, evm: None, constructor: None, constructor_args: Vec::new() });
        let _ = std::fs::remove_dir_all(&dir);
        sent?;

        Ok(at)
    }

    /// A gateway the chain announces in its directory: gateway or btc_gateway.
    pub(super) fn announced(&self, key: &str) -> CliResult<Option<Address>> {
        super::send::announced(&self.agent, &self.node, key)
    }

    /// The gateway here: given, or the one the chain announces.
    fn gateway(&self) -> CliResult<Address> {
        match self.args.gateway {
            Some(gateway) => Ok(gateway),
            None => self.announced("gateway")?.ok_or_else(|| "no gateway: the chain's directory names none. Give --gateway, or deploy one with chainv4 bridge deploy and have governance add it".into()),
        }
    }

    /// This chain's id, as the gateways name it: what its EVM answers CHAINID with.
    fn here(&self) -> CliResult<u64> {
        self.status()?["chain_id"].as_u64().ok_or_else(|| "the node does not say its chain id".into())
    }

    /// The one EVM chain given, for commands about one.
    fn rpc(&self) -> CliResult<Rpc> {
        match self.args.eth_rpc.as_slice() {
            [url] => Ok(Rpc::new(self.agent.clone(), url)),
            _ => Err("give one --eth-rpc: the EVM chain's JSON-RPC URL".into()),
        }
    }

    fn evm_secret(&self) -> CliResult<[u8; 32]> {
        let path = self.args.evm_key.as_deref().ok_or("give --evm-key: a secp256k1 key file")?;

        match signer(Some(path))?.private() {
            PrivateKey::Secp256k1(secret) => Ok(*secret),
            _ => Err("--evm-key is a secp256k1 key: make one with keygen --scheme secp256k1".into()),
        }
    }

    fn deploy(&self) -> CliResult<()> {
        let me = signer(self.args.key.as_deref())?.public().address();
        let status = self.status()?;
        let token = self.deploy_lua(&me, "wrapped.lua", WRAPPED_LUA)?;
        let nft = self.deploy_lua(&me, "nft.lua", NFT_LUA)?;
        let multi = self.deploy_lua(&me, "multi.lua", MULTI_LUA)?;
        let gateway = self.deploy_lua(&me, "gateway.lua", GATEWAY_LUA)?;
        let address = TxCallArgumentType::Address;
        self.call(gateway, "setup", u256::ZERO, vec![address(token), address(nft), address(multi)])?;
        // The coin has a symbol, such as DEV, which names it too.
        let symbol = TxCallArgumentType::String(status["coin"]["symbol"].as_str().unwrap_or("COIN").to_string());
        self.call(gateway, "set_coin", u256::ZERO, vec![symbol.clone(), symbol])?;
        println!("gateway {gateway}");

        Ok(())
    }

    fn deploy_evm(&self, coin_name: &str, coin_symbol: &str) -> CliResult<()> {
        let (rpc, secret) = (self.rpc()?, self.evm_secret()?);
        let federation = self.status()?["federation"].clone();

        if federation.is_null() {
            return Err("this chain has no federation to put in Gateway.sol: governance decides one first".into());
        }

        let signers = federation["signers"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|key| {
                let key: [u8; 33] = key.as_str().and_then(from_hex).and_then(|bytes| bytes.try_into().ok()).ok_or("a signer that is not a key")?;

                crate::crypto::secp256k1::address(&key).ok_or_else(|| "a signer that is not a key".into())
            })
            .collect::<CliResult<Vec<[u8; 20]>>>()?;
        let mut templates = [[0; 20]; 3];

        for (template, code) in templates.iter_mut().zip(WRAPPED_CODES) {
            *template = rpc.deploy(&from_hex(code.trim()).ok_or("built-in code is hex")?, &secret)?;
        }

        let threshold = federation["threshold"].as_u64().unwrap_or(1);
        let version = federation["version"].as_u64().unwrap_or(0);
        let init = [from_hex(GATEWAY_CODE.trim()).ok_or("built-in code is hex")?, gateway_constructor(&signers, threshold, version, (coin_name, coin_symbol), templates)].concat();
        // A plain create: whoever deploys owns it, and so sets its peers.
        let receipt = rpc.send(&Call { to: [0; 20], value: u256::ZERO, input: init, what: "Deploy the gateway" }, true, &secret)?;
        println!("gateway {}", receipt["contractAddress"].as_str().ok_or("the deploy's receipt names no contract")?);

        Ok(())
    }

    fn peer(&self, evm_gateway: &[u8; 20]) -> CliResult<()> {
        let (rpc, secret) = (self.rpc()?, self.evm_secret()?);
        let (gateway, here, there) = (self.gateway()?, self.here()?, rpc.chain_id()?);
        self.call(gateway, "set_peer", u256::ZERO, vec![TxCallArgumentType::U64(there), TxCallArgumentType::Bytes(evm_gateway.to_vec())])?;
        let input = calldata("setPeer(uint64,bytes)", &[AbiValue::Uint(u256::from(here)), AbiValue::Bytes(self.identity(&gateway)?)])?;
        rpc.send(&Call { to: *evm_gateway, value: u256::ZERO, input, what: "Set the gateway's peer" }, false, &secret)?;

        Ok(())
    }

    /// The gateway here as transfers write it, and a peer must name it: not only its address.
    fn identity(&self, gateway: &Address) -> CliResult<Vec<u8>> {
        let values = self.read(gateway, "identity", json!([]))?;
        let value = values.first().ok_or("the gateway gave no identity")?;

        Ok(match value["type"].as_str() {
            Some("bytes") => value["value"].as_str().and_then(from_hex).ok_or("the gateway gave an identity that is not hex")?,
            _ => value["value"].as_str().unwrap_or_default().as_bytes().to_vec(),
        })
    }

    /// The gateway on an EVM chain, as the one here knows it: only one the chain's directory lists,
    /// or this operator named with --evm-gateway. A signer vouches for what a peer says left it, and
    /// the gateway's owner sets its peers alone, so going by the peer would let one key mint anything
    /// here, and a chain with no directory is no exception.
    fn evm_gateway(&self, gateway: &Address, chain: u64) -> CliResult<[u8; 20]> {
        let peer = self.read(gateway, "peer", json!([{ "type": "u64", "value": chain.to_string() }]))?;
        let peer: [u8; 20] = peer
            .first()
            .and_then(|value| value["value"].as_str())
            .and_then(from_hex)
            .and_then(|bytes| bytes.try_into().ok())
            .ok_or_else(|| format!("the gateway here has no peer on chain {chain}: join them with chainv4 bridge peer"))?;
        let announced = self.status()?["directory"]["evm_gateways"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|evm| evm["chain_id"].as_u64() == Some(chain))
            .and_then(|evm| evm["gateway"].as_str().and_then(from_hex));

        let vouched = self.args.evm_gateways.iter().find(|(named, _)| *named == chain).map(|(_, gateway)| gateway);

        match (announced, vouched) {
            (Some(announced), _) if announced != peer => Err(format!("the gateway here names {} its peer on chain {chain}, but the chain announces {}", to_hex(&peer), to_hex(&announced)).into()),
            (Some(_), Some(vouched)) if *vouched != peer => Err(format!("--evm-gateway names {} on chain {chain}, and the chain announces {}", to_hex(vouched), to_hex(&peer)).into()),
            (Some(_), _) => Ok(peer),
            (None, Some(vouched)) if *vouched == peer => Ok(peer),
            (None, Some(vouched)) => Err(format!("the gateway here names {} its peer on chain {chain}, but --evm-gateway names {}", to_hex(&peer), to_hex(vouched)).into()),
            (None, None) => Err(format!("the chain's directory lists no gateway on chain {chain}: have governance list it, or vouch for it yourself with --evm-gateway {chain}:{}", to_hex(&peer)).into()),
        }
    }

    fn send_one(&self, kind: AssetKind, recipient: &str, amount: u256, asset: Option<&str>, id: u256, from_evm: bool) -> CliResult<()> {
        let gateway = self.gateway()?;

        match from_evm {
            false => {
                let rpc = self.rpc()?;
                let recipient = parse_address(recipient)?;
                let target = rpc.chain_id()?;
                // No signer signs for a chain it may not vouch for, so it would never arrive.
                self.evm_gateway(&gateway, target)?;
                let asset = asset.map(|asset| asset.parse::<Address>().map_err(|_| format!("{asset} is not an address here"))).transpose()?;
                let wrapped = match asset {
                    Some(asset) => !self.read(&gateway, "origin_of", json!([{ "type": "address", "value": asset.to_string() }]))?.first().is_none_or(|value| value["value"].is_null()),
                    None => false,
                };
                let wide = TxCallArgumentType::U256;

                // The gateway takes what it holds with transfer_from; what it wrapped, it burns.
                match (kind, asset, wrapped) {
                    (AssetKind::Token, Some(asset), false) => self.call(asset, "approve", u256::ZERO, vec![TxCallArgumentType::Address(gateway), wide(amount)])?,
                    (AssetKind::Nft, Some(asset), _) => self.call(asset, "approve", u256::ZERO, vec![TxCallArgumentType::Address(gateway), wide(id)])?,
                    (AssetKind::Multi, Some(asset), _) => self.call(asset, "set_approval_for_all", u256::ZERO, vec![TxCallArgumentType::Address(gateway), TxCallArgumentType::Bool(true)])?,
                    (AssetKind::Coin, None, _) | (AssetKind::Token, Some(_), true) => {}
                    _ => return Err("the coin takes no --asset; anything else names it".into()),
                }

                let coin = if kind == AssetKind::Coin { amount } else { u256::ZERO };
                let asset = asset.map_or(TxCallArgumentType::Nil, TxCallArgumentType::Address);
                self.call(gateway, "send", coin, vec![TxCallArgumentType::U64(target), TxCallArgumentType::Bytes(recipient.to_vec()), TxCallArgumentType::U64(kind as u64), asset, wide(id), wide(amount)])
            }
            true => {
                let (rpc, secret) = (self.rpc()?, self.evm_secret()?);
                let evm_gateway = self.evm_gateway(&gateway, rpc.chain_id()?)?;
                let recipient: Address = recipient.parse().map_err(|_| format!("{recipient} is not an address here"))?;
                let asset = asset.map(parse_address).transpose()?.unwrap_or([0; 20]);
                let origin = rpc.call(&evm_gateway, &calldata("originOf(address)", &[AbiValue::Address(asset)])?)?;
                let wrapped = decode_values(&[AbiType::Uint(64), AbiType::Uint(8), AbiType::Bytes], &origin).ok().is_some_and(|values| values[0] != AbiValue::Uint(u256::ZERO));
                let approve = match (kind, wrapped) {
                    (AssetKind::Token, false) => Some(calldata("approve(address,uint256)", &[AbiValue::Address(evm_gateway), AbiValue::Uint(amount)])?),
                    (AssetKind::Nft, false) => Some(calldata("approve(address,uint256)", &[AbiValue::Address(evm_gateway), AbiValue::Uint(id)])?),
                    (AssetKind::Multi, false) => Some(calldata("setApprovalForAll(address,bool)", &[AbiValue::Address(evm_gateway), AbiValue::Bool(true)])?),
                    _ => None,
                };

                if let Some(input) = approve {
                    rpc.send(&Call { to: asset, value: u256::ZERO, input, what: "Let the gateway move it" }, false, &secret)?;
                }

                let value = if kind == AssetKind::Coin { amount } else { u256::ZERO };
                let input = send_input(self.here()?, &recipient.to_abi(), kind, asset, id, amount);
                rpc.send(&Call { to: evm_gateway, value, input, what: "Send through the gateway" }, false, &secret)?;

                Ok(())
            }
        }
    }

    /// What left here and what left each EVM chain given, signed and posted once final. Each
    /// transfer stands alone: one refused is said and passed over, for the next one, and one chain
    /// away stops no other.
    fn sign(&self, from_block: u64) -> CliResult<()> {
        let secret = self.evm_secret()?;
        let gateway = self.gateway()?;
        let status = self.status()?;
        let here = self.here()?;
        let federation = &status["federation"];
        let identity = self.identity(&gateway)?;
        let my_key = crate::crypto::secp256k1::public_key(&secret).ok_or("--evm-key is no key")?;

        if !federation["signers"].as_array().into_iter().flatten().any(|key| key.as_str() == Some(&to_hex(&my_key))) {
            return Err("--evm-key is not a signer of the federation in force".into());
        }

        // Final here: at or below the finalized checkpoint, or chain_depth deep without one.
        let tip = status["height"].as_u64().unwrap_or(0);
        let safe = match status["finality"]["finalized"]["height"].as_u64() {
            Some(finalized) => finalized,
            None => tip.saturating_sub(federation["chain_depth"].as_u64().unwrap_or(20)),
        };
        let left = ok(self.agent.get(format!("{}/events?contract={gateway}&name=Sent&limit=500", self.node)).call())
            .map_err(|err| format!("reading what left needs a node with --index-events: {err}"))?;

        for event in left["events"].as_array().into_iter().flatten() {
            if event["height"].as_u64().is_none_or(|height| height > safe) {
                continue;
            }

            let Some(nonce) = event["data"][0]["value"].as_str().and_then(|text| text.parse::<u64>().ok()) else {
                eprintln!("a Sent event without its nonce, at height {}", event["height"]);

                continue;
            };
            let what = format!("signing transfer {nonce} from here");

            if self.refused.borrow().due(&what) && !self.refused.borrow().is_done(&gateway, &what) {
                let result = self.left_here(&gateway, &identity, here, nonce).and_then(|transfer| self.post(&gateway, &transfer, &secret));
                self.settle_or_done(&gateway, what, result.map(|()| true));
            }
        }

        // What left each EVM chain, final there, and not yet arrived here.
        for url in &self.args.eth_rpc {
            if let Err(err) = self.sign_from_evm(url, &gateway, &identity, here, federation, from_block, &secret) {
                eprintln!("signing what left {url}: {err}");
            }
        }

        Ok(())
    }

    /// What left one EVM chain, final there, and not yet arrived here.
    #[allow(clippy::too_many_arguments)]
    fn sign_from_evm(&self, url: &str, gateway: &Address, identity: &[u8], here: u64, federation: &Value, from_block: u64, secret: &[u8; 32]) -> CliResult<()> {
        let rpc = Rpc::new(self.agent.clone(), url);
        let chain = rpc.chain_id()?;
        let evm_gateway = self.evm_gateway(gateway, chain)?;
        let (tip, finalized) = rpc.heads()?;
        let safe = finalized.unwrap_or_else(|| tip.saturating_sub(federation["evm_depth"].as_u64().unwrap_or(12)));
        let topic: [u8; 32] = Hasher::digest(HashType::Keccak256, b"Sent(uint64,uint64,bytes32)").to_bytes().try_into().expect("32 bytes");
        let mut from = from_block;

        while from <= safe {
            let to = (from + LOG_WINDOW - 1).min(safe);

            for log in rpc.logs(&evm_gateway, &topic, from, to)? {
                let Some(nonce) = log["topics"][1].as_str().and_then(from_hex).and_then(|word| u64::try_from(u256::from_be_bytes(word.try_into().ok()?)).ok()) else {
                    eprintln!("a Sent log without its nonce, from chain {chain}");

                    continue;
                };
                let what = format!("signing transfer {nonce} from chain {chain}");

                if self.refused.borrow().others.contains(&(chain, nonce)) || !self.refused.borrow().due(&what) || self.refused.borrow().is_done(gateway, &what) {
                    continue;
                }

                let result = self.sign_one_from_evm(&rpc, gateway, identity, &evm_gateway, here, chain, nonce, secret);
                self.settle_or_done(gateway, what, result.map(|()| true));
            }

            from = to + 1;
        }

        Ok(())
    }

    /// One transfer in an EVM chain's logs, signed if it is for this gateway and has not arrived.
    #[allow(clippy::too_many_arguments)]
    fn sign_one_from_evm(&self, rpc: &Rpc, gateway: &Address, identity: &[u8], evm_gateway: &[u8; 20], here: u64, chain: u64, nonce: u64, secret: &[u8; 32]) -> CliResult<()> {
        let at = json!([{ "type": "u64", "value": chain.to_string() }, { "type": "u64", "value": nonce.to_string() }]);
        let arrived = self.read(gateway, "received", at.clone())?;

        if arrived.first().is_some_and(|value| value["value"] == true) {
            return Ok(());
        }

        // Voided, as one refused for good is, it never arrives, and the gateway refuses a signature.
        if self.read(gateway, "voided", at)?.first().is_some_and(|value| !value["value"].is_null()) {
            return Ok(());
        }

        let values = rpc.call(evm_gateway, &calldata("sent(uint64)", &[AbiValue::Uint(u256::from(nonce))])?)?;
        let (_, types) = crate::abi::parse_signature(&format!("({})", crate::bridge::TRANSFER_TUPLE))?;
        // A name or URI that is not UTF-8 ends up here, from a gateway from before it dropped them.
        let transfer = decode_values(&types, &values).map_err(|err| format!("Gateway.sol's sent answered what does not read: {err}"))?;
        let transfer = Transfer::from_evm(&transfer[0]).ok_or("Gateway.sol's sent answered what is no transfer")?;

        // What the logs still hold from before a reset was for a chain or a gateway that is gone:
        // signing it could only be refused, or worse.
        if !transfer.arrives_at(here, identity, evm_gateway) {
            self.refused.borrow_mut().others.insert((chain, nonce));

            return Ok(());
        }

        self.post(gateway, &transfer, secret)
    }

    /// A transfer that left the gateway here, as its sent(nonce) gives it.
    fn left_here(&self, gateway: &Address, identity: &[u8], here: u64, nonce: u64) -> CliResult<Transfer> {
        let values = self.read(gateway, "sent", json!([{ "type": "u64", "value": nonce.to_string() }]))?;
        let text = |index: usize| values.get(index).and_then(|value| value["value"].as_str()).unwrap_or_default().to_string();
        let whole = |index: usize| text(index).parse::<u64>().map_err(|_| format!("sent({nonce}) gave no number at {index}"));
        let raw = |index: usize| -> Vec<u8> {
            let value = &values[index];

            match value["type"].as_str() {
                Some("bytes") => value["value"].as_str().and_then(from_hex).unwrap_or_default(),
                _ => value["value"].as_str().unwrap_or_default().as_bytes().to_vec(),
            }
        };
        let wide = |index: usize| u256::from_str_radix(&text(index), 10).map_err(|_| format!("sent({nonce}) gave no amount at {index}"));
        let target = whole(0)?;

        Ok(Transfer {
            from: identity.to_vec(),
            to: self.evm_gateway(gateway, target)?.to_vec(),
            source: here,
            target,
            nonce,
            kind: AssetKind::from_number(whole(1)?).ok_or("a kind the gateways do not have")?,
            origin: whole(2)?,
            asset: raw(3),
            id: wide(4)?,
            amount: wide(5)?,
            recipient: raw(6),
            name: text(7),
            symbol: text(8),
            decimals: u8::try_from(whole(9)?).map_err(|_| "decimals past 255")?,
            uri: text(10),
        })
    }

    /// A signature over a transfer posted to the board, unless it is there already.
    fn post(&self, gateway: &Address, transfer: &Transfer, secret: &[u8; 32]) -> CliResult<()> {
        let signature = transfer.sign(secret);
        let posted = self.read(gateway, "signatures", json!([{ "type": "bytes", "value": to_hex(&transfer.digest()) }]))?;

        if posted.iter().skip(1).any(|value| value["value"].as_str() == Some(&to_hex(&signature))) {
            return Ok(());
        }

        println!("signing transfer {} from chain {} to chain {}", transfer.nonce, transfer.source, transfer.target);

        self.call(*gateway, "attest", u256::ZERO, transfer.attest_args(&signature))
    }

    /// What left here for each EVM chain given, delivered once the board has enough signatures.
    /// As with signing, each transfer stands alone, and each chain.
    fn relay(&self) -> CliResult<()> {
        let secret = self.evm_secret()?;
        let gateway = self.gateway()?;
        let here = self.here()?;
        let identity = self.identity(&gateway)?;
        let place = match self.turns {
            Some(_) => (self.place()?, self.status()?["federation"]["signers"].as_array().map_or(0, Vec::len) as u64),
            None => (0, 0),
        };
        let count = self.read(&gateway, "sent_count", json!([]))?.first().and_then(|value| value["value"].as_str()?.parse::<u64>().ok()).unwrap_or(0);

        for url in &self.args.eth_rpc {
            let rpc = Rpc::new(self.agent.clone(), url);
            let chain = match rpc.chain_id() {
                Ok(chain) => chain,
                Err(err) => {
                    eprintln!("relaying to {url}: {err}");

                    continue;
                }
            };
            let evm_gateway = match self.evm_gateway(&gateway, chain) {
                Ok(evm_gateway) => evm_gateway,
                Err(err) => {
                    eprintln!("relaying to {url}: {err}");

                    continue;
                }
            };

            for nonce in 1..=count {
                let what = format!("delivering transfer {nonce} to chain {chain}");

                if self.refused.borrow().due(&what) && !self.refused.borrow().is_done(&gateway, &what) {
                    let result = self.deliver_one(&rpc, &gateway, &identity, &evm_gateway, here, chain, nonce, place, &secret);
                    self.settle_or_done(&gateway, what, result);
                }
            }
        }

        Ok(())
    }

    /// One transfer delivered, if it is for this chain, not yet there, and signed enough.
    #[allow(clippy::too_many_arguments)]
    fn deliver_one(&self, rpc: &Rpc, gateway: &Address, identity: &[u8], evm_gateway: &[u8; 20], here: u64, chain: u64, nonce: u64, (place, signers): (u64, u64), secret: &[u8; 32]) -> CliResult<bool> {
        let transfer = self.left_here(gateway, identity, here, nonce)?;

        if transfer.target != chain {
            return Ok(true);
        }

        let delivered = rpc.call(evm_gateway, &calldata("delivered(bytes32)", &[AbiValue::FixedBytes(transfer.delivered_key())])?)?;

        if delivered.last() == Some(&1) {
            if let Some(turns) = &self.turns {
                turns.ready.borrow_mut().remove(&(chain, nonce));
            }

            return Ok(true);
        }

        let posted = self.read(gateway, "signatures", json!([{ "type": "bytes", "value": to_hex(&transfer.digest()) }]))?;
        let threshold = posted.first().and_then(|value| value["value"].as_str()?.parse::<usize>().ok()).unwrap_or(usize::MAX);
        let signatures: Vec<[u8; 65]> = posted.iter().skip(1).filter_map(|value| value["value"].as_str().and_then(from_hex)?.try_into().ok()).collect();

        if signatures.len() < threshold {
            return Ok(false);
        }

        // Voided, as one refused for good is, its signatures stop counting there.
        if rpc.call(evm_gateway, &calldata("voidedAt(bytes32)", &[AbiValue::FixedBytes(transfer.delivered_key())])?)?.iter().any(|byte| *byte != 0) {
            return Ok(true);
        }

        // Its turn: the first goes at once, and each after it a turn after the one before, by when
        // this signer first found it ready, so the one before had the time to be mined there.
        if let Some(turns) = &self.turns {
            let since = *turns.ready.borrow_mut().entry((chain, nonce)).or_insert_with(std::time::Instant::now);

            if since.elapsed() < turns.turn * relay_turn(place, signers, nonce) as u32 {
                return Ok(false);
            }
        }

        println!("delivering transfer {nonce} to chain {chain}, from {}", to_hex(&from_key(secret)?));
        rpc.send(&Call { to: *evm_gateway, value: u256::ZERO, input: transfer.deliver_input(&signatures), what: "Deliver a transfer" }, false, secret)?;

        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_read_turned_away_as_too_many_is_asked_again_after_longer_and_longer_pauses() {
        let too_many = || -> CliResult<u32> { Err("the node refused (429): too many requests from this address: slow down and try again".into()) };

        // Turned away twice, then answered: the answer, after a second and two.
        let mut waited = Vec::new();
        let mut asked = 0;
        let answer = slowed(
            |pause| waited.push(pause.as_secs()),
            || {
                asked += 1;

                if asked < 3 { too_many() } else { Ok(7) }
            },
        );
        assert_eq!(answer.unwrap(), 7);
        assert_eq!(waited, [1, 2]);

        // Turned away every time: it gives up after SLOWED_TRIES, saying why.
        let mut waited = Vec::new();
        let answer = slowed(|pause| waited.push(pause.as_secs()), too_many);
        assert!(answer.unwrap_err().to_string().contains("429"));
        assert_eq!(waited, [1, 2, 4]);

        // Anything else is not asked again.
        let mut asked = 0;
        let answer: CliResult<u32> = slowed(
            |_| panic!("no pause"),
            || {
                asked += 1;

                Err("the node refused (400): no such function".into())
            },
        );
        assert!(answer.is_err());
        assert_eq!(asked, 1);
    }

    #[test]
    fn what_is_done_for_good_is_remembered_by_gateway_and_what_is_refused_is_not() {
        let mut refused = Refusals::default();
        let (one, other) = (Address::Contract([1; 32]), Address::Contract([2; 32]));
        let what = "delivering transfer 4 to chain 11155111".to_string();

        refused.settle(what.clone(), Err("not yet".into()));
        assert!(!refused.is_done(&one, &what));

        refused.settle(what.clone(), Ok(()));
        refused.done.insert((one, what.clone()));
        assert!(refused.is_done(&one, &what));
        assert!(refused.listed().is_empty());

        // The same nonce at a gateway set up anew, after a reset, is still to do.
        assert!(!refused.is_done(&other, &what));
    }

    #[test]
    fn the_first_turn_to_deliver_goes_round_the_signers_and_one_no_signers_goes_last() {
        // Three signers: transfer 1 is the second's to deliver first, then the third's, then the first's.
        assert_eq!([0, 1, 2].map(|place| relay_turn(place, 3, 1)), [2, 0, 1]);
        assert_eq!([0, 1, 2].map(|place| relay_turn(place, 3, 3)), [0, 1, 2]);

        // Every nonce has exactly one first, and each signer is first as often as the next.
        for nonce in 1..=30 {
            let mut turns = [0, 1, 2].map(|place| relay_turn(place, 3, nonce));
            turns.sort();
            assert_eq!(turns, [0, 1, 2], "{nonce}");
        }

        assert_eq!(relay_turn(3, 3, 7), 3);
        assert_eq!(relay_turn(0, 0, 7), 0);
    }

    #[test]
    fn an_age_limit_reads_as_a_number_and_its_unit() {
        assert_eq!(parse_age("6h"), Ok(21_600_000));
        assert_eq!(parse_age("90m"), Ok(5_400_000));
        assert_eq!(parse_age("30s"), Ok(30_000));
        assert_eq!(parse_age("2d"), Ok(172_800_000));
        assert_eq!(parse_age("0s"), Ok(0));

        for wrong in ["", "6", "h", "6x", "6é", "-1h", "99999999999999999999h"] {
            assert!(parse_age(wrong).is_err(), "{wrong}");
        }
    }

    #[test]
    fn a_gateway_a_signer_vouches_for_reads_as_chain_and_address() {
        assert_eq!(parse_evm_gateway(&format!("11155111:{}", to_hex(&[0xab; 20]))), Ok((11155111, [0xab; 20])));
        assert!(parse_evm_gateway("11155111").is_err());
        assert!(parse_evm_gateway("sepolia:0xabab").is_err());
        assert!(parse_evm_gateway(&format!("1:{}", to_hex(&[0xab; 19]))).is_err());
    }

    #[test]
    fn a_refused_transfer_sits_out_more_passes_each_time_and_stays_listed_until_it_goes_through() {
        let mut refusals = Refusals::default();
        let what = "signing transfer 1 from chain 5".to_string();
        let tried = |refusals: &mut Refusals, passes: u64| (0..passes).filter(|_| {
            let due = refusals.due("signing transfer 1 from chain 5");

            if due {
                refusals.settle("signing transfer 1 from chain 5".to_string(), Err("refused".into()));
            }

            refusals.next_pass();
            due
        }).count();

        assert!(refusals.due(&what), "nothing is held against it yet");
        // Tried in passes 0, 2, 5, 10, 19, 36, then every 65 passes at most.
        assert_eq!(tried(&mut refusals, 37), 6);
        assert_eq!(tried(&mut refusals, 65), 1, "at most one try in a long while");
        assert_eq!(refusals.listed(), ["signing transfer 1 from chain 5: refused"]);
        assert!(refusals.due("signing transfer 2 from chain 5"), "it holds up nothing else");

        // A try that goes through is forgotten, and the next refusal starts over.
        refusals.pass += 65;
        refusals.settle(what.clone(), Ok(()));
        assert!(refusals.listed().is_empty());
        refusals.settle(what.clone(), Err("refused".into()));
        assert_eq!(refusals.held[&what].sits, 1);
    }
}
