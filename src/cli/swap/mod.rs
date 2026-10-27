//! `chainv4 swap`: atomic swaps between any two chains from the command line. The terms go between
//! the two sides as codes, offer and answer, two legs each on chainv4, Bitcoin or an EVM chain.
//! chainv4's legs go through its swap contract, which lock, claim and refund send to; an EVM
//! chain's through Swap.sol there, by its JSON-RPC (--eth-rpc); Bitcoin's is an HTLC, paid from
//! any Bitcoin wallet, which bitcoin-claim and bitcoin-refund spend, printing the signed
//! transaction to broadcast with any Bitcoin node or explorer.

pub(super) mod rpc;

use {
    super::send::{SendArgs, TlsArgs, TxArgs, agent, ok, send},
    super::{CliResult, parse_amount, parse_duration, signer},
    crate::crypto::bitcoin::tx::{Network, Transaction},
    crate::crypto::keypair::{Keypair, PrivateKey, PublicKey},
    crate::structs::address::Address,
    crate::structs::bech32,
    crate::structs::hex::{from_hex, to_hex},
    crate::structs::tx::TxCallArgumentType,
    crate::structs::u256::u256,
    crate::swap::evm::{self, parse_address, show_address},
    crate::swap::htlc::{Coins, Spend, revealed},
    crate::swap::{BitcoinLeg, ChainLeg, EvmLeg, Item, Leg, Lock, LockState, Parties, Role, Settled, Side, Terms, secret},
    rpc::Rpc,
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
    /// Offers a swap: its two legs, what you pay and what you get, and who you are on each, as a
    /// code to hand the other side.
    Offer {
        /// The secret's hash, from swap secret: 0x and 64 hex digits.
        #[arg(long)]
        hash: String,
        /// What you pay, locked first: chainv4:AMOUNT[:TOKEN], bitcoin:SATS, bitcoin-testnet:SATS,
        /// or an EVM chain's, evm:CHAIN_ID:AMOUNT[:TOKEN] or by name, such as
        /// sepolia:10000000000000000. Amounts are in smallest units: wei, satoshis.
        #[arg(long)]
        pay: String,
        /// What you get, locked second by the other side, written the same way.
        #[arg(long)]
        get: String,
        #[command(flatten)]
        parties: PartyArgs,
        /// How long until your lock, the first, opens to a refund: 24h, 2d.
        #[arg(long, default_value = "24h", value_parser = parse_duration)]
        first: u64,
        /// How long until the other's lock does: at least 4h before the first.
        #[arg(long, default_value = "12h", value_parser = parse_duration)]
        second: u64,
        /// The swap contract a chainv4 leg's lock goes into: by default the one the name swap
        /// under the chain's suffix resolves to, such as swap.dev, or else the one the node names.
        #[arg(long)]
        contract: Option<Address>,
        /// The Swap.sol an EVM leg's lock goes into: by default where the deterministic
        /// deployment proxy puts it, the same on every chain.
        #[arg(long)]
        evm_contract: Option<String>,
        /// The node to find the default contract at, for a chainv4 leg without --contract.
        #[arg(long, default_value = "http://127.0.0.1:8080")]
        node: String,
        #[command(flatten)]
        tls: TlsArgs,
    },
    /// Answers an offer with who you are on its chains: the agreed terms, as a code to hand back.
    Answer {
        /// The offer's code.
        offer: String,
        #[command(flatten)]
        parties: PartyArgs,
    },
    /// Shows a swap's terms: who pays what on which chain and when, and the Bitcoin HTLC to pay.
    Show {
        /// The terms' code.
        terms: String,
    },
    /// Locks the leg you pay, on chainv4 or an EVM chain: the coin, or a token, approving the
    /// swap contract to move it first. Locking second, it first checks the starter's lock stands
    /// as the terms say, where it can see it.
    Lock(LegAction),
    /// Claims a leg with the secret, which shows it on that chain. Anyone may send it: what is
    /// locked goes to the leg's receiver.
    Claim {
        #[command(flatten)]
        action: LegAction,
        /// The secret: 0x and 64 hex digits.
        #[arg(long)]
        secret: String,
    },
    /// Takes a lock back to whoever made it, once its deadline passed and nobody claimed it.
    Refund(LegAction),
    /// Where each leg stands: locked, claimed with its secret, or refunded; Bitcoin's HTLC to look
    /// up.
    Status {
        terms: String,
        #[arg(long, default_value = "http://127.0.0.1:8080")]
        node: String,
        /// An EVM chain's JSON-RPC URL, for its leg: give one for each EVM chain in the swap.
        #[arg(long)]
        eth_rpc: Vec<String>,
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
    /// Deploys Swap.sol on an EVM chain: through the deterministic deployment proxy where the
    /// chain has it, at the address offers default to, or else wherever a create puts it.
    EvmDeploy {
        /// The chain's JSON-RPC URL.
        #[arg(long)]
        eth_rpc: String,
        /// A secp256k1 key file holding the chain's coin, to pay for it.
        #[arg(long)]
        key: PathBuf,
        #[command(flatten)]
        tls: TlsArgs,
    },
}

/// Who you are on a swap's chains: what its legs take of you.
#[derive(clap::Args)]
pub struct PartyArgs {
    /// Your chainv4 address, for a chainv4 leg: where you are paid, or what you lock from.
    #[arg(long)]
    pub address: Option<Address>,
    /// Your Bitcoin key, for a Bitcoin leg: its public key, bitcoin:0x… as keygen prints it, or a
    /// key file.
    #[arg(long)]
    pub bitcoin_key: Option<String>,
    /// Your address on EVM chains, for an EVM leg: 0x and 40 hex digits, or a secp256k1 key file.
    #[arg(long)]
    pub evm_address: Option<String>,
}

#[derive(Clone, Copy, clap::ValueEnum)]
pub enum LegArg {
    First,
    Second,
}

/// What acting on a leg on chainv4 or an EVM chain sends with.
#[derive(clap::Args)]
pub struct LegAction {
    /// The terms' code: the contracts are the ones they name.
    pub terms: String,
    /// The leg: by default the one the key pays, to lock or refund, or receives, to claim; or the
    /// only one not on Bitcoin.
    #[arg(long, value_enum)]
    pub leg: Option<LegArg>,
    /// The node's HTTP API, for a chainv4 leg.
    #[arg(long, default_value = "http://127.0.0.1:8080")]
    pub node: String,
    /// An EVM chain's JSON-RPC URL, for an EVM leg, and to check the other leg locking second.
    #[arg(long)]
    pub eth_rpc: Vec<String>,
    #[command(flatten)]
    pub tls: TlsArgs,
    /// The key file to sign with: a secp256k1 one for an EVM leg.
    #[arg(long)]
    pub key: Option<PathBuf>,
    /// Lock second without the starter's lock checked: when it is on Bitcoin, or out of reach.
    #[arg(long)]
    pub anyway: bool,
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
        SwapAction::Offer { hash, pay, get, parties, first, second, contract, evm_contract, node, tls } => {
            let (agent, node) = (agent(&tls)?, node.trim_end_matches('/').to_string());
            let mut chain_contract = || match contract {
                Some(contract) => Ok(contract),
                None => default_contract(&agent, &node),
            };
            let evm_contract = evm_contract.as_deref().map(parse_address).transpose()?.unwrap_or_else(evm::deployed_address);
            let pay = parse_leg(&pay, &mut chain_contract, evm_contract)?;
            let get = parse_leg(&get, &mut chain_contract, evm_contract)?;
            let terms = Terms::offer(bytes32(&hash, "--hash")?, pay, get, now + first, now + second, &parties.read()?)?;
            terms.check(now)?;
            println!("{}", terms.to_text());
        }
        SwapAction::Answer { offer, parties } => {
            let terms = Terms::from_text(&offer)?.answer(&parties.read()?)?;
            terms.check(now)?;
            println!("{}", terms.to_text());
        }
        SwapAction::Show { terms } => show(&Terms::from_text(&terms)?),
        SwapAction::Lock(action) => {
            let terms = agreed(&action.terms)?;
            let (keypair, parties) = keyed(&action)?;
            let side = pick(&terms, &action, &parties, Role::Payer, "lock")?;

            // The second lock goes in only once the first stands as the terms say, and is final.
            if side == Side::Second && !action.anyway {
                match look(&terms, Side::First, &action) {
                    Ok(lock) => {
                        terms.first.stands(&lock, terms.first_deadline).map_err(|why| format!("the starter's lock does not stand: {why}. Lock only once it does"))?;

                        if let settled @ (Settled::Waiting(_) | Settled::Unknown(_)) = settled(&terms, Side::First, &action, &lock)? {
                            return Err(format!("the starter's lock is {}. Lock once it is, or with --anyway", settled.describe()).into());
                        }
                    }
                    Err(why) => return Err(format!("cannot check the starter's lock: {why}. Check it yourself, then lock with --anyway").into()),
                }
            }

            match terms.leg(side) {
                Leg::Chain(leg) => {
                    let (hash, recipient, deadline) = (text(to_hex(&terms.hash)), TxCallArgumentType::Address(leg.receiver.ok_or("no receiver")?), TxCallArgumentType::I64(terms.deadline(side) as i64));

                    match (leg.token, leg.item) {
                        (None, _) => call(&action, leg.contract, "lock", leg.amount, vec![hash, recipient, deadline])?,
                        // An NFT moves as a token does: approved for the contract, then pulled in.
                        (Some(collection), Some(item)) if !item.multi => {
                            call(&action, collection, "approve", u256::ZERO, vec![TxCallArgumentType::Address(leg.contract), TxCallArgumentType::U256(item.id)])?;
                            call(&action, leg.contract, "lock_nft", u256::ZERO, vec![hash, recipient, deadline, TxCallArgumentType::Address(collection), TxCallArgumentType::U256(item.id)])?;
                        }
                        (Some(collection), Some(item)) => {
                            call(&action, collection, "set_approval_for_all", u256::ZERO, vec![TxCallArgumentType::Address(leg.contract), TxCallArgumentType::Bool(true)])?;
                            call(
                                &action,
                                leg.contract,
                                "lock_multi",
                                u256::ZERO,
                                vec![hash, recipient, deadline, TxCallArgumentType::Address(collection), TxCallArgumentType::U256(item.id), TxCallArgumentType::U256(leg.amount)],
                            )?;
                        }
                        (Some(token), None) => {
                            // The contract moves the token itself, so it may first: the approval
                            // lands before the lock is tried.
                            call(&action, token, "approve", u256::ZERO, vec![TxCallArgumentType::Address(leg.contract), TxCallArgumentType::U256(leg.amount)])?;
                            call(&action, leg.contract, "lock_token", u256::ZERO, vec![hash, recipient, deadline, TxCallArgumentType::Address(token), TxCallArgumentType::U256(leg.amount)])?;
                        }
                    }
                }
                Leg::Evm(leg) => {
                    let (rpc, secret) = (evm_rpc(&action, leg)?, evm_secret(&keypair)?);

                    for call in leg.lock_calls(&terms.hash, terms.deadline(side))? {
                        rpc.send(&call, false, &secret)?;
                    }
                }
                Leg::Bitcoin(_) => unreachable!("pick leaves Bitcoin's legs out"),
            }
        }
        SwapAction::Claim { action, secret } => {
            let terms = agreed(&action.terms)?;
            let secret = bytes32(&secret, "--secret")?;

            if crate::crypto::bitcoin::sha256(&secret) != terms.hash {
                return Err("that secret's SHA-256 is not the swap's hash".into());
            }

            let (keypair, parties) = keyed(&action)?;
            let side = pick(&terms, &action, &parties, Role::Receiver, "claim")?;

            // The starter's claim shows the secret: only once the second lock is final, which a
            // reorganization could otherwise take away with the secret already out.
            if side == Side::Second && !action.anyway {
                let lock = look(&terms, side, &action)?;

                if let settled @ (Settled::Waiting(_) | Settled::Unknown(_)) = settled(&terms, side, &action, &lock)? {
                    return Err(format!("their lock is {}. Claim once it is, or with --anyway", settled.describe()).into());
                }
            }

            match terms.leg(side) {
                Leg::Chain(leg) => call(&action, leg.contract, "claim", u256::ZERO, vec![text(to_hex(&secret))])?,
                Leg::Evm(leg) => _ = evm_rpc(&action, leg)?.send(&leg.claim_call(&secret), false, &evm_secret(&keypair)?)?,
                Leg::Bitcoin(_) => unreachable!("pick leaves Bitcoin's legs out"),
            }
        }
        SwapAction::Refund(action) => {
            let terms = agreed(&action.terms)?;
            let (keypair, parties) = keyed(&action)?;

            match terms.leg(pick(&terms, &action, &parties, Role::Payer, "refund")?) {
                Leg::Chain(leg) => call(&action, leg.contract, "refund", u256::ZERO, vec![text(to_hex(&terms.hash))])?,
                Leg::Evm(leg) => _ = evm_rpc(&action, leg)?.send(&leg.refund_call(&terms.hash), false, &evm_secret(&keypair)?)?,
                Leg::Bitcoin(_) => unreachable!("pick leaves Bitcoin's legs out"),
            }
        }
        SwapAction::Status { terms, node, eth_rpc, tls } => {
            let terms = Terms::from_text(&terms)?;
            let action = LegAction { terms: String::new(), leg: None, node, eth_rpc, tls, key: None, anyway: false };

            for (side, leg) in terms.legs() {
                let name = match side {
                    Side::First => "first",
                    Side::Second => "second",
                };

                if let (Leg::Bitcoin(leg), Some(htlc)) = (leg, terms.htlc()) {
                    println!("{name} leg, on {}: the HTLC {}, to look up on a Bitcoin explorer", leg.network().name(), htlc.address(leg.network()));
                    continue;
                }

                match look(&terms, side, &action) {
                    Ok(lock) => {
                        let state = match lock.state {
                            LockState::None => "nothing locked",
                            LockState::Locked => "locked",
                            LockState::Claimed => "claimed",
                            LockState::Refunded => "refunded",
                        };
                        let stands = match leg.stands(&lock, terms.deadline(side)) {
                            Ok(()) => "as the terms say".to_string(),
                            Err(why) => why,
                        };
                        let settled = match lock.state {
                            LockState::Locked | LockState::Claimed => format!(", {}", settled(&terms, side, &action, &lock).map_or_else(|why| format!("finality not known: {why}"), |settled| settled.describe())),
                            _ => String::new(),
                        };
                        println!("{name} leg, on {}: {state}, {stands}{settled}", leg.chain_name());

                        if let Some(secret) = lock.secret {
                            println!("secret {}", to_hex(&secret));
                        }
                    }
                    Err(why) => println!("{name} leg, on {}: not known, {why}", leg.chain_name()),
                }
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
        SwapAction::EvmDeploy { eth_rpc, key, tls } => {
            let secret = evm_secret(&signer(Some(&key))?)?;
            let address = Rpc::new(agent(&tls)?, &eth_rpc).deploy(&evm::init_code(), &secret)?;
            println!("Swap.sol at {}", show_address(&address));
        }
    }

    Ok(())
}

/// A leg from how --pay and --get write one.
fn parse_leg(text: &str, chain_contract: &mut dyn FnMut() -> CliResult<Address>, evm_contract: [u8; 20]) -> CliResult<Leg> {
    let parts: Vec<&str> = text.trim().split(':').collect();
    let bad = || {
        format!(
            "{text} is not a leg: chainv4:AMOUNT[:TOKEN], chainv4:nft:COLLECTION:ID, chainv4:multi:COLLECTION:ID:AMOUNT, bitcoin:SATS, bitcoin-testnet:SATS, \
             evm:CHAIN_ID:AMOUNT[:TOKEN], evm:CHAIN_ID:nft:COLLECTION:ID, evm:CHAIN_ID:multi:COLLECTION:ID:AMOUNT, or a chain's name for evm:CHAIN_ID, such as sepolia:AMOUNT"
        )
    };
    // An NFT's collection, id and, for a multi-token one, amount; a chainv4 collection holds a
    // colon of its own, contract:0x….
    let item = |multi: bool, rest: &[&str]| -> CliResult<(String, Item, u256)> {
        let (collection, numbers) = match (multi, rest) {
            (false, [collection @ .., id]) => (collection.join(":"), (*id, "1")),
            (true, [collection @ .., id, amount]) => (collection.join(":"), (*id, *amount)),
            _ => return Err(bad().into()),
        };

        Ok((collection, Item { id: parse_amount(numbers.0)?, multi }, parse_amount(numbers.1)?))
    };
    let sats = |text: &str| text.parse::<u64>().map_err(|_| format!("{text} is not a whole number of satoshis"));
    let evm = |chain_id: u64, rest: &[&str]| -> CliResult<Leg> {
        if let [kind @ ("nft" | "multi"), rest @ ..] = rest {
            let (collection, found, amount) = item(*kind == "multi", rest)?;

            return Ok(Leg::Evm(EvmLeg { chain_id, contract: evm_contract, token: Some(parse_address(&collection)?), item: Some(found), amount, payer: None, receiver: None }));
        }

        let (amount, token) = match rest {
            [amount] => (parse_amount(amount)?, None),
            [amount, token] => (parse_amount(amount)?, Some(parse_address(token)?)),
            _ => return Err(bad().into()),
        };

        Ok(Leg::Evm(EvmLeg { chain_id, contract: evm_contract, token, item: None, amount, payer: None, receiver: None }))
    };

    match parts.as_slice() {
        ["chainv4", kind @ ("nft" | "multi"), rest @ ..] => {
            let (collection, found, amount) = item(*kind == "multi", rest)?;
            let collection: Address = collection.parse().or_else(|_| format!("contract:{collection}").parse()).map_err(|_| format!("{collection} is not a collection's address"))?;

            Ok(Leg::Chain(ChainLeg { contract: chain_contract()?, token: Some(collection), item: Some(found), amount, payer: None, receiver: None }))
        }
        ["chainv4", amount] => Ok(Leg::Chain(ChainLeg { contract: chain_contract()?, token: None, item: None, amount: parse_amount(amount)?, payer: None, receiver: None })),
        ["chainv4", amount, token] => {
            // A chainv4 address holds a colon of its own, contract:0x….
            let token: Address = token.parse().or_else(|_| format!("contract:{token}").parse()).map_err(|_| format!("{token} is not a token's address"))?;

            Ok(Leg::Chain(ChainLeg { contract: chain_contract()?, token: Some(token), item: None, amount: parse_amount(amount)?, payer: None, receiver: None }))
        }
        ["chainv4", amount, "contract", token] => Ok(Leg::Chain(ChainLeg {
            contract: chain_contract()?,
            item: None,
            token: Some(format!("contract:{token}").parse().map_err(|_| format!("{token} is not a token's address"))?),
            amount: parse_amount(amount)?,
            payer: None,
            receiver: None,
        })),
        ["bitcoin", amount] => Ok(Leg::Bitcoin(BitcoinLeg { testnet: false, sats: sats(amount)?, payer: None, receiver: None })),
        ["bitcoin-testnet", amount] => Ok(Leg::Bitcoin(BitcoinLeg { testnet: true, sats: sats(amount)?, payer: None, receiver: None })),
        ["evm", chain_id, rest @ ..] => evm(chain_id.parse().map_err(|_| format!("{chain_id} is not a chain id"))?, rest),
        [name, rest @ ..] => match evm::CHAINS.iter().find(|(_, known, _)| known.to_ascii_lowercase().replace(' ', "-") == name.to_ascii_lowercase()) {
            Some((chain_id, ..)) => evm(*chain_id, rest),
            None => Err(bad().into()),
        },
        [] => Err(bad().into()),
    }
}

impl PartyArgs {
    fn read(&self) -> CliResult<Parties> {
        Ok(Parties {
            chain: self.address,
            bitcoin: self.bitcoin_key.as_deref().map(bitcoin_public).transpose()?,
            evm: self.evm_address.as_deref().map(evm_party).transpose()?,
        })
    }
}

/// An EVM address, as text or a secp256k1 key file's.
fn evm_party(given: &str) -> CliResult<[u8; 20]> {
    match std::fs::read_to_string(given) {
        Ok(file) => evm_secret(&Keypair::from_key_file(&file)?).and_then(|secret| rpc::from_key(&secret)),
        Err(_) => Ok(parse_address(given)?),
    }
}

/// The key a leg action signs with, and who it is on the swap's chains.
fn keyed(action: &LegAction) -> CliResult<(Keypair, Parties)> {
    let keypair = signer(action.key.as_deref())?;
    let evm = match keypair.private() {
        PrivateKey::Secp256k1(secret) => Some(rpc::from_key(secret)?),
        _ => None,
    };

    let parties = Parties { chain: Some(keypair.public().address()), bitcoin: None, evm };

    Ok((keypair, parties))
}

/// The leg to act on: the one given; or the one the key is in, as payer or receiver; or the only
/// one not on Bitcoin, whose contract anyone may send claims and refunds to.
fn pick(terms: &Terms, action: &LegAction, parties: &Parties, role: Role, what: &str) -> CliResult<Side> {
    let side = match action.leg {
        Some(LegArg::First) => Side::First,
        Some(LegArg::Second) => Side::Second,
        None => {
            let reachable: Vec<Side> = terms.legs().into_iter().filter(|(_, leg)| !matches!(leg, Leg::Bitcoin(_))).map(|(side, _)| side).collect();
            let mine = reachable.iter().copied().find(|side| terms.leg(*side).is(role, parties));

            match (mine, reachable.as_slice()) {
                (Some(side), _) => side,
                (None, [only]) if role == Role::Receiver || what == "refund" => *only,
                _ => return Err(format!("which leg to {what}? the key is in neither as its {}: give --leg", if role == Role::Payer { "payer" } else { "receiver" }).into()),
            }
        }
    };

    match terms.leg(side) {
        Leg::Bitcoin(_) => Err(format!("that leg is Bitcoin's: pay its HTLC from a Bitcoin wallet, or use swap bitcoin-claim and bitcoin-refund; there is no {what} to send").into()),
        _ => Ok(side),
    }
}

/// Whether a leg's lock is final, as its chain says: chainv4's by its finality, or its depth on a
/// chain without; an EVM chain's by its finalized block, or its depth.
fn settled(terms: &Terms, side: Side, action: &LegAction, lock: &Lock) -> CliResult<Settled> {
    match terms.leg(side) {
        Leg::Chain(_) => {
            let status = ok(agent(&action.tls)?.get(format!("{}/status", action.node.trim_end_matches('/'))).call())?;
            let tip = status["height"].as_u64().unwrap_or(0);
            let finalized = status["finality"].is_object().then(|| status["finality"]["finalized"]["height"].as_u64().unwrap_or(0));

            Ok(crate::swap::chain_settled(lock, tip, finalized))
        }
        Leg::Evm(leg) => {
            let (tip, finalized) = evm_rpc(action, leg)?.heads()?;

            Ok(crate::swap::evm_settled(lock, tip, finalized))
        }
        Leg::Bitcoin(_) => Ok(Settled::Unknown("Bitcoin's depth is for you to check".to_string())),
    }
}

/// A leg's lock as its contract shows it.
fn look(terms: &Terms, side: Side, action: &LegAction) -> CliResult<Lock> {
    match terms.leg(side) {
        Leg::Chain(leg) => {
            let (agent, node) = (agent(&action.tls)?, action.node.trim_end_matches('/'));
            let ask = |function: &str| -> CliResult<Vec<String>> {
                let body = json!({ "contract": leg.contract.to_string(), "function": function, "args": [{ "type": "string", "value": to_hex(&terms.hash) }] });
                let answer = ok(agent.post(format!("{node}/call")).header("content-type", "application/json").send(body.to_string()))?;
                let values: Vec<String> = answer["values"].as_array().into_iter().flatten().map(|value| value["value"].as_str().map_or_else(|| value["value"].to_string(), str::to_string)).collect();

                match values.is_empty() {
                    true => Err(format!("the contract did not answer {function}: {}", answer["error"]).into()),
                    false => Ok(values),
                }
            };
            let lock = Lock::from_chain(&ask("get")?)?;

            // Which NFT it holds, asked only of legs that lock one: contracts from before NFTs
            // have no item.
            match leg.item {
                Some(_) => Ok(lock.with_chain_item(&ask("item")?)?),
                None => Ok(lock),
            }
        }
        Leg::Evm(leg) => {
            let rpc = evm_rpc(action, leg)?;
            let mut lock = evm::read_lock(&rpc.call(&leg.contract, &evm::get_input(&terms.hash))?)?;

            if leg.item.is_some() {
                lock.item = evm::read_item(&rpc.call(&leg.contract, &evm::item_input(&terms.hash))?)?;
            }

            Ok(lock)
        }
        Leg::Bitcoin(_) => Err("Bitcoin's lock is an HTLC: look its address up on an explorer".into()),
    }
}

fn evm_rpc(action: &LegAction, leg: &EvmLeg) -> CliResult<Rpc> {
    Rpc::for_chain(&agent(&action.tls)?, &action.eth_rpc, leg.chain_id)
}

fn evm_secret(keypair: &Keypair) -> CliResult<[u8; 32]> {
    match keypair.private() {
        PrivateKey::Secp256k1(secret) => Ok(*secret),
        _ => Err("an EVM chain takes a secp256k1 key: make one with keygen --scheme secp256k1".into()),
    }
}

/// The terms laid out, and the HTLC to pay on Bitcoin once both keys are in.
fn show(terms: &Terms) {
    let when = |ms: u64| format!("{ms} ms since 1970");
    println!("hash {}", to_hex(&terms.hash));

    for (side, leg) in terms.legs() {
        let who = match side {
            Side::First => "the starter pays, locked first",
            Side::Second => "the other pays, locked second",
        };
        println!("{} on {}: {who}, until {}", leg.what(), leg.chain_name(), when(terms.deadline(side)));

        match leg {
            Leg::Chain(leg) => println!("  swap contract {}", leg.contract),
            Leg::Evm(leg) => println!("  Swap.sol {} on chain {}", show_address(&leg.contract), leg.chain_id),
            Leg::Bitcoin(_) => {}
        }
    }

    match (terms.agreed(), terms.htlc(), terms.bitcoin()) {
        (_, Some(htlc), Some((_, leg))) => {
            println!("bitcoin HTLC {}", htlc.address(leg.network()));
            println!("its script {}", to_hex(&htlc.script()));
        }
        (false, ..) => println!("an offer: the other side's part is not in yet"),
        _ => {}
    }
}

/// Terms both sides agreed.
fn agreed(text: &str) -> CliResult<Terms> {
    let terms = Terms::from_text(text)?;

    match terms.agreed() {
        true => Ok(terms),
        false => Err("an offer, not yet answered: both sides' parts must be in".into()),
    }
}

/// The swap contract a new offer uses: the one the chain's directory names, or else the one the
/// node advertises.
pub(super) fn default_contract(agent: &ureq::Agent, node: &str) -> CliResult<Address> {
    let status = ok(agent.get(format!("{node}/status")).call())?;

    status["directory"]["swap"]
        .as_str()
        .or_else(|| status["swap"]["contract"].as_str())
        .ok_or("no swap contract: the chain's directory names none, nor does the node. Give --contract")?
        .parse()
        .map_err(|_| "the node named a swap contract that is not an address".into())
}

/// Sends a call through send, waiting for it to land.
fn call(action: &LegAction, to: Address, function: &str, amount: u256, args: Vec<TxCallArgumentType>) -> CliResult<()> {
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

/// Bitcoin's leg, spent: the signed transaction, as hex to broadcast.
pub(super) fn bitcoin(spend: &BitcoinSpend, how: Spend) -> CliResult<String> {
    let terms = agreed(&spend.terms)?;
    let (htlc, (_, leg)) = (terms.htlc().ok_or("the terms hold no HTLC: no leg is on Bitcoin")?, terms.bitcoin().ok_or("no leg is on Bitcoin")?);
    let (txid, vout) = spend.funding.split_once(':').ok_or("--funding is txid:vout")?;
    let mut txid: [u8; 32] = from_hex(&format!("0x{txid}")).and_then(|bytes| bytes.try_into().ok()).ok_or("--funding's txid is 64 hex digits")?;
    // Explorers write a txid backwards from how a transaction names it.
    txid.reverse();
    let vout: u32 = vout.parse().map_err(|_| "--funding's vout is a number")?;
    let key = Keypair::from_key_file(&std::fs::read_to_string(&spend.key)?)?;
    let PrivateKey::Bitcoin(secret) = key.private() else {
        return Err("--key is not a bitcoin key file: make one with keygen --scheme bitcoin".into());
    };
    let to = output_script(&spend.to, leg.network())?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legs_are_read_as_the_flags_write_them_nfts_included() {
        let contract = Address::Contract([0x5a; 32]);
        let collection = Address::Contract([0xc1; 32]);
        let evm_contract = [0xe0; 20];
        let read = |text: &str| parse_leg(text, &mut || Ok(contract), evm_contract);
        let chain = |leg: Leg| match leg {
            Leg::Chain(leg) => leg,
            other => panic!("{other:?}"),
        };
        let on_evm = |leg: Leg| match leg {
            Leg::Evm(leg) => leg,
            other => panic!("{other:?}"),
        };

        let coin = chain(read("chainv4:5000").unwrap());

        assert_eq!((coin.token, coin.item, coin.amount), (None, None, u256::new(5000)));
        let nft = chain(read(&format!("chainv4:nft:{collection}:7")).unwrap());

        assert_eq!((nft.token, nft.item, nft.amount), (Some(collection), Some(Item { id: u256::new(7), multi: false }), u256::new(1)));
        let bare = chain(read(&format!("chainv4:multi:{}:7:20", collection.to_string().trim_start_matches("contract:"))).unwrap());

        assert_eq!((bare.token, bare.item, bare.amount), (Some(collection), Some(Item { id: u256::new(7), multi: true }), u256::new(20)), "a collection's hex alone");

        let erc721 = on_evm(read("sepolia:nft:0x00000000000000000000000000000000000000c1:3").unwrap());

        assert_eq!((erc721.chain_id, erc721.token, erc721.item), (11155111, Some({ let mut a = [0; 20]; a[19] = 0xc1; a }), Some(Item { id: u256::new(3), multi: false })));
        let erc1155 = on_evm(read("evm:1:multi:0x00000000000000000000000000000000000000c1:3:40").unwrap());

        assert_eq!((erc1155.amount, erc1155.item.map(|item| item.multi)), (u256::new(40), Some(true)));

        for bad in ["chainv4:nft:contract:0xc1", "chainv4:multi:x:1", "evm:1:nft:3", "dogecoin:5"] {
            assert!(read(bad).is_err(), "{bad}");
        }
    }
}
