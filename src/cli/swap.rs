//! `chainv4 swap`: atomic swaps with Bitcoin from the command line. The terms go between the two
//! sides as codes, offer and answer; chainv4's side is the swap contract, which lock, claim and
//! refund send to; Bitcoin's side is an HTLC, which bitcoin-claim and bitcoin-refund spend,
//! printing the signed transaction to broadcast with any Bitcoin node or explorer.

use {
    super::send::{SendArgs, TlsArgs, TxArgs, agent, ok, send},
    super::{CliResult, parse_amount, parse_duration},
    crate::crypto::bitcoin::tx::{Network, Transaction},
    crate::crypto::keypair::{Keypair, PrivateKey, PublicKey},
    crate::structs::address::Address,
    crate::structs::bech32,
    crate::structs::hex::{from_hex, to_hex},
    crate::structs::tx::TxCallArgumentType,
    crate::structs::u256::u256,
    crate::swap::htlc::{Coins, Spend, revealed},
    crate::swap::{Counterparty, Pays, Terms, secret},
    serde_json::json,
    std::path::PathBuf,
};

#[derive(clap::Args)]
pub struct SwapArgs {
    #[command(subcommand)]
    pub action: SwapAction,
}

#[derive(clap::Subcommand)]
pub enum SwapAction {
    /// A new secret and its hash, to start a swap with. Keep the secret to yourself until you
    /// claim: whoever knows it can claim your side's lock.
    Secret,
    /// Offers a swap: its terms and your keys, as a code to hand the other side.
    Offer {
        /// What you pay: bitcoin, for coins or a token on chainv4, or chain, for bitcoin.
        #[arg(long, value_enum)]
        pays: PaysArg,
        /// The secret's hash, from swap secret: 0x and 64 hex digits.
        #[arg(long)]
        hash: String,
        /// The bitcoin, in satoshis.
        #[arg(long)]
        sats: u64,
        /// What is locked on chainv4, in its smallest units.
        #[arg(long, value_parser = parse_amount)]
        amount: u256,
        /// A token to lock on chainv4 instead of its coin.
        #[arg(long)]
        token: Option<Address>,
        /// On Bitcoin's test networks, testnet, signet or regtest.
        #[arg(long)]
        testnet: bool,
        /// Your Bitcoin key: its public key, bitcoin:0x… as keygen prints it, or a key file.
        #[arg(long)]
        bitcoin_key: String,
        /// Your chainv4 address: where you are paid, or what you lock from.
        #[arg(long)]
        address: Address,
        /// How long until your lock, the first, opens to a refund: 24h, 2d.
        #[arg(long, default_value = "24h", value_parser = parse_duration)]
        first: u64,
        /// How long until the other's lock does: at least 4h before the first.
        #[arg(long, default_value = "12h", value_parser = parse_duration)]
        second: u64,
        /// The swap contract the lock on chainv4 goes into: by default the one the name swap
        /// under the chain's suffix resolves to, such as swap.dev, or else the one the node names.
        #[arg(long)]
        contract: Option<Address>,
        /// The node to find the default contract at, when --contract is not given.
        #[arg(long, default_value = "http://127.0.0.1:8080")]
        node: String,
        #[command(flatten)]
        tls: TlsArgs,
    },
    /// Answers an offer with your keys: the agreed terms, as a code to hand back.
    Answer {
        /// The offer's code.
        offer: String,
        /// Your Bitcoin key: its public key or a key file.
        #[arg(long)]
        bitcoin_key: String,
        /// Your chainv4 address.
        #[arg(long)]
        address: Address,
    },
    /// Shows a swap's terms: who pays what and when, and the Bitcoin HTLC to pay.
    Show {
        /// The terms' code.
        terms: String,
    },
    /// Locks what the terms say on chainv4, as their chain side's payer: the coin, or a token,
    /// approving the swap contract to move it first.
    Lock(ChainAction),
    /// Claims what was locked on chainv4 for you, with the secret, which shows it on chainv4.
    Claim {
        #[command(flatten)]
        action: ChainAction,
        /// The secret: 0x and 64 hex digits.
        #[arg(long)]
        secret: String,
    },
    /// Takes back what you locked on chainv4, once its deadline passed and nobody claimed it.
    Refund(ChainAction),
    /// Where the swap stands on chainv4: locked, claimed, with its secret, or refunded.
    Status {
        terms: String,
        #[arg(long, default_value = "http://127.0.0.1:8080")]
        node: String,
        #[command(flatten)]
        tls: TlsArgs,
    },
    /// Claims the bitcoin locked for you, with the secret: prints the signed transaction.
    BitcoinClaim {
        #[command(flatten)]
        spend: BitcoinSpend,
        /// The secret: 0x and 64 hex digits.
        #[arg(long)]
        secret: String,
    },
    /// Takes back the bitcoin you locked, once its lock time passed: prints the signed
    /// transaction, which Bitcoin mines once its median time is past the deadline.
    BitcoinRefund(BitcoinSpend),
    /// The secret a Bitcoin claim of the swap showed, from that transaction's hex.
    Revealed {
        terms: String,
        /// The claim transaction, in hex, as an explorer or getrawtransaction gives it.
        #[arg(long)]
        tx: String,
    },
}

#[derive(Clone, Copy, clap::ValueEnum)]
pub enum PaysArg {
    Bitcoin,
    Chain,
}

/// What chainv4's side of a swap sends with.
#[derive(clap::Args)]
pub struct ChainAction {
    /// The terms' code: the swap contract is the one they name.
    pub terms: String,
    /// The node's HTTP API.
    #[arg(long, default_value = "http://127.0.0.1:8080")]
    pub node: String,
    #[command(flatten)]
    pub tls: TlsArgs,
    /// The key file to sign with.
    #[arg(long)]
    pub key: Option<PathBuf>,
}

/// What a Bitcoin spend of the HTLC needs.
#[derive(clap::Args)]
pub struct BitcoinSpend {
    /// The terms' code.
    pub terms: String,
    /// The HTLC's output: the funding transaction's id, as explorers write it, and its index,
    /// txid:vout.
    #[arg(long)]
    pub funding: String,
    /// What that output holds, in satoshis.
    #[arg(long)]
    pub sats: u64,
    /// Your bitcoin key file, from keygen --scheme bitcoin.
    #[arg(long)]
    pub key: PathBuf,
    /// Where the coins go: a bc1q… or tb1q… address.
    #[arg(long)]
    pub to: String,
    /// The fee, in satoshis a virtual byte.
    #[arg(long, default_value_t = 2)]
    pub fee_rate: u64,
}

pub fn swap(args: SwapArgs) -> CliResult<()> {
    let now = crate::node::now_ms();

    match args.action {
        SwapAction::Secret => {
            let (secret, hash) = secret()?;
            println!("secret {}", to_hex(&secret));
            println!("hash   {}", to_hex(&hash));
        }
        SwapAction::Offer { pays, hash, sats, amount, token, testnet, bitcoin_key, address, first, second, contract, node, tls } => {
            let contract = match contract {
                Some(contract) => contract,
                None => default_contract(&agent(&tls)?, node.trim_end_matches('/'))?,
            };
            let (starter, key) = (match pays { PaysArg::Bitcoin => Pays::Bitcoin, PaysArg::Chain => Pays::Chain }, bitcoin_public(&bitcoin_key)?);
            let terms = Terms {
                chain: if testnet { Counterparty::BitcoinTestnet } else { Counterparty::Bitcoin },
                contract,
                hash: bytes32(&hash, "--hash")?,
                sats,
                token,
                amount,
                starter,
                bitcoin_payer: (starter == Pays::Bitcoin).then_some(key),
                bitcoin_receiver: (starter == Pays::Chain).then_some(key),
                chain_payer: (starter == Pays::Chain).then_some(address),
                chain_receiver: (starter == Pays::Bitcoin).then_some(address),
                first_deadline: now + first,
                second_deadline: now + second,
            };
            terms.check(now)?;
            println!("{}", terms.to_text());
        }
        SwapAction::Answer { offer, bitcoin_key, address } => {
            let mut terms = Terms::from_text(&offer)?;
            let key = bitcoin_public(&bitcoin_key)?;

            if terms.agreed() {
                return Err("those terms are answered already".into());
            }

            match terms.starter {
                Pays::Bitcoin => (terms.bitcoin_receiver, terms.chain_payer) = (Some(key), Some(address)),
                Pays::Chain => (terms.bitcoin_payer, terms.chain_receiver) = (Some(key), Some(address)),
            }

            terms.check(now)?;
            println!("{}", terms.to_text());
        }
        SwapAction::Show { terms } => show(&Terms::from_text(&terms)?),
        SwapAction::Lock(action) => {
            let terms = agreed(&action.terms)?;
            let contract = terms.contract;
            let (hash, recipient, deadline) = (text(to_hex(&terms.hash)), TxCallArgumentType::Address(terms.chain_receiver.ok_or("no receiver")?), TxCallArgumentType::I64(terms.chain_deadline() as i64));

            match terms.token {
                None => call(&action, contract, "lock", terms.amount, vec![hash, recipient, deadline])?,
                Some(token) => {
                    // The contract moves the token itself, so it may first: the approval lands
                    // before the lock is tried.
                    call(&action, token, "approve", u256::ZERO, vec![TxCallArgumentType::Address(contract), TxCallArgumentType::U256(terms.amount)])?;
                    call(&action, contract, "lock_token", u256::ZERO, vec![hash, recipient, deadline, TxCallArgumentType::Address(token), TxCallArgumentType::U256(terms.amount)])?;
                }
            }
        }
        SwapAction::Claim { action, secret } => {
            let terms = agreed(&action.terms)?;
            let secret = bytes32(&secret, "--secret")?;

            if crate::crypto::bitcoin::sha256(&secret) != terms.hash {
                return Err("that secret's SHA-256 is not the swap's hash".into());
            }

            call(&action, terms.contract, "claim", u256::ZERO, vec![text(to_hex(&secret))])?;
        }
        SwapAction::Refund(action) => {
            let terms = agreed(&action.terms)?;
            call(&action, terms.contract, "refund", u256::ZERO, vec![text(to_hex(&terms.hash))])?;
        }
        SwapAction::Status { terms, node, tls } => {
            let terms = Terms::from_text(&terms)?;
            let agent = agent(&tls)?;
            let node = node.trim_end_matches('/');
            let contract = terms.contract;
            let body = json!({ "contract": contract.to_string(), "function": "get", "args": [{ "type": "string", "value": to_hex(&terms.hash) }] });
            let answer = ok(agent.post(format!("{node}/call")).header("content-type", "application/json").send(body.to_string()))?;
            let values: Vec<String> = answer["values"].as_array().into_iter().flatten().map(|value| value["value"].as_str().map_or_else(|| value["value"].to_string(), str::to_string)).collect();

            match values.as_slice() {
                [state, locker, recipient, token, amount, deadline, secret] => {
                    println!("{state}: {amount} of {token} from {locker} to {recipient}, deadline {deadline}");

                    if !secret.is_empty() {
                        println!("secret {secret}");
                    }
                }
                [state, ..] => println!("{state}"),
                [] => return Err(format!("the contract did not answer: {}", answer["error"]).into()),
            }
        }
        SwapAction::BitcoinClaim { spend, secret } => {
            let secret = bytes32(&secret, "--secret")?;
            println!("{}", bitcoin(&spend, Spend::Claim { secret })?);
        }
        SwapAction::BitcoinRefund(spend) => println!("{}", bitcoin(&spend, Spend::Refund)?),
        SwapAction::Revealed { terms, tx } => {
            let terms = Terms::from_text(&terms)?;
            let (tx, _) = Transaction::read(&from_hex(&format!("0x{}", tx.trim().trim_start_matches("0x"))).ok_or("--tx is not hex")?)?;
            let secret = revealed(&tx, &terms.hash).ok_or("that transaction shows no secret of this swap's")?;
            println!("secret {}", to_hex(&secret));
        }
    }

    Ok(())
}

/// The terms laid out, and the HTLC to pay on Bitcoin once both keys are in.
fn show(terms: &Terms) {
    let (what, deadline) = (|ms: u64| format!("{} ms since 1970", ms), terms.chain_deadline());
    let asset = terms.token.map_or_else(|| "the coin".to_string(), |token| format!("token {token}"));
    println!("{} sats on {} for {} of {asset} on chainv4", terms.sats, terms.network().name(), terms.amount);
    println!("starter pays {}", match terms.starter { Pays::Bitcoin => "bitcoin, and locks first", Pays::Chain => "on chainv4, and locks first" });
    println!("hash {}", to_hex(&terms.hash));
    println!("swap contract {}", terms.contract);
    println!("chainv4 lock ends {}", what(deadline));
    println!("bitcoin lock ends {}", what(terms.bitcoin_deadline()));

    match terms.htlc() {
        Some(htlc) => {
            println!("bitcoin HTLC {}", htlc.address(terms.network()));
            println!("its script {}", to_hex(&htlc.script()));
        }
        None => println!("an offer: the other side's keys are not in yet"),
    }
}

/// Terms both sides agreed.
fn agreed(text: &str) -> CliResult<Terms> {
    let terms = Terms::from_text(text)?;

    match terms.agreed() {
        true => Ok(terms),
        false => Err("an offer, not yet answered: both sides' keys must be in".into()),
    }
}

/// The swap contract a new offer uses: the one the name swap under the chain's suffix resolves
/// to, or else the one the node advertises.
pub(super) fn default_contract(agent: &ureq::Agent, node: &str) -> CliResult<Address> {
    let suffix = ok(agent.get(format!("{node}/names")).call()).ok().and_then(|names| names["suffix"].as_str().map(str::to_string));

    if let Some(contract) = suffix.and_then(|suffix| super::send::resolve(agent, node, &format!("{}.{suffix}", crate::swap::SWAP_NAME)).ok()) {
        return Ok(contract);
    }

    ok(agent.get(format!("{node}/status")).call())?["swap"]["contract"]
        .as_str()
        .ok_or("no swap contract: none resolves by name, nor does the node name one. Give --contract")?
        .parse()
        .map_err(|_| "the node named a swap contract that is not an address".into())
}

/// Sends a call through send, waiting for it to land.
fn call(action: &ChainAction, to: Address, function: &str, amount: u256, args: Vec<TxCallArgumentType>) -> CliResult<()> {
    send(SendArgs {
        node: action.node.clone(),
        tls: TlsArgs { ca_cert: action.tls.ca_cert.clone(), insecure: action.tls.insecure },
        key: action.key.clone(),
        no_wait: false,
        timeout: 120,
        confirmations: 1,
        finalized: false,
        gas_limit: None,
        anyway: false,
        eip712: false,
        max_price: None,
        tip: None,
        tx: TxArgs::Call { to, function: function.to_string(), amount, args },
    })
}

/// Bitcoin's side, spent: the signed transaction, as hex to broadcast.
pub(super) fn bitcoin(spend: &BitcoinSpend, how: Spend) -> CliResult<String> {
    let terms = agreed(&spend.terms)?;
    let htlc = terms.htlc().ok_or("the terms hold no HTLC")?;
    let (txid, vout) = spend.funding.split_once(':').ok_or("--funding is txid:vout")?;
    let mut txid: [u8; 32] = from_hex(&format!("0x{txid}")).and_then(|bytes| bytes.try_into().ok()).ok_or("--funding's txid is 64 hex digits")?;
    // Explorers write a txid backwards from how a transaction names it.
    txid.reverse();
    let vout: u32 = vout.parse().map_err(|_| "--funding's vout is a number")?;
    let key = Keypair::from_key_file(&std::fs::read_to_string(&spend.key)?)?;
    let PrivateKey::Bitcoin(secret) = key.private() else {
        return Err("--key is not a bitcoin key file: make one with keygen --scheme bitcoin".into());
    };
    let to = output_script(&spend.to, terms.network())?;
    let tx = htlc.spend(Coins { txid, vout, value: spend.sats }, to, spend.fee_rate, how, secret)?;

    Ok(to_hex(&tx.encode())[2..].to_string())
}

/// The script a native segwit address on the network pays.
fn output_script(address: &str, network: Network) -> CliResult<Vec<u8>> {
    let (hrp, program) = bech32::decode_segwit(address).ok_or("--to is a native segwit address: bc1q…, or tb1q… on testnet")?;

    if hrp != network.hrp() {
        return Err(format!("{address} is not on {}", network.name()).into());
    }

    Ok([&[0x00, program.len() as u8][..], &program].concat())
}

/// A Bitcoin public key, as text or from a key file.
fn bitcoin_public(given: &str) -> CliResult<[u8; 33]> {
    let key = match std::fs::read_to_string(given) {
        Ok(file) => Keypair::from_key_file(&file)?.public(),
        Err(_) => given.parse::<PublicKey>().map_err(|_| format!("{given} is neither a bitcoin public key nor a key file"))?,
    };

    match key {
        PublicKey::Bitcoin(bytes) => Ok(bytes),
        _ => Err("a Bitcoin key is a bitcoin one: bitcoin:0x…, from keygen --scheme bitcoin".into()),
    }
}

fn bytes32(text: &str, what: &str) -> CliResult<[u8; 32]> {
    from_hex(text)
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(|| format!("{what} is 32 bytes: 0x and 64 hex digits").into())
}

fn text(value: String) -> TxCallArgumentType {
    TxCallArgumentType::String(value)
}
