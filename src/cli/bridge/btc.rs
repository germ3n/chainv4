//! `chainv4 bridge btc`: Bitcoin's side of the bridges. The light client contract
//! (contracts/examples/btc_relay) is started at a checkpoint and kept in sync from an Esplora API;
//! the Bitcoin gateway (contracts/lua/btc_gateway.lua) mints wBTC for deposits shown with their
//! proofs, burns it for withdrawals, and pays them out through releases proposed and signed by the
//! federation input by input, assembled, broadcast and proven here. A signer's pass does all of that
//! by itself, every step standing alone, so withdrawals are paid with no one at a keyboard.

use {
    super::Bridge,
    crate::bridge::{
        BTC_CHILD_DOMAIN, BTC_DUST, BTC_REPLACE_DOMAIN, BTC_SEQUENCE, BumpKind, NamedWithdrawal, ReleaseFacts, ReleaseFee, ReleaseRefusal, ReleaseRepay, ReleaseStanding, ReleaseStep, SpentOutput, btc_block_ms, btc_bump_digest, btc_bump_kind, btc_cancel_digest, btc_fee_and_vbytes, btc_is_stuck, btc_may_propose_again,
        btc_most_fee_rate, btc_propose_digest, btc_release_rate, btc_release_step, btc_release_vbytes, btc_stuck_warning, btc_turn_delay, check_btc_child, check_btc_release, check_btc_replacement, plan_btc_child, plan_btc_child_replacement, plan_btc_release, plan_btc_replacement, sign_digest,
    },
    crate::registry::federation::BtcPolicy,
    crate::cli::CliResult,
    crate::crypto::bitcoin::sha256d,
    crate::crypto::bitcoin::tx::{Transaction, TxIn, TxOut, der, p2wsh},
    crate::structs::address::Address,
    crate::structs::hex::{from_hex, to_hex},
    crate::structs::tx::TxCallArgumentType,
    crate::structs::u256::u256,
    serde_json::{Value, json},
};

/// How soon a release proposed from the queue should be mined, in blocks: what its fee rate is the
/// network's for, and what a bump pays when it is not.
const SOON_BLOCKS: u32 = 3;

mod anchor;

#[derive(clap::Subcommand)]
pub enum BtcAction {
    /// Anchors the chain into Bitcoin (docs/anchoring.md): writes its newest finalized checkpoint
    /// into an OP_RETURN from the key's coins every anchoring.every epochs, and shows it to the
    /// chain once the relay holds it deep enough, which pays the reward. With anchoring.ethereum
    /// on, --eth-rpc and --evm-key, writes it to that chain too.
    Anchor {
        /// The secp256k1 key whose native segwit coins pay for anchors: --evm-key's unless given.
        #[arg(long)]
        btc_key: Option<std::path::PathBuf>,
        /// Seconds between passes; 0 for one pass.
        #[arg(long, default_value_t = 60)]
        every: u64,
        /// The rate to pay, in satoshis a virtual byte: what Esplora asks for within a few blocks
        /// unless given.
        #[arg(long)]
        fee_rate: Option<u64>,
    },
    /// Starts the relay, deployed from contracts/examples/btc_relay, at the header Esplora has at
    /// a height: on main, a difficulty period's first, a height divisible by 2016.
    RelayStart {
        relay: Address,
        height: u64,
        /// main, regtest or test (testnet4, testnet3 and signet): which rules its headers keep.
        /// Without it, the one Esplora serves, by its genesis block; refused if Esplora serves
        /// another.
        #[arg(long)]
        network: Option<String>,
    },
    /// Submits to the relay every header Esplora has past the relay's tip, a batch a
    /// transaction. Esplora is asked at an even pace and asked again, waiting longer each time,
    /// when it throttles; a batch it stops answering midway still submits what it fetched.
    RelaySync {
        relay: Address,
        /// Headers a transaction: small, so each keeps what it fetched.
        #[arg(long, default_value_t = 25)]
        batch: usize,
    },
    /// Deploys the Bitcoin gateway, with wBTC's template, on a relay, printing where.
    Deploy {
        relay: Address,
        /// The most fee a release may pay, in satoshis.
        #[arg(long, default_value_t = 50_000)]
        fee_cap: u64,
        /// The least fee a release may pay, in satoshis a virtual byte: a release proposed for less
        /// would hold what it spends without being mined. The cap must pay 710 virtual bytes of it.
        #[arg(long, default_value_t = 2)]
        fee_floor: u64,
        /// How long an old federation's deposit addresses stay open, in hours.
        #[arg(long, default_value_t = 168)]
        grace_hours: u64,
    },
    /// An address's deposit address on Bitcoin, under the federation in force, written for the
    /// Bitcoin the gateway's relay follows: bc1q… on main, tb1q… on the test networks, bcrt1q…
    /// on regtest. Which one, and what said so, goes to stderr.
    Address {
        recipient: Address,
        /// main, test or regtest: used only when the relay is not started and Esplora (--esplora)
        /// is not given or serves a Bitcoin it does not know; refused if either says another. Main
        /// when nothing says.
        #[arg(long)]
        network: Option<String>,
    },
    /// Shows a deposit, by its transaction and output, minting its wBTC to the address it pays.
    Deposit { txid: String, vout: u32, recipient: Address },
    /// Burns wBTC to be paid out to a Bitcoin address, bc1q…, or an output script, 0x…. What arrives
    /// is the amount less its share of its release's fee. An address must be on the Bitcoin the
    /// gateway's relay follows: tb1q… on a test network, bcrt1q… on regtest.
    Withdraw {
        address: String,
        sats: u64,
        /// The most it may lose to that share, in satoshis: a tenth of it unless told.
        #[arg(long)]
        most_fee: Option<u64>,
        /// main, test or regtest, as deposit-address takes it: used only when neither the relay nor
        /// Esplora says which Bitcoin the gateway pays out on.
        #[arg(long)]
        network: Option<String>,
    },
    /// Changes the most a queued withdrawal of yours may lose to fees, as fees rise past it.
    SetMostFee { withdrawal: u64, most_fee: u64 },
    /// A federation signer, with --evm-key, proposes a release paying the queued withdrawals from
    /// what the gateway holds. Its fee comes off them evenly, and one that agreed to lose less than
    /// its share waits. A pass does this by itself; this is for a fee told by hand.
    Propose {
        /// The fee in satoshis: what Esplora says the network asks, for the release's size, unless
        /// told.
        #[arg(long)]
        fee: Option<u64>,
    },
    /// A federation signer, with --evm-key, proposes a sweep of everything older federations hold
    /// to the one in force, with this fee.
    Sweep {
        #[arg(long)]
        fee: u64,
    },
    /// A federation signer's signing, with --evm-key: goes over every open release against the
    /// gateway's state, and signs every input of those that stand, at no more than --max-fee-rate
    /// allows.
    Sign,
    /// One pass of what pays withdrawals: with a signer's --evm-key, proposes what is queued, signs
    /// what stands, and votes to cancel what it refuses for good or what is still short of its
    /// signatures past --cancel-after; with or without one, broadcasts what has its signatures, shows
    /// what Bitcoin mined, and warns of what has all its signatures and is not mined past that limit.
    /// Each step stands alone, and what cannot go now waits for the next pass.
    Pass,
    /// A pass, then again after a pause, until stopped: what a signer leaves running.
    Watch {
        /// Seconds between passes.
        #[arg(long, default_value_t = 60)]
        every: u64,
    },
    /// A federation signer's vote to cancel a release with an input under its threshold of
    /// signatures, with --evm-key; once enough voted, its inputs and withdrawals go back.
    Cancel { proposal: u64 },
    /// Assembles an open release's witnesses from the signatures posted, and broadcasts it.
    Finish { proposal: u64 },
    /// Shows a release confirmed on Bitcoin, spending its inputs and holding its change.
    Released { proposal: u64 },
    /// Bumps a release with all its signatures that Bitcoin has not mined, sooner than a pass would:
    /// a federation signer, with --evm-key, proposes a replacement of its newest version paying more,
    /// or with --child a child paying for the version Bitcoin holds. The other signers sign it as a
    /// pass does, only within their limits and while their Esplora says it is stuck.
    Bump {
        /// Any proposal of the release: the release, a replacement or a child.
        release: u64,
        /// What it should pay a virtual byte, a child with what it pays for: what Esplora says the
        /// network asks to mine it within 3 blocks unless told.
        #[arg(long)]
        rate: Option<u64>,
        /// A child spending the change of the version Bitcoin holds, rather than a replacement.
        #[arg(long)]
        child: bool,
    },
}

/// A proposal of a release's tree, as the gateway gives it.
#[derive(Clone, Debug)]
pub(crate) struct Member {
    id: u64,
    found: Vec<Value>,
    state: String,
    kind: String,
    /// What it replaced, or for a child what it pays for.
    of: Option<u64>,
    /// The first of its group of versions: the release's own number, or a child's.
    group: u64,
    fee: u64,
}

impl Member {
    /// Its transaction's id, as explorers write it.
    fn txid(&self) -> String {
        shown(&raw(self.found.get(1).unwrap_or(&Value::Null)))
    }
}

/// What a member of a release's tree pays for, if it is a child or a replacement of one: the version
/// of the release its group's first spends the change of.
fn paid_for(members: &[Member], member: &Member) -> Option<u64> {
    let root = members.first()?.id;

    members.iter().find(|head| head.id == member.group).and_then(|head| head.of).filter(|_| member.group != root)
}

/// What a signer's step waits its turn to send: one call any one signer's does for all, so each
/// after the first checks again that it is still undone.
#[derive(Clone, Copy, Debug)]
enum Turn {
    /// released, for this proposal.
    Show(u64),
    /// A vote to cancel this proposal, needed while fewer than the threshold voted.
    Cancel(u64),
    /// A proposal of what is queued, worked out when this many were proposed.
    Propose(u64),
    /// A bump, worked out when this many were proposed.
    Bump(u64),
}

/// A transaction without its witness, its block, its place in it, and its Merkle path there; and
/// the block's coinbase the same way, which shows how tall its tree is.
struct Proof {
    tx: Vec<u8>,
    block: [u8; 32],
    index: u64,
    path: Vec<u8>,
    coinbase: Vec<u8>,
    coinbase_path: Vec<u8>,
}

/// Bitcoin's byte order for a hash written as explorers write it: reversed.
fn internal(display: &str) -> CliResult<[u8; 32]> {
    let mut bytes: [u8; 32] = from_hex(&format!("0x{}", display.trim().trim_start_matches("0x"))).and_then(|bytes| bytes.try_into().ok()).ok_or_else(|| format!("{display} is not a 32-byte hash"))?;
    bytes.reverse();

    Ok(bytes)
}

fn shown(internal: &[u8]) -> String {
    let mut bytes = internal.to_vec();
    bytes.reverse();

    to_hex(&bytes).trim_start_matches("0x").to_string()
}

/// Which Bitcoin a relay follows, by the names its start takes: main, test (testnet4, testnet3 and
/// signet, which share their addresses and the relay's loose rules) or regtest.
/// Which Bitcoin the gateway's relay follows: the library's, which the wallet writes and reads
/// addresses by too.
pub(crate) use crate::bridge::BtcNetwork as Followed;

/// Bitcoin's genesis blocks as explorers write their hashes, and whose they are.
const GENESIS_BLOCKS: [(&str, Followed); 5] = [
    ("000000000019d6689c085ae165831e934ff763ae46a2a6c172b3f1b60a8ce26f", Followed::Main),
    ("00000000da84f2bafbbc53dee25a72ae507ff4914b867c565be350b0da8bf043", Followed::Test),
    ("000000000933ea01ad0ee984209779baaec3ced90fa3f408719526f8d77f4943", Followed::Test),
    ("00000008819873e925422c1ff0f99f7cc9bbb232af63a077a480a3633bee1ef6", Followed::Test),
    ("0f9188f13cb7b2c71f2a335e3a4fc328bf5beb436012afca590b1a11466e2206", Followed::Regtest),
];

impl Followed {
    /// A network named on the command line: main, test or regtest.
    pub(crate) fn named(name: &str) -> CliResult<Followed> {
        Followed::from_name(name).ok_or_else(|| format!("{} is not a network: main, test or regtest", name.trim()).into())
    }

    /// The network whose genesis block has this hash, as Esplora's /block-height/0 answers it.
    pub(crate) fn from_genesis(hash: &str) -> Option<Followed> {
        GENESIS_BLOCKS.iter().find(|(known, _)| known.eq_ignore_ascii_case(hash.trim())).map(|(_, network)| *network)
    }
}

/// Which Bitcoin to write addresses for, and what said so: the relay as it was started, else
/// Esplora's genesis block, else what was told, else main. One told that the relay or Esplora
/// contradicts is refused, rather than write an address no deposit here would be proven from.
pub(crate) fn pick_network(relay: Option<&str>, esplora: Option<&str>, told: Option<&str>) -> CliResult<(Followed, String)> {
    let told = told.map(Followed::named).transpose()?;
    let (found, why) = match (relay.map(Followed::named).transpose()?, esplora.and_then(Followed::from_genesis)) {
        (Some(network), _) => (Some(network), "the relay follows it"),
        (None, Some(network)) => (Some(network), "the relay is not started, and Esplora serves it"),
        (None, None) => (None, ""),
    };

    match (found, told) {
        (Some(found), Some(told)) if found != told => Err(format!("--network {} is not the Bitcoin here: {}, as {why}", told.name(), found.name()).into()),
        (Some(found), _) => Ok((found, why.to_string())),
        (None, Some(told)) => Ok((told, "--network says so, and neither the relay nor Esplora does".to_string())),
        (None, None) => Ok((Followed::Main, "nothing says which, as the relay is not started and Esplora was not asked or did not say: give --network test or regtest for another".to_string())),
    }
}

/// A read's value as text, a number, or bytes.
fn text(value: &Value) -> String {
    value["value"].as_str().map_or_else(|| value["value"].to_string(), str::to_string)
}

pub(super) fn raw(value: &Value) -> Vec<u8> {
    match value["type"].as_str() {
        Some("bytes") => value["value"].as_str().and_then(from_hex).unwrap_or_default(),
        _ => value["value"].as_str().unwrap_or_default().as_bytes().to_vec(),
    }
}

/// How long between two questions to Esplora, so a sync does not burst.
const ESPLORA_PACE: std::time::Duration = std::time::Duration::from_millis(150);

/// How many times Esplora is asked before a question fails: waiting 2, 4, 8, 16 and 32 seconds
/// between, about a minute in all.
const ESPLORA_TRIES: u32 = 6;

/// An answer, asked again while it fails or is a web page, which is how a throttled Esplora
/// answers, waiting twice as long each time.
fn asked_again(tries: u32, mut wait: impl FnMut(std::time::Duration), mut ask: impl FnMut() -> Result<String, String>) -> Result<String, String> {
    let mut last = String::new();

    for attempt in 0..tries.max(1) {
        if attempt > 0 {
            wait(std::time::Duration::from_secs(1 << attempt));
        }

        match ask() {
            Ok(answer) if !answer.trim_start().starts_with('<') => return Ok(answer),
            Ok(_) => last = "Esplora answered a web page, as it does when it throttles".to_string(),
            Err(err) => last = err,
        }
    }

    Err(format!("{last}; asked {} times", tries.max(1)))
}

fn whole_size(value: &Value) -> usize {
    whole(value) as usize
}

fn whole(value: &Value) -> u64 {
    text(value).parse().unwrap_or(0)
}

/// What the network asks for a release to be mined, as Esplora says it, in satoshis a virtual byte.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct FeeRates {
    /// For the next block: what a signer's limit is reckoned from.
    fastest: f64,
    /// For within SOON_BLOCKS: what a release proposed from the queue pays.
    soon: f64,
}

/// What an answer from /fee-estimates comes to: its body for any success, as mempool.space answers
/// these with 203; no estimates for a 404, as regtest's Esplora has none; and otherwise why not.
fn fee_answer(status: u16, body: impl FnOnce() -> Result<String, String>) -> Result<String, String> {
    match status {
        404 => Ok("{}".to_string()),
        200..=299 => body(),
        status => Err(format!("Esplora /fee-estimates: it answered {status}")),
    }
}

/// Esplora's /fee-estimates, a rate by how many blocks it is for: None for no rate at all, as a
/// regtest one answers, and an error for an answer that is no such thing.
fn fee_rates_from(answer: &str) -> Result<Option<FeeRates>, String> {
    let estimates: std::collections::BTreeMap<String, f64> = serde_json::from_str(answer).map_err(|_| "Esplora gave fee estimates that do not read".to_string())?;
    let rates: std::collections::BTreeMap<u32, f64> = estimates.into_iter().filter_map(|(blocks, rate)| Some((blocks.parse().ok()?, rate))).filter(|(_, rate)| rate.is_finite() && *rate > 0.0).collect();
    let Some(fastest) = rates.values().next().copied() else {
        return Ok(None);
    };

    Ok(Some(FeeRates { fastest, soon: rates.range(SOON_BLOCKS..).next().map_or(fastest, |(_, rate)| *rate) }))
}

/// How far a pass has found nothing left to do: what is paid, spent or released stays so, so a
/// signer running for months does not read its whole history every pass.
#[derive(Clone, Copy, Default)]
pub(super) struct BtcDone {
    withdrawals: u64,
    outputs: u64,
    releases: u64,
}

/// The path in Esplora's Merkle proof, each sibling in Bitcoin's own byte order.
fn merkle_path(proof: &Value) -> CliResult<Vec<u8>> {
    let path = proof["merkle"].as_array().into_iter().flatten().map(|sibling| internal(sibling.as_str().unwrap_or_default())).collect::<CliResult<Vec<_>>>()?;

    Ok(path.concat())
}

impl Bridge {
    /// The newest block at or below a height that the relay's main chain and Esplora agree on. Bitcoin
    /// reorganizing leaves the relay's tip on a branch that lost, and headers from just past its height
    /// build on nothing it holds, so a sync starts from where the two last agreed.
    fn last_shared(&self, relay: &Address, mut height: u64) -> CliResult<u64> {
        loop {
            let held = self.read(relay, "block_at", json!([{ "type": "u64", "value": height.to_string() }]))?;
            let held = held.first().map(raw).filter(|hash| !hash.is_empty()).ok_or("the relay and Esplora share no block: start a relay from a newer checkpoint")?;

            if shown(&held) == self.esplora(&format!("/block-height/{height}"))?.trim() {
                return Ok(height);
            }

            log::info!("the relay's block at {height} is not Esplora's: looking further back");
            height = height.checked_sub(1).ok_or("the relay and Esplora share no block: start a relay from a newer checkpoint")?;
        }
    }

    /// Asks Esplora, at an even pace, and again when it throttles: a public one such as
    /// mempool.space answers a burst with an error or a web page, and a moment later answers again.
    fn esplora(&self, path: &str) -> CliResult<String> {
        let base = self.args.esplora.as_deref().ok_or("give --esplora: an Esplora API, such as https://mempool.space/api")?;
        let url = format!("{}{path}", base.trim_end_matches('/'));

        Ok(asked_again(ESPLORA_TRIES, std::thread::sleep, || {
            std::thread::sleep(ESPLORA_PACE);
            let mut answer = self.agent.get(&url).call().map_err(|err| format!("Esplora {path}: {err}"))?;

            answer.body_mut().read_to_string().map_err(|err| format!("Esplora {path}: {err}"))
        })?)
    }

    /// The Bitcoin gateway here: given, or the one the chain announces.
    fn btc_gateway(&self) -> CliResult<Address> {
        match self.args.btc_gateway {
            Some(gateway) => Ok(gateway),
            None => self.announced("btc_gateway")?.ok_or_else(|| "no Bitcoin gateway: the chain's directory names none. Give --btc-gateway".into()),
        }
    }

    /// A transaction as the gateway takes it, without its witness, with its block, place and
    /// Merkle path, as Esplora proves it.
    fn proof(&self, txid: &str) -> CliResult<Proof> {
        let (tx, proof) = self.placed(txid)?;
        let height = proof["block_height"].as_u64().ok_or("the transaction is not in a block yet")?;
        let hash = self.esplora(&format!("/block-height/{height}"))?;
        let (coinbase, of_coinbase) = self.placed(self.esplora(&format!("/block/{}/txid/0", hash.trim()))?.trim())?;

        Ok(Proof { tx, block: internal(&hash)?, index: proof["pos"].as_u64().unwrap_or(0), path: merkle_path(&proof)?, coinbase, coinbase_path: merkle_path(&of_coinbase)? })
    }

    /// A transaction without its witness, and Esplora's Merkle proof of it.
    fn placed(&self, txid: &str) -> CliResult<(Vec<u8>, Value)> {
        let hex = self.esplora(&format!("/tx/{txid}/hex"))?;
        let (tx, _) = Transaction::read(&from_hex(&format!("0x{}", hex.trim())).ok_or("Esplora gave a transaction that is not hex")?)?;
        let proof = serde_json::from_str(&self.esplora(&format!("/tx/{txid}/merkle-proof"))?).map_err(|_| "Esplora gave a proof that is not JSON")?;

        Ok((tx.base(), proof))
    }

    pub(super) fn btc(&self, action: &BtcAction) -> CliResult<()> {
        let bytes = TxCallArgumentType::Bytes;
        let number = TxCallArgumentType::U64;

        match action {
            BtcAction::RelayStart { relay, height, network } => {
                let served = Followed::from_genesis(&self.esplora("/block-height/0")?);
                let network = match (network.as_deref().map(Followed::named).transpose()?, served) {
                    (Some(told), Some(served)) if told != served => return Err(format!("--network {} is not what Esplora serves: {}", told.name(), served.name()).into()),
                    (Some(network), _) | (None, Some(network)) => network,
                    (None, None) => return Err("Esplora serves a Bitcoin whose genesis block is none of main's, test's or regtest's: give --network".into()),
                };
                eprintln!("starting the relay on Bitcoin's {} network", network.name());
                let hash = self.esplora(&format!("/block-height/{height}"))?;
                let header = from_hex(&format!("0x{}", self.esplora(&format!("/block/{}/header", hash.trim()))?.trim())).ok_or("Esplora gave a header that is not hex")?;
                self.call(*relay, "start", u256::ZERO, vec![bytes(header), number(*height), TxCallArgumentType::String(network.name().to_string())])
            }
            BtcAction::RelaySync { relay, batch } => {
                let tip = self.read(relay, "tip", json!([]))?;
                let mut height = self.last_shared(relay, tip.get(1).map_or(0, whole))?;
                let top: u64 = self.esplora("/blocks/tip/height")?.trim().parse().map_err(|_| "Esplora gave a height that is not a number")?;

                while height < top {
                    let mut headers = Vec::new();
                    let mut stopped = None;

                    while height < top && headers.len() / 80 < *batch {
                        let header = self.esplora(&format!("/block-height/{}", height + 1)).and_then(|hash| {
                            from_hex(&format!("0x{}", self.esplora(&format!("/block/{}/header", hash.trim()))?.trim())).ok_or_else(|| "Esplora gave a header that is not hex".into())
                        });

                        match header {
                            Ok(header) => {
                                headers.extend(header);
                                height += 1;
                            }
                            Err(err) => {
                                stopped = Some(err);
                                break;
                            }
                        }
                    }

                    // What came before Esplora stopped answering is kept.
                    if !headers.is_empty() {
                        println!("submitting headers to {height}");
                        self.call(*relay, "submit", u256::ZERO, vec![bytes(headers)])?;
                    }

                    if let Some(err) = stopped {
                        return Err(err);
                    }
                }

                Ok(())
            }
            BtcAction::Deploy { relay, fee_cap, fee_floor, grace_hours } => {
                let me = crate::cli::signer(self.args.key.as_deref())?.public().address();
                let template = self.deploy_lua(&me, "wrapped.lua", super::WRAPPED_LUA)?;
                let gateway = self.deploy_lua(&me, "btc_gateway.lua", include_str!("../../../contracts/lua/btc_gateway.lua"))?;
                self.call(gateway, "setup", u256::ZERO, vec![TxCallArgumentType::Address(*relay), TxCallArgumentType::Address(template), number(*fee_cap), number(*fee_floor), number(grace_hours * 3_600_000)])?;
                println!("btc gateway {gateway}");

                Ok(())
            }
            BtcAction::Address { recipient, network: told } => {
                let (written, network, why) = self.deposit_address(recipient, told.as_deref())?;
                eprintln!("for Bitcoin's {} network: {why}", network.name());
                println!("{written}");

                Ok(())
            }
            BtcAction::Deposit { txid, vout, recipient } => {
                let Proof { tx, block, index, path, coinbase, coinbase_path } = self.proof(txid)?;
                let args = vec![bytes(block.to_vec()), bytes(tx), number(index), bytes(path), bytes(coinbase), bytes(coinbase_path), number(u64::from(*vout)), TxCallArgumentType::Address(*recipient), TxCallArgumentType::Nil];
                self.call(self.btc_gateway()?, "deposit", u256::ZERO, args)
            }
            BtcAction::SetMostFee { withdrawal, most_fee } => self.call(self.btc_gateway()?, "set_most_fee", u256::ZERO, vec![number(*withdrawal), number(*most_fee)]),
            BtcAction::Withdraw { address: text, sats, most_fee, network } => {
                let gateway = self.btc_gateway()?;
                // Only an address on the Bitcoin the gateway pays out on: one for another network
                // would pay bitcoin there to whoever holds that program's key, or no one.
                let (followed, why) = self.followed(&gateway, network.as_deref())?;
                let script = followed.output_script(text).map_err(|err| format!("{err} ({why})"))?;
                let part = self.status().ok().and_then(|status| status["federation"]["btc"]["fee_part"].as_u64()).unwrap_or(10).max(1);
                let most = most_fee.unwrap_or(sats / part);
                let (owed, repaying, repay) = self.repay_policy(&gateway)?;
                let repaying = match owed > repaying {
                    true => format!(", and while the gateway repays {} satoshis children cost it, up to {} more of that", owed - repaying, repay.most.min(most)),
                    false => String::new(),
                };
                println!("it arrives as at least {}: {sats} less its share of its release's fee{repaying}, all within the {most} it may lose", sats.saturating_sub(most));
                self.call(gateway, "withdraw", u256::ZERO, vec![bytes(script), number(*sats), most_fee.map_or(TxCallArgumentType::Nil, number)])
            }
            BtcAction::Propose { fee } => self.propose(*fee),
            BtcAction::Sweep { fee } => self.sweep(*fee),
            BtcAction::Sign => self.sign_releases(&self.fee_rates(), false),
            BtcAction::Pass => self.btc_pass(),
            BtcAction::Anchor { btc_key, every, fee_rate } => {
                let secret = match btc_key {
                    Some(path) => match crate::cli::signer(Some(path))?.private() {
                        crate::crypto::keypair::PrivateKey::Secp256k1(secret) => *secret,
                        _ => return Err("--btc-key is a secp256k1 key: make one with keygen --scheme secp256k1".into()),
                    },
                    None => self.evm_secret()?,
                };
                let mut pending = None;

                loop {
                    if let Err(why) = self.anchor_pass(&secret, &mut pending, *fee_rate) {
                        eprintln!("the anchor pass: {why}");
                    }

                    if *every == 0 {
                        return Ok(());
                    }

                    std::thread::sleep(std::time::Duration::from_secs(*every));
                }
            }
            BtcAction::Watch { every } => loop {
                if let Err(why) = self.btc_pass() {
                    eprintln!("the Bitcoin pass: {why}");
                }

                std::thread::sleep(std::time::Duration::from_secs(*every));
            },
            BtcAction::Cancel { proposal } => self.vote_cancel(*proposal),
            BtcAction::Finish { proposal } => self.finish(*proposal),
            BtcAction::Released { proposal } => {
                let found = self.read(&self.btc_gateway()?, "proposal", json!([{ "type": "u64", "value": proposal.to_string() }]))?;
                self.show_released(*proposal, &found)
            }
            BtcAction::Bump { release, rate, child } => self.bump_by_hand(*release, *rate, *child),
        }
    }

    /// A recipient's deposit address under the federation in force, written for the Bitcoin the
    /// gateway's relay follows: the address, that network, and what said so.
    pub(crate) fn deposit_address(&self, recipient: &Address, told: Option<&str>) -> CliResult<(String, Followed, String)> {
        let gateway = self.btc_gateway()?;
        let scripts = self.read(&gateway, "deposit_script", json!([{ "type": "address", "value": recipient.to_string() }]))?;
        let (network, why) = self.followed(&gateway, told)?;

        Ok((network.address(&raw(scripts.get(1).ok_or("the gateway gave no deposit script")?)), network, why))
    }

    /// Which Bitcoin the gateway's relay follows, as pick_network decides it: Esplora is asked
    /// only when the relay does not say, and only when given.
    fn followed(&self, gateway: &Address, told: Option<&str>) -> CliResult<(Followed, String)> {
        let relay = self.read(gateway, "relay", json!([]))?.first().map(text).and_then(|text| text.parse::<Address>().ok());
        let name = match relay {
            Some(relay) => self.read(&relay, "network_name", json!([]))?.first().filter(|value| value["type"].as_str() == Some("string")).map(text),
            None => None,
        };
        let genesis = match (&name, &self.args.esplora) {
            (None, Some(_)) => self.esplora("/block-height/0").map_err(|why| eprintln!("Esplora did not say which Bitcoin it serves: {why}")).ok(),
            _ => None,
        };

        pick_network(name.as_deref(), genesis.as_deref(), told)
    }

    /// Shows a release Bitcoin mined, by Esplora's proof of it.
    fn show_released(&self, proposal: u64, found: &[Value]) -> CliResult<()> {
        self.call(self.btc_gateway()?, "released", u256::ZERO, self.released_args(proposal, found)?)
    }

    /// What released takes for a proposal, from Esplora's proof of it.
    fn released_args(&self, proposal: u64, found: &[Value]) -> CliResult<Vec<TxCallArgumentType>> {
        let (bytes, number) = (TxCallArgumentType::Bytes, TxCallArgumentType::U64);
        let txid = shown(&raw(found.get(1).ok_or("no such release")?));
        let Proof { tx, block, index, path, coinbase, coinbase_path } = self.proof(&txid)?;

        Ok(vec![number(proposal), bytes(block.to_vec()), bytes(tx), number(index), bytes(path), bytes(coinbase), bytes(coinbase_path)])
    }

    /// Shows a proposal Bitcoin mined, in this signer's turn, as one released does for every signer:
    /// the proof is fetched first, so the wait is all that stands between checking and sending.
    fn show_in_turn(&self, gateway: &Address, proposal: u64, found: &[Value]) -> CliResult<()> {
        let args = self.released_args(proposal, found)?;

        if !self.my_turn(gateway, Turn::Show(proposal))? {
            println!("release {proposal} is shown already, or about to be");

            return Ok(());
        }

        self.call(*gateway, "released", u256::ZERO, args)
    }

    /// This signer's place among the federation in force's signers, from 0, which its turn goes by:
    /// a pass by none of them, with no --evm-key, goes after them all.
    pub(super) fn place(&self) -> CliResult<u64> {
        let status = self.status()?;
        let signers = status["federation"]["signers"].as_array().cloned().unwrap_or_default();
        let key = match self.args.evm_key {
            Some(_) => Some(to_hex(&crate::crypto::secp256k1::public_key(&self.evm_secret()?).ok_or("--evm-key is no key")?)),
            None => None,
        };

        Ok(key.and_then(|key| signers.iter().position(|signer| signer.as_str() == Some(&key))).unwrap_or(signers.len()) as u64)
    }

    /// The chain's block time, from its newest ten blocks: five seconds should they not read.
    fn block_ms(&self) -> u64 {
        let block = |at: &str| -> Option<(u64, u64)> {
            let found = crate::cli::send::ok(self.agent.get(format!("{}/blocks/{at}", self.node)).call()).ok()?;

            Some((found["height"].as_u64()?, found["timestamp"].as_u64()?))
        };

        match block("latest") {
            Some(newer) => block(&newer.0.saturating_sub(10).to_string()).map_or(5_000, |older| btc_block_ms(newer, older)),
            None => 5_000,
        }
    }

    /// How many calls to the gateway of these functions wait in the node's mempool, for this
    /// proposal if one is given: what another signer sent that no block holds yet. None are known of
    /// a node that does not say.
    fn pending(&self, gateway: &Address, functions: &[&str], proposal: Option<u64>) -> usize {
        let Ok(pool) = crate::cli::send::ok(self.agent.get(format!("{}/mempool?limit=1000", self.node)).call()) else {
            return 0;
        };
        let gateway = gateway.to_string();

        pool["txs"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|signed| &signed["tx"])
            .filter(|tx| tx["type"] == "contract_call" && tx["to"].as_str() == Some(&gateway) && functions.contains(&tx["func"].as_str().unwrap_or_default()))
            .filter(|tx| proposal.is_none_or(|proposal| tx["data"][0]["value"].as_str() == Some(&proposal.to_string())))
            .count()
    }

    /// Waits this signer's turn to send what any one signer's does for all, then says whether it
    /// still should, by what the gateway says now and what waits in the mempool. The first goes at
    /// once and each after it a block after the one before, so the first is usually the only one to
    /// pay, and the rest step in only should it be down. Signing takes no turn, as every signer signs.
    fn my_turn(&self, gateway: &Address, turn: Turn) -> CliResult<bool> {
        let delay = btc_turn_delay(self.place()?, self.block_ms());

        if delay > 0 {
            std::thread::sleep(std::time::Duration::from_millis(delay));
        }

        let state = |proposal: u64| -> CliResult<String> { Ok(self.read(gateway, "proposal", json!([{ "type": "u64", "value": proposal.to_string() }]))?.get(4).map(text).unwrap_or_default()) };

        Ok(match turn {
            Turn::Show(proposal) => matches!(state(proposal)?.as_str(), "open" | "cancelled") && self.pending(gateway, &["released"], Some(proposal)) == 0,
            Turn::Cancel(proposal) => {
                let status = self.status()?;
                let threshold = status["federation"]["threshold"].as_u64().unwrap_or(1) as usize;
                let mut voted = 0;

                for signer in status["federation"]["signers"].as_array().into_iter().flatten() {
                    let key = json!({ "type": "bytes", "value": signer.as_str().unwrap_or_default() });

                    voted += usize::from(self.read(gateway, "voted", json!([{ "type": "u64", "value": proposal.to_string() }, key]))?.first().is_some_and(|value| value["value"] == true));
                }

                state(proposal)? == "open" && voted + self.pending(gateway, &["cancel"], Some(proposal)) < threshold
            }
            Turn::Propose(before) => self.proposal_count(gateway)? == before && self.pending(gateway, &["propose"], None) == 0,
            Turn::Bump(before) => self.proposal_count(gateway)? == before && self.pending(gateway, &["replace", "child"], None) == 0,
        })
    }

    /// What the network asks now, from Esplora: None for an Esplora that has no estimates to give,
    /// and an error for one out of reach or throttling. Asked once a pass, and again only once: fees
    /// going unread must not hold up a pass for long.
    fn fee_rates(&self) -> Result<Option<FeeRates>, String> {
        let base = self.args.esplora.as_deref().ok_or("no --esplora given")?;
        let url = format!("{}/fee-estimates", base.trim_end_matches('/'));
        let answer = asked_again(2, std::thread::sleep, || {
            let mut answer = self.agent.get(&url).call().map_err(|err| format!("Esplora /fee-estimates: {err}"))?;

            fee_answer(answer.status().as_u16(), || answer.body_mut().read_to_string().map_err(|err| format!("Esplora /fee-estimates: {err}")))
        })?;

        fee_rates_from(&answer)
    }

    /// Whether this key is a signer of the federation in force.
    pub(super) fn is_signer(&self, secret: &[u8; 32]) -> CliResult<bool> {
        let key = to_hex(&crate::crypto::secp256k1::public_key(secret).ok_or("--evm-key is no key")?);

        Ok(self.status()?["federation"]["signers"].as_array().into_iter().flatten().any(|signer| signer.as_str() == Some(&key)))
    }

    /// The federation signer's key, for what only it may propose.
    fn proposer(&self) -> CliResult<[u8; 32]> {
        let secret = self.evm_secret().map_err(|why| format!("only a signer of the federation in force proposes: {why}"))?;

        match self.is_signer(&secret)? {
            true => Ok(secret),
            false => Err("--evm-key is not a signer of the federation in force, which alone proposes".into()),
        }
    }

    /// One pass of all that pays withdrawals. Each step stands alone, so one going wrong stops no
    /// other, and what cannot go now waits for the next pass.
    pub(super) fn btc_pass(&self) -> CliResult<()> {
        let gateway = self.btc_gateway()?;
        let rates = self.fee_rates();

        if let Err(why) = &rates {
            eprintln!("what Bitcoin's network asks: {why}");
        }

        let signer = match self.args.evm_key.as_ref().map(|_| self.evm_secret()).transpose()? {
            Some(secret) if self.is_signer(&secret)? => {
                if let Err(why) = self.propose_by_itself(&gateway, &secret, &rates) {
                    eprintln!("proposing: {why}");
                }

                if let Err(why) = self.sign_releases(&rates, true) {
                    eprintln!("signing releases: {why}");
                }

                Some(secret)
            }
            Some(_) => {
                eprintln!("--evm-key is not a signer of the federation in force: proposing, signing and bumping nothing");

                None
            }
            None => None,
        };

        // Without Esplora nothing can be broadcast or proven, and each try would wait to be asked again.
        if rates.is_ok()
            && let Err(why) = self.carry_on_releases(&gateway, &rates, signer.as_ref())
        {
            eprintln!("finishing releases: {why}");
        }

        Ok(())
    }

    /// Proposes what is queued at the rate the network asks, if anything can go. What cannot goes
    /// said and stays queued. Signers doing this at once work out one release, and the first to land
    /// wins: the others find that before they send, or at worst one transaction later.
    fn propose_by_itself(&self, gateway: &Address, secret: &[u8; 32], rates: &Result<Option<FeeRates>, String>) -> CliResult<()> {
        let rate = match rates {
            Ok(Some(rates)) => rates.soon.ceil() as u64,
            // A rate of nothing pays the gateway's floor, all that is known to ask.
            Ok(None) => 0,
            Err(_) if self.queued(gateway)?.is_empty() => return Ok(()),
            Err(why) => {
                eprintln!("what is queued stays queued, as no fee rate could be read: {why}");

                return Ok(());
            }
        };
        let (most_rate, _) = self.signing_limit(gateway, rates)?;
        let (_, floor) = self.fee_limits(gateway)?;
        let rate = rate.max(floor);

        // It would refuse its own release, and vote it down: the next pass would only propose it again.
        if rate > most_rate {
            if !self.queued(gateway)?.is_empty() {
                println!("what is queued waits: the network asks {rate} satoshis a virtual byte, past the {most_rate} this signer signs for");
            }

            return Ok(());
        }

        let cooling = self.cooling(gateway, rate)?;
        let Some(plan) = self.plan_queue_without(gateway, ReleaseFee::Rate(rate), &cooling)? else {
            return Ok(());
        };

        for (id, why) in cooling.iter().chain(&plan.waiting) {
            println!("withdrawal {id} waits: {why}");
        }

        let Some(tx) = plan.tx else {
            return Ok(());
        };
        let before = self.proposal_count(gateway)?;
        println!("proposing a release paying {} withdrawals with a fee of {}, txid {}", plan.ids.len(), plan.fee, shown(&sha256d(&tx.base())));

        if !self.my_turn(gateway, Turn::Propose(before))? || !self.post_unless_beaten(gateway, secret, &tx, &plan.ids, before)? {
            println!("another signer proposed first");
        }

        Ok(())
    }

    /// Posts a release worked out when this many were proposed: false if another came first, which
    /// is how signers at it together are meant to end, and no failure. By the dry run that costs
    /// nothing, or at worst the one transaction that lost; the next pass works from what stands then.
    pub(crate) fn post_unless_beaten(&self, gateway: &Address, secret: &[u8; 32], tx: &Transaction, ids: &[u64], before: u64) -> CliResult<bool> {
        match self.post_proposal(gateway, secret, tx, ids, before + 1) {
            Ok(()) => Ok(true),
            Err(_) if self.proposal_count(gateway)? > before => Ok(false),
            Err(why) => Err(why),
        }
    }

    fn proposal_count(&self, gateway: &Address) -> CliResult<u64> {
        Ok(whole(self.read(gateway, "proposal_count", json!([]))?.first().ok_or("no count")?))
    }

    /// Posts a release under this signer's signature over it, for the number it will take.
    fn post_proposal(&self, gateway: &Address, secret: &[u8; 32], tx: &Transaction, ids: &[u64], number: u64) -> CliResult<()> {
        let raw = tx.base();
        let signature = sign_digest(secret, &btc_propose_digest(self.here()?, gateway, number, &raw, ids));
        let mut args = vec![TxCallArgumentType::Bytes(raw), TxCallArgumentType::Bytes(signature.to_vec())];
        args.extend(ids.iter().map(|id| TxCallArgumentType::U64(*id)));

        self.call(*gateway, "propose", u256::ZERO, args)
    }

    /// How long a release waits before this signer votes to cancel it: the federation's policy,
    /// which governance sets for every signer alike, or what this signer was started with while
    /// it names none.
    fn cancel_after(&self) -> u64 {
        self.status().ok().and_then(|status| status["federation"]["btc"]["cancel_after_ms"].as_u64()).unwrap_or(self.args.cancel_after)
    }

    /// How long a release with all its signatures goes unmined before this signer bumps it, and how
    /// far under what the network asks it must pay: the federation's policy, which governance sets
    /// for every signer alike, or what this signer was started with and the default while it names
    /// none.
    fn bump_policy(&self) -> (u64, u64) {
        let status = self.status().ok();
        let btc = status.as_ref().map(|status| &status["federation"]["btc"]);

        (btc.and_then(|btc| btc["bump_after_ms"].as_u64()).unwrap_or(self.args.bump_after), btc.and_then(|btc| btc["bump_below_pct"].as_u64()).unwrap_or(BtcPolicy::BUMP_BELOW_PCT))
    }

    /// For each proposal still open, or cancelled and yet mined: shows one Bitcoin mined deep enough;
    /// then for each release still open with nothing of it mined, does what carry_on_tree says. One
    /// going wrong is said and passed over, for the next.
    fn carry_on_releases(&self, gateway: &Address, rates: &Result<Option<FeeRates>, String>, signer: Option<&[u8; 32]>) -> CliResult<()> {
        let count = self.proposal_count(gateway)?;
        let depth = self.status()?["federation"]["bitcoin_depth"].as_u64().unwrap_or(6);
        let mut done = self.btc_done.get();
        let tip: u64 = self.esplora("/blocks/tip/height")?.trim().parse().map_err(|_| "Esplora gave a height that is not a number")?;
        let (mut open, mut mined) = (std::collections::BTreeSet::new(), std::collections::HashSet::new());

        if done.releases > count {
            done.releases = 0;
        }

        for id in done.releases + 1..=count {
            let found = self.read(gateway, "proposal", json!([{ "type": "u64", "value": id.to_string() }]))?;
            let state = found.get(4).map(text).unwrap_or_default();

            // What is shown, or closed as another version of it was, stays so.
            if (state == "released" || state == "replaced") && id == done.releases + 1 {
                done.releases = id;
            }

            if state != "open" && state != "cancelled" {
                continue;
            }

            let root = self.read(gateway, "tree", json!([{ "type": "u64", "value": id.to_string() }]))?.first().map_or(id, whole);

            match self.show_if_mined(gateway, id, &found, tip, depth) {
                Ok(true) => {
                    mined.insert(root);
                }
                Ok(false) if state == "open" => {
                    open.insert(root);
                }
                Ok(false) => {}
                Err(why) => eprintln!("release {id}: {why}"),
            }
        }

        self.btc_done.set(done);

        for root in open.into_iter().filter(|root| !mined.contains(root)) {
            if let Err(why) = self.carry_on_tree(gateway, root, rates, signer) {
                eprintln!("release {root}: {why}");
            }
        }

        Ok(())
    }

    /// Whether Bitcoin mined a proposal, showing it once it is deep enough, in this signer's turn.
    fn show_if_mined(&self, gateway: &Address, id: u64, found: &[Value], tip: u64, depth: u64) -> CliResult<bool> {
        let txid = shown(&raw(found.get(1).ok_or("no such release")?));
        let status: Value = serde_json::from_str(&self.esplora(&format!("/tx/{txid}/status"))?).unwrap_or(Value::Null);

        if status["confirmed"] != true {
            return Ok(false);
        }

        if (tip + 1).saturating_sub(status["block_height"].as_u64().unwrap_or(u64::MAX)) >= depth {
            self.show_in_turn(gateway, id, found)?;
        }

        Ok(true)
    }

    /// A release open on the gateway that Bitcoin has mined none of, with its every bump. Broadcasts
    /// its newest version with all its signatures that Bitcoin has not heard of, and the newest of a
    /// child paying for the version it holds; warns of it past the age limit; and, by a signer, once
    /// nothing of it waits for signatures, bumps it should it be stuck, as btc_is_stuck has it, or
    /// with a child at once should Esplora refuse a replacement while an older version stands, as when
    /// a recipient pinned it with a large child of their own.
    fn carry_on_tree(&self, gateway: &Address, root: u64, rates: &Result<Option<FeeRates>, String>, signer: Option<&[u8; 32]>) -> CliResult<()> {
        let members = self.tree(gateway, root)?;
        let mut signed = Vec::new();
        let mut short = false;

        // Asked apart from its status, as Esplora says a transaction it never heard of is unconfirmed.
        for member in members.iter().filter(|member| member.state == "open") {
            match self.signed_enough(gateway, member.id, &member.found)? {
                true => signed.push((member, self.bitcoin_knows(&member.txid())?)),
                false => short = true,
            }
        }

        // The release's versions, newest first, and the newest Bitcoin holds.
        let versions: Vec<(&Member, bool)> = signed.iter().filter(|(member, _)| member.group == root).rev().copied().collect();
        let live = versions.iter().find(|(_, known)| *known).map(|(member, _)| *member);
        let child_of = |parent: &Member| signed.iter().rfind(|(member, _)| paid_for(&members, member) == Some(parent.id)).copied();
        let child = live.and_then(child_of);
        let mut refused = false;

        // What was just broadcast gets a pass to be heard of before anything bumps it. A replacement is
        // not sent again once a child pays for the version Bitcoin holds instead, signed or not.
        let paid_for_live = live.is_some_and(|live| members.iter().any(|member| member.state == "open" && paid_for(&members, member) == Some(live.id)));

        if let Some((newest, false)) = versions.first().copied()
            && !paid_for_live
        {
            match self.finish(newest.id) {
                Ok(()) => return Ok(()),
                Err(why) => {
                    eprintln!("release {}: {why}", newest.id);
                    refused = live.is_some();
                }
            }
        }

        if let Some((newest, false)) = child {
            match self.finish(newest.id) {
                Ok(()) => return Ok(()),
                Err(why) => eprintln!("release {}: {why}", newest.id),
            }
        }

        let asked = rates.as_ref().ok().copied().flatten().map(|rates| rates.soon.ceil() as u64);

        if let Some(live) = live
            && let Some(warning) = self.stuck_warning(gateway, root, &live.found, asked)?
        {
            eprintln!("{warning}");
        }

        // A bump waits while anything of it waits for signatures: one at a time.
        let (Some(secret), false) = (signer, short) else {
            return Ok(());
        };
        let Some(target) = live.or(versions.first().map(|(member, _)| *member)) else {
            return Ok(());
        };
        let newest = members.iter().map(|member| member.id).max().unwrap_or(root);
        let age = self.release_ages(gateway, &[newest]).ok().and_then(|ages| ages.get(&newest).copied());
        let rate = self.paying(gateway, target, child.map(|(child, _)| child))?;
        let (after, below) = self.bump_policy();
        // Bitcoin holding none of it, a version paying more is all that can go.
        let kind = live.map_or(BumpKind::Replace, |live| btc_bump_kind(child.is_some(), live.kind == "replacement", refused));

        if !(refused || btc_is_stuck(age, rate, asked, after, below)) {
            return Ok(());
        }

        let Some(asked) = asked else {
            eprintln!("release {root}: Esplora gives no rate to bump it to");

            return Ok(());
        };
        let (most_rate, _) = self.signing_limit(gateway, rates)?;

        self.propose_bump(gateway, secret, &members, target, kind, asked.max(rate + 1), most_rate, true)
    }

    /// What a version pays a virtual byte, with the child paying for it if one is given, as Bitcoin's
    /// miners reckon the two together.
    fn paying(&self, gateway: &Address, version: &Member, child: Option<&Member>) -> CliResult<u64> {
        let mut total = (0, 0);

        for member in std::iter::once(version).chain(child) {
            let (fee, vbytes) = btc_fee_and_vbytes(&self.release_facts(gateway, &member.found, 0)?)?;
            total = (total.0 + fee, total.1 + vbytes);
        }

        Ok(total.0 / total.1.max(1))
    }

    /// Proposes a bump of a release at a rate: a replacement of a version, a child paying for one, or
    /// a replacement of that child, as plan_btc_replacement, plan_btc_child and
    /// plan_btc_child_replacement work it out, and never past what this signer signs for, as it
    /// would only vote it down. In a pass it waits its turn first.
    #[allow(clippy::too_many_arguments)]
    fn propose_bump(&self, gateway: &Address, secret: &[u8; 32], members: &[Member], target: &Member, kind: BumpKind, rate: u64, most_rate: u64, in_turn: bool) -> CliResult<()> {
        let root = members.first().map_or(target.id, |member| member.id);

        if rate > most_rate {
            println!("release {root} waits: a bump to {rate} satoshis a virtual byte is past the {most_rate} this signer signs for");

            return Ok(());
        }

        let open_fee = |group: u64| members.iter().filter(|member| member.group == group && member.state == "open").map(|member| member.fee).max().unwrap_or(0);
        let facts = |member: &Member| self.release_facts(gateway, &member.found, most_rate);
        let (plan, domain, function, of) = match kind {
            BumpKind::Replace => (plan_btc_replacement(&facts(target)?, open_fee(target.group), rate)?, BTC_REPLACE_DOMAIN, "replace", target.id),
            BumpKind::Child => {
                let change = p2wsh(&raw(self.read(gateway, "change_script", json!([]))?.first().ok_or("no change script")?));

                (plan_btc_child(&facts(target)?, &self.unspent(gateway)?, &change, rate)?, BTC_CHILD_DOMAIN, "child", target.id)
            }
            BumpKind::ReplaceChild => {
                let child = members.iter().rfind(|member| member.state == "open" && paid_for(members, member) == Some(target.id)).ok_or("no child pays for it")?;

                (plan_btc_child_replacement(&facts(target)?, &facts(child)?, open_fee(child.group), rate)?, BTC_REPLACE_DOMAIN, "replace", child.id)
            }
        };

        for id in &plan.held_back {
            println!("withdrawal {id} holds the bump of release {root} under {rate} satoshis a virtual byte: its owner may raise what it may lose with set-most-fee");
        }

        let raw = plan.tx.base();
        let before = self.proposal_count(gateway)?;
        let what = if function == "child" { "a child paying for" } else { "a replacement of" };
        println!("bumping release {root}: {what} {of}, with a fee of {}, {} satoshis a virtual byte, txid {}", plan.fee, plan.rate, shown(&sha256d(&raw)));

        if in_turn && !self.my_turn(gateway, Turn::Bump(before))? {
            println!("release {root} is bumped by another signer, or about to be");

            return Ok(());
        }

        let signature = sign_digest(secret, &btc_bump_digest(domain, self.here()?, gateway, before + 1, of, &raw));

        match self.call(*gateway, function, u256::ZERO, vec![TxCallArgumentType::U64(of), TxCallArgumentType::Bytes(raw), TxCallArgumentType::Bytes(signature.to_vec())]) {
            Ok(()) => Ok(()),
            Err(_) if self.proposal_count(gateway)? > before => {
                println!("another signer bumped release {root} first");

                Ok(())
            }
            Err(why) => Err(why),
        }
    }

    /// A signer bumps a release by hand, sooner than a pass would: a replacement of its newest
    /// version, or with child a child paying for the version Bitcoin holds, or should a child pay for
    /// it already, a replacement of that, at the rate told or what the network asks. The others sign
    /// it as any bump, within their limits and while it is stuck as their Esplora says.
    fn bump_by_hand(&self, release: u64, rate: Option<u64>, child: bool) -> CliResult<()> {
        let (gateway, secret) = (self.btc_gateway()?, self.proposer()?);
        let rates = self.fee_rates();
        let members = self.tree(&gateway, release)?;
        let root = members.first().ok_or("no such release")?;

        if root.state != "open" {
            return Err(format!("release {} is {}: only one Bitcoin has not mined is bumped", root.id, root.state).into());
        }

        let mut versions = Vec::new();

        for member in members.iter().filter(|member| member.state == "open") {
            if !self.signed_enough(&gateway, member.id, &member.found)? {
                return Err(format!("{} waits for its signatures: a bump goes once all of a release holds them, one at a time", member.id).into());
            }

            if member.group == root.id {
                versions.push((member, self.bitcoin_knows(&member.txid())?));
            }
        }

        let target = versions.iter().rev().find(|(_, known)| *known).or(versions.last()).map(|(member, _)| *member).ok_or("no version of it is open")?;
        let child_open = members.iter().any(|member| member.state == "open" && paid_for(&members, member) == Some(target.id));
        let kind = match (child, child_open) {
            (_, true) => BumpKind::ReplaceChild,
            (true, false) => BumpKind::Child,
            (false, false) => BumpKind::Replace,
        };
        let rate = match rate {
            Some(rate) => rate,
            None => rates.as_ref().ok().copied().flatten().map(|rates| rates.soon.ceil() as u64).ok_or("Esplora gives no rate: give --rate")?,
        };
        let (most_rate, _) = self.signing_limit(&gateway, &rates)?;

        self.propose_bump(&gateway, &secret, &members, target, kind, rate, most_rate, false)
    }

    /// A release's tree, from any proposal of it: the release, then every bump of it in the order
    /// proposed, each with the group of versions it is in.
    pub(crate) fn tree(&self, gateway: &Address, id: u64) -> CliResult<Vec<Member>> {
        let found = self.read(gateway, "tree", json!([{ "type": "u64", "value": id.to_string() }]))?;
        let root = found.first().map_or(id, whole);
        let bumps = found.get(1).map(text).unwrap_or_default();
        let mut members: Vec<Member> = Vec::new();

        for id in std::iter::once(root).chain(bumps.split(',').filter_map(|id| id.trim().parse().ok())) {
            let found = self.read(gateway, "proposal", json!([{ "type": "u64", "value": id.to_string() }]))?;
            let kind = found.get(5).filter(|value| !value["value"].is_null()).map_or_else(|| "release".to_string(), text);
            let of = found.get(6).filter(|value| !value["value"].is_null()).map(whole);
            let group = match kind.as_str() {
                "replacement" => members.iter().find(|member| Some(member.id) == of).map_or(root, |member| member.group),
                "child" => id,
                _ => root,
            };

            members.push(Member { id, state: found.get(4).map(text).unwrap_or_default(), kind, of, group, fee: found.get(7).map_or(0, whole), found });
        }

        Ok(members)
    }

    /// What a pass says of a release with all its signatures that Bitcoin has not mined, once it is
    /// past the age limit, by what the chain says of its age. One with an input shown spent can never
    /// be mined and takes a vote instead, and none is said if its age cannot be read.
    pub(crate) fn stuck_warning(&self, gateway: &Address, id: u64, found: &[Value], asked: Option<u64>) -> CliResult<Option<String>> {
        let Some(age) = self.release_ages(gateway, &[id]).ok().and_then(|ages| ages.get(&id).copied()).filter(|age| *age >= self.cancel_after()) else {
            return Ok(None);
        };
        let facts = self.release_facts(gateway, found, 0)?;

        if facts.inputs.iter().any(|(_, held)| held.state == "spent") {
            return Ok(None);
        }

        Ok(Some(btc_stuck_warning(id, &shown(&facts.txid), age, btc_release_rate(&facts)?, asked)))
    }

    /// How old releases are, by the chain's block time: its tip's less that of the block each was
    /// proposed in, so no signer's clock has a say. The gateway keeps no time for a release, so this
    /// reads its Proposed events, and a release the index has no event for is left out, as one on a
    /// node that pruned it. An error for a node with no index at all, as --index-events gives.
    fn release_ages(&self, gateway: &Address, ids: &[u64]) -> CliResult<std::collections::HashMap<u64, u64>> {
        let mut wanted: std::collections::HashSet<u64> = ids.iter().copied().collect();
        let mut ages = std::collections::HashMap::new();
        let time_of = |block: &str| -> CliResult<u64> { crate::cli::send::ok(self.agent.get(format!("{}/blocks/{block}", self.node)).call())?["timestamp"].as_u64().ok_or_else(|| "a block with no time".into()) };
        let now = time_of("latest")?;
        let mut cursor: Option<String> = None;

        while !wanted.is_empty() {
            let at = cursor.as_ref().map_or(String::new(), |cursor| format!("&cursor={cursor}"));
            let page = crate::cli::send::ok(self.agent.get(format!("{}/events?contract={gateway}&name=Proposed&limit=500{at}", self.node)).call()).map_err(|err| format!("how old a release is needs a node with --index-events: {err}"))?;

            for event in page["events"].as_array().into_iter().flatten() {
                let Some(id) = event["data"][0]["value"].as_str().and_then(|text| text.parse::<u64>().ok()) else {
                    continue;
                };

                if wanted.remove(&id)
                    && let Some(height) = event["height"].as_u64()
                {
                    ages.insert(id, now.saturating_sub(time_of(&height.to_string())?));
                }
            }

            cursor = page["next"].as_str().map(str::to_string);

            if cursor.is_none() {
                break;
            }
        }

        Ok(ages)
    }

    /// The most a virtual byte this signer signs for now, and whether nothing but a fixed limit says
    /// so, its operator having told none and Esplora none either.
    fn signing_limit(&self, gateway: &Address, rates: &Result<Option<FeeRates>, String>) -> CliResult<(u64, bool)> {
        let (_, floor) = self.fee_limits(gateway)?;
        let fastest = rates.as_ref().ok().copied().flatten().map(|rates| rates.fastest);

        Ok((btc_most_fee_rate(self.args.max_fee_rate, fastest, floor), self.args.max_fee_rate.is_none() && fastest.is_none()))
    }

    /// The queued withdrawals that may not go at this rate yet, each with why: a release paying them
    /// was cancelled at about this rate or more, and is younger than the limit. Without this the same
    /// release would be proposed, refused and cancelled every pass for as long as the network asks what
    /// some signer refuses. It costs nothing unless one is queued from a cancelled release, as its
    /// anchor says.
    fn cooling(&self, gateway: &Address, rate: u64) -> CliResult<Vec<(u64, String)>> {
        let anchored: std::collections::HashSet<u64> = self.queued(gateway)?.iter().filter(|withdrawal| withdrawal.anchor.is_some()).map(|withdrawal| withdrawal.id).collect();

        if anchored.is_empty() {
            return Ok(Vec::new());
        }

        let mut cancelled = Vec::new();

        for id in (self.btc_done.get().releases + 1)..=self.proposal_count(gateway)? {
            let found = self.read(gateway, "proposal", json!([{ "type": "u64", "value": id.to_string() }]))?;

            // A bump cancelled alone left its release standing, and anchored nothing.
            if found.get(4).map(text).as_deref() == Some("cancelled") && found.get(5).is_none_or(|kind| text(kind) == "release") {
                let paid: Vec<u64> = found.get(2).map(text).unwrap_or_default().split(',').filter_map(|paid| paid.parse().ok()).collect();

                if paid.iter().any(|withdrawal| anchored.contains(withdrawal)) {
                    cancelled.push((id, found, paid));
                }
            }
        }

        let ids: Vec<u64> = cancelled.iter().map(|(id, _, _)| *id).collect();
        // An age unread holds back all but a much lower rate, which errs on the side of not going round.
        let ages = self.release_ages(gateway, &ids).unwrap_or_default();
        let mut cooling = std::collections::BTreeMap::new();

        for (id, found, paid) in cancelled {
            let paid_rate = btc_release_rate(&self.release_facts(gateway, &found, 0)?)?;

            if !btc_may_propose_again(rate, paid_rate, ages.get(&id).copied(), self.cancel_after()) {
                for withdrawal in paid.into_iter().filter(|withdrawal| anchored.contains(withdrawal)) {
                    cooling.insert(withdrawal, format!("release {id} paying it was cancelled at {paid_rate} satoshis a virtual byte, and at {rate} it would go the same way: it goes at about a quarter less, or past --cancel-after"));
                }
            }
        }

        Ok(cooling.into_iter().collect())
    }

    /// Whether every input of a release holds its threshold of signatures.
    fn signed_enough(&self, gateway: &Address, id: u64, found: &[Value]) -> CliResult<bool> {
        let (tx, _) = Transaction::read(&raw(found.first().ok_or("no such release")?))?;

        for index in 0..tx.inputs.len() {
            let posted = self.read(gateway, "input_signatures", json!([{ "type": "u64", "value": id.to_string() }, { "type": "u64", "value": index.to_string() }]))?;

            if (posted.len().saturating_sub(2) / 2) < posted.get(1).map_or(usize::MAX, whole_size) {
                return Ok(false);
            }
        }

        Ok(true)
    }

    /// Whether Esplora knows a transaction, mined or in its mempool: one more question, once.
    fn bitcoin_knows(&self, txid: &str) -> CliResult<bool> {
        let base = self.args.esplora.as_deref().ok_or("give --esplora")?;
        let answer = self.agent.get(format!("{}/tx/{txid}/hex", base.trim_end_matches('/'))).call().map_err(|err| format!("Esplora /tx: {err}"))?;

        Ok(answer.status().is_success())
    }

    /// What the gateway holds unspent, in the order it took it, each with what it says of it.
    fn unspent(&self, gateway: &Address) -> CliResult<Vec<(String, SpentOutput)>> {
        let count = whole(self.read(gateway, "utxo_count", json!([]))?.first().ok_or("no count")?);
        let mut done = self.btc_done.get();
        let mut found = Vec::new();

        if done.outputs > count {
            done.outputs = 0;
        }

        for n in done.outputs..count {
            let utxo = self.read(gateway, "utxo", json!([{ "type": "u64", "value": n.to_string() }]))?;

            match utxo.get(4).map(text).as_deref() {
                Some("unspent") => found.push((text(&utxo[0]), self.spent_output(gateway, &text(&utxo[0]))?)),
                Some("spent") if n == done.outputs => done.outputs = n + 1,
                _ => {}
            }
        }

        self.btc_done.set(done);

        Ok(found)
    }

    /// What is queued, oldest first, each with what the gateway says of it.
    fn queued(&self, gateway: &Address) -> CliResult<Vec<NamedWithdrawal>> {
        let count = whole(self.read(gateway, "withdrawal_count", json!([]))?.first().ok_or("no count")?);
        let mut done = self.btc_done.get();
        let mut found = Vec::new();

        if done.withdrawals > count {
            done.withdrawals = 0;
        }

        for id in done.withdrawals + 1..=count {
            let withdrawal = self.read(gateway, "withdrawal", json!([{ "type": "u64", "value": id.to_string() }]))?;
            let state = withdrawal.get(2).map(text).unwrap_or_default();

            if state == "paid" && id == done.withdrawals + 1 {
                done.withdrawals = id;
            }

            if state == "queued" {
                found.push(NamedWithdrawal { id, script: raw(&withdrawal[0]), sats: whole(&withdrawal[1]), most_fee: whole(&withdrawal[4]), state, anchor: self.anchor(gateway, id)? });
            }
        }

        self.btc_done.set(done);

        Ok(found)
    }

    /// What the gateway says of an output it holds, by its outpoint.
    fn spent_output(&self, gateway: &Address, outpoint: &str) -> CliResult<SpentOutput> {
        let found = self.read(gateway, "utxo_of", json!([{ "type": "string", "value": outpoint }]))?;

        match found.get(5) {
            Some(threshold) => Ok(SpentOutput { sats: whole(&found[0]), version: whole(&found[1]), state: text(&found[3]), threshold: whole(threshold) as u32, script_len: raw(&found[4]).len() }),
            None => Err(format!("the gateway does not hold {outpoint}").into()),
        }
    }

    /// The input a withdrawal's next release must spend, after a cancel, if any.
    fn anchor(&self, gateway: &Address, id: u64) -> CliResult<Option<String>> {
        let found = self.read(gateway, "anchor", json!([{ "type": "u64", "value": id.to_string() }]))?;

        Ok(found.first().filter(|value| !value["value"].is_null()).map(text))
    }

    fn fee_limits(&self, gateway: &Address) -> CliResult<(u64, u64)> {
        let limits = self.read(gateway, "fee_limits", json!([]))?;

        Ok((limits.first().map_or(0, whole), limits.get(1).map_or(0, whole)))
    }

    /// What of the queue can be proposed at this fee: None with nothing queued.
    pub(crate) fn plan_queue(&self, gateway: &Address, fee: ReleaseFee) -> CliResult<Option<crate::bridge::ReleasePlan>> {
        self.plan_queue_without(gateway, fee, &[])
    }

    /// The same, leaving out some of what is queued.
    fn plan_queue_without(&self, gateway: &Address, fee: ReleaseFee, without: &[(u64, String)]) -> CliResult<Option<crate::bridge::ReleasePlan>> {
        let mut queued = self.queued(gateway)?;
        queued.retain(|withdrawal| !without.iter().any(|(id, _)| *id == withdrawal.id));

        if queued.is_empty() {
            return Ok(None);
        }

        let change = p2wsh(&raw(self.read(gateway, "change_script", json!([]))?.first().ok_or("no change script")?));
        let (cap, floor) = self.fee_limits(gateway)?;
        let (owed, repaying, repay) = self.repay_policy(gateway)?;
        let repay = ReleaseRepay { owed: owed.saturating_sub(repaying), ..repay };

        Ok(Some(plan_btc_release(&queued, &self.unspent(gateway)?, &change, cap, floor, fee, &repay)?))
    }

    /// What children cost the backing that is not repaid yet, how much of it open releases repay,
    /// and the federation's policy for repaying it, or the gateway's own while it names none.
    fn repay_policy(&self, gateway: &Address) -> CliResult<(u64, u64, ReleaseRepay)> {
        let owed = self.read(gateway, "shortfall", json!([]))?;
        let status = self.status().ok();
        let btc = status.as_ref().map(|status| &status["federation"]["btc"]);
        let pct = btc.and_then(|btc| btc["repay_pct"].as_u64()).unwrap_or(BtcPolicy::REPAY_PCT);
        let most = btc.and_then(|btc| btc["repay_most"].as_u64()).unwrap_or(BtcPolicy::REPAY_MOST);

        Ok((owed.first().map_or(0, whole), owed.get(1).map_or(0, whole), ReleaseRepay { pct, most, ..ReleaseRepay::default() }))
    }

    /// A signer proposes a release of the queued withdrawals by hand: what it pays told, or what the
    /// network asks. A withdrawal from a cancelled release brings the input it must spend; what cannot
    /// go now is said and waits, and with nothing left to pay that is the answer.
    fn propose(&self, fee: Option<u64>) -> CliResult<()> {
        let (gateway, secret) = (self.btc_gateway()?, self.proposer()?);
        let fee = match fee {
            Some(fee) => ReleaseFee::Fixed(fee),
            None => ReleaseFee::Rate(match self.fee_rates().map_err(|why| format!("{why}: give --fee"))? {
                Some(rates) => rates.soon.ceil() as u64,
                None => 0,
            }),
        };
        let Some(plan) = self.plan_queue(&gateway, fee)? else {
            return Err("no withdrawals are queued".into());
        };

        for (id, why) in &plan.waiting {
            println!("withdrawal {id} waits: {why}");
        }

        let Some(tx) = plan.tx else {
            let why = plan.waiting.iter().map(|(id, why)| format!("withdrawal {id}: {why}")).collect::<Vec<_>>().join("; ");

            return Err(format!("nothing queued can go now: {why}").into());
        };
        println!("proposing a release paying {} withdrawals with a fee of {}, txid {}", plan.ids.len(), plan.fee, shown(&sha256d(&tx.base())));
        let number = self.proposal_count(&gateway)? + 1;

        self.post_proposal(&gateway, &secret, &tx, &plan.ids, number)
    }

    /// A signer proposes a sweep of older federations' outputs: as much as the fee pays at the floor,
    /// for another sweep to take the rest.
    fn sweep(&self, fee: u64) -> CliResult<()> {
        let (gateway, secret) = (self.btc_gateway()?, self.proposer()?);
        let version = self.status()?["federation"]["version"].as_u64().ok_or("this chain has no federation")?;
        let change = p2wsh(&raw(self.read(&gateway, "change_script", json!([]))?.first().ok_or("no change script")?));
        let (_, floor) = self.fee_limits(&gateway)?;
        let mut inputs = Vec::new();
        let mut sizes = Vec::new();
        let mut taken = 0;

        for (outpoint, held) in self.unspent(&gateway)?.into_iter().filter(|(_, held)| held.version < version) {
            sizes.push((held.threshold, held.script_len));

            // Each input and its one output, and the fixed parts around them.
            if !inputs.is_empty() && btc_release_vbytes(10 + (inputs.len() + 1) * 41 + 43, &sizes) * floor > fee {
                sizes.pop();
                break;
            }

            let (txid, vout) = outpoint.split_once(':').ok_or("an outpoint that does not read")?;
            // The gateway keys what it holds by the id in Bitcoin's own byte order.
            let id: [u8; 32] = from_hex(&format!("0x{txid}")).and_then(|bytes| bytes.try_into().ok()).ok_or("an outpoint that does not read")?;
            inputs.push(TxIn { txid: id, vout: vout.parse()?, script_sig: Vec::new(), sequence: BTC_SEQUENCE, witness: Vec::new() });
            taken += held.sats;
        }

        if inputs.is_empty() || taken < fee + BTC_DUST {
            return Err(format!("the older federations hold {taken} satoshis to sweep, short of a fee of {fee} and dust").into());
        }

        let tx = Transaction { version: 2, inputs, outputs: vec![TxOut { value: taken - fee, script: change }], lock_time: 0 };
        let least = btc_release_vbytes(tx.base().len(), &sizes) * floor;

        if fee < least {
            return Err(format!("a fee of {fee} is under the floor: this release pays at least {least}").into());
        }

        println!("proposing a sweep, txid {}", shown(&sha256d(&tx.base())));
        let number = self.proposal_count(&gateway)? + 1;

        self.post_proposal(&gateway, &secret, &tx, &[], number)
    }

    /// What a release is, from the gateway's state alone: what the signer goes over.
    fn release_facts(&self, gateway: &Address, found: &[Value], most_rate: u64) -> CliResult<ReleaseFacts> {
        let release = raw(found.first().ok_or("no such release")?);
        let (tx, _) = Transaction::read(&release)?;
        let version = found.get(3).map_or(0, whole);
        let mut withdrawals = Vec::new();

        for id in found.get(2).map(text).unwrap_or_default().split(',').filter(|id| !id.is_empty()) {
            let id: u64 = id.parse().map_err(|_| "a release names what is no withdrawal")?;
            let withdrawal = self.read(gateway, "withdrawal", json!([{ "type": "u64", "value": id.to_string() }]))?;
            withdrawals.push(NamedWithdrawal { id, script: raw(&withdrawal[0]), sats: whole(&withdrawal[1]), most_fee: whole(&withdrawal[4]), state: text(&withdrawal[2]), anchor: self.anchor(gateway, id)? });
        }

        let inputs = tx.inputs.iter().map(|input| {
            let outpoint = format!("{}:{}", to_hex(&input.txid).trim_start_matches("0x"), input.vout);
            let held = self.spent_output(gateway, &outpoint)?;

            Ok((outpoint, held))
        }).collect::<CliResult<Vec<_>>>()?;
        let change_script = raw(self.read(gateway, "change_script", json!([{ "type": "u64", "value": version.to_string() }]))?.first().ok_or("no change script")?);
        let (fee_cap, fee_floor) = self.fee_limits(gateway)?;
        // What it repays, as the gateway worked it out, checked against what is owed now in all.
        let (owed, _, policy) = self.repay_policy(gateway)?;
        let shares = found.get(9).map(text).unwrap_or_default().split(',').filter_map(|share| share.trim().parse().ok()).collect();
        let repay = ReleaseRepay { shares, budget: found.get(10).map_or(0, whole), owed, ..policy };

        Ok(ReleaseFacts { raw: release, txid: raw(found.get(1).ok_or("no transaction id")?), version, withdrawals, inputs, change_script, fee_cap, fee_floor, most_rate, repay })
    }

    /// Goes over every open release against the gateway's state, and does for each what
    /// btc_release_step says: signs each input of those that stand and that this signer has not
    /// signed; and given voting, as a pass is and an operator signing by hand is not, votes down what it
    /// refuses for good, and what is past the age limit. One going wrong is said and passed over, for
    /// the next one, and a read that fails is one of those: nothing is voted on half of what it knows.
    fn sign_releases(&self, rates: &Result<Option<FeeRates>, String>, voting: bool) -> CliResult<()> {
        let (gateway, secret) = (self.btc_gateway()?, self.evm_secret()?);
        let count = whole(self.read(&gateway, "proposal_count", json!([]))?.first().ok_or("no count")?);
        let my_key = crate::crypto::secp256k1::public_key(&secret).ok_or("--evm-key is no key")?;
        let (most_rate, blind) = self.signing_limit(&gateway, rates)?;
        let mut told = !blind;
        // Two open releases naming one withdrawal would pay it twice, should the gateway ever let
        // them: the first keeps it. Versions of one release name the same, as only one is mined.
        let mut paying = std::collections::HashMap::new();
        let mut open = Vec::new();

        for id in (self.btc_done.get().releases + 1).min(count + 1)..=count {
            let found = self.read(&gateway, "proposal", json!([{ "type": "u64", "value": id.to_string() }]))?;

            if found.get(4).map(text).as_deref() == Some("open") {
                open.push((id, found));
            }
        }

        let ages = match voting && !open.is_empty() {
            true => self.release_ages(&gateway, &open.iter().map(|(id, _)| *id).collect::<Vec<_>>()).map(Some).unwrap_or_else(|why| {
                eprintln!("how old each release is goes unread, so none gets a vote for its age: {why}");

                None
            }),
            false => None,
        };

        for (id, found) in open {
            // Fees going unread should not stop withdrawals, so it goes on, but no further than this.
            if !std::mem::replace(&mut told, true) {
                eprintln!("no fee rate to go by: signing for at most the fixed {most_rate} satoshis a virtual byte");
            }

            let past_limit = ages.as_ref().and_then(|ages| ages.get(&id)).map(|age| *age >= self.cancel_after());

            if let Err(why) = self.step_release(&gateway, id, &found, &my_key, &secret, &mut paying, most_rate, blind, past_limit, voting, rates) {
                eprintln!("release {id}: {why}");
            }
        }

        Ok(())
    }

    /// One open release: what this signer knows of it, read in full or not at all, and what that says
    /// to do. Every failure before the step is a read that failed, so none of them votes.
    #[allow(clippy::too_many_arguments)]
    fn step_release(&self, gateway: &Address, id: u64, found: &[Value], my_key: &[u8; 33], secret: &[u8; 32], paying: &mut std::collections::HashMap<u64, u64>, most_rate: u64, blind: bool, past_limit: Option<bool>, voting: bool, rates: &Result<Option<FeeRates>, String>) -> CliResult<()> {
        if self.read(gateway, "voted", json!([{ "type": "u64", "value": id.to_string() }, { "type": "bytes", "value": to_hex(my_key) }]))?.first().is_some_and(|value| value["value"] == true) {
            return Err("this signer voted to cancel it".into());
        }

        let facts = self.release_facts(gateway, found, most_rate)?;
        let (tx, _) = Transaction::read(&facts.raw)?;
        let (root, mut refusal) = self.refusal(gateway, id, found, &facts, most_rate, rates)?;

        if refusal.is_none()
            && let Some(withdrawal) = facts.withdrawals.iter().find(|withdrawal| *paying.entry(withdrawal.id).or_insert(root) != root)
        {
            refusal = Some(ReleaseRefusal::Wrong(format!("another open release pays withdrawal {}", withdrawal.id)));
        }

        let mut signatures = Vec::new();
        let mut signed_by_me = false;

        for (index, (_, held)) in facts.inputs.iter().enumerate() {
            let posted = self.read(gateway, "input_signatures", json!([{ "type": "u64", "value": id.to_string() }, { "type": "u64", "value": index.to_string() }]))?;
            let script = raw(posted.first().ok_or("no witness script")?);
            let signature = crate::crypto::secp256k1::sign_hash(secret, &sha256d(&tx.script_preimage(index, &script, held.sats)));
            let mine = posted.iter().skip(2).any(|value| raw(value) == signature);
            signed_by_me |= mine;
            signatures.push((index, signature, mine));
        }

        let standing = ReleaseStanding { refusal, blind, signed_by_me, signed_enough: self.signed_enough(gateway, id, found)?, past_limit };

        match btc_release_step(&standing) {
            ReleaseStep::Vote(why) if voting => {
                println!("voting to cancel release {id}: {why}");

                if !self.my_turn(gateway, Turn::Cancel(id))? {
                    println!("release {id} has the votes it needs, or will");

                    return Ok(());
                }

                self.vote_cancel(id)
            }
            ReleaseStep::Vote(why) | ReleaseStep::Leave(why) => Err(format!("not signing: {why}").into()),
            ReleaseStep::Nothing => Ok(()),
            ReleaseStep::Sign => {
                for (index, signature, mine) in signatures {
                    if !mine {
                        println!("signing release {id}, input {index}");
                        self.call(*gateway, "sign", u256::ZERO, vec![TxCallArgumentType::U64(id), TxCallArgumentType::U64(index as u64), TxCallArgumentType::Bytes(signature.to_vec())])?;
                    }
                }

                Ok(())
            }
        }
    }

    /// Why this signer will not sign a proposal, read in full, with the release it is of: a release as
    /// check_btc_release has it; a bump as check_btc_replacement or check_btc_child has it, against
    /// what it replaces or pays for, and only while what it bumps is stuck, as needless has it.
    fn refusal(&self, gateway: &Address, id: u64, found: &[Value], facts: &ReleaseFacts, most_rate: u64, rates: &Result<Option<FeeRates>, String>) -> CliResult<(u64, Option<ReleaseRefusal>)> {
        if found.get(5).is_none_or(|kind| text(kind) == "release") {
            return Ok((id, check_btc_release(facts).err()));
        }

        let members = self.tree(gateway, id)?;
        let root = members.first().map_or(id, |member| member.id);
        let me = members.iter().find(|member| member.id == id).ok_or("it is not in its release's tree")?;
        let others = members.iter().filter(|member| member.group == me.group && member.state == "open" && member.id != id).map(|member| member.fee).max().unwrap_or(0);
        let of = members.iter().find(|member| Some(member.id) == me.of).ok_or("what it bumps is not in its release's tree")?;
        let checked = match paid_for(&members, me) {
            None => check_btc_replacement(&self.release_facts(gateway, &of.found, most_rate)?, facts, others),
            Some(parent) => {
                let parent = members.iter().find(|member| member.id == parent).ok_or("what it pays for is not in its release's tree")?;

                check_btc_child(&self.release_facts(gateway, &parent.found, most_rate)?, facts, (me.kind == "replacement").then_some(others))
            }
        };

        match checked {
            Err(why) => Ok((root, Some(why))),
            Ok(()) => Ok((root, self.needless(gateway, &members, me, rates)?.map(ReleaseRefusal::Needless))),
        }
    }

    /// Why a bump is needless, as this signer's Esplora says, if it is: a version of its release is
    /// mined, or what it bumps pays what the network asks now, with any child of it it replaces. With
    /// no rate from the network, only mined says so.
    fn needless(&self, gateway: &Address, members: &[Member], me: &Member, rates: &Result<Option<FeeRates>, String>) -> CliResult<Option<String>> {
        let root = members.first().map_or(me.id, |member| member.id);

        for version in members.iter().filter(|member| member.group == root && member.id != me.id) {
            let status: Value = serde_json::from_str(&self.esplora(&format!("/tx/{}/status", version.txid()))?).unwrap_or(Value::Null);

            if status["confirmed"] == true {
                return Ok(Some(format!("release {root} is mined already, as {}", version.txid())));
            }
        }

        let Some(asked) = rates.as_ref().ok().copied().flatten().map(|rates| rates.soon.ceil() as u64) else {
            return Ok(None);
        };
        let find = |id: Option<u64>| members.iter().find(|member| Some(member.id) == id).ok_or("what it bumps is not in its release's tree");
        let rate = match paid_for(members, me) {
            None => self.paying(gateway, find(me.of)?, None)?,
            Some(parent) if me.kind == "child" => self.paying(gateway, find(Some(parent))?, None)?,
            Some(parent) => self.paying(gateway, find(Some(parent))?, Some(find(me.of)?))?,
        };

        Ok((rate >= asked).then(|| format!("what it bumps pays {rate} satoshis a virtual byte, and the network asks {asked}")))
    }

    /// A signer's vote to cancel a release, with --evm-key, posted with --key.
    fn vote_cancel(&self, proposal: u64) -> CliResult<()> {
        let (gateway, secret, here) = (self.btc_gateway()?, self.evm_secret()?, self.here()?);
        let found = self.read(&gateway, "proposal", json!([{ "type": "u64", "value": proposal.to_string() }]))?;
        let txid: [u8; 32] = raw(found.get(1).ok_or("no such release")?).try_into().map_err(|_| "no such release")?;
        let signature = sign_digest(&secret, &btc_cancel_digest(here, &gateway, proposal, &txid));

        self.call(gateway, "cancel", u256::ZERO, vec![TxCallArgumentType::U64(proposal), TxCallArgumentType::Bytes(signature.to_vec())])
    }

    /// Assembles a release's witnesses, each its threshold of signatures in the keys' order and
    /// its script, and broadcasts it through Esplora.
    fn finish(&self, proposal: u64) -> CliResult<()> {
        let gateway = self.btc_gateway()?;
        let found = self.read(&gateway, "proposal", json!([{ "type": "u64", "value": proposal.to_string() }]))?;
        let (mut tx, _) = Transaction::read(&raw(found.first().ok_or("no such release")?))?;

        if found.get(4).map(text).as_deref() != Some("open") {
            return Err(format!("release {proposal} is {}", found.get(4).map(text).unwrap_or_default()).into());
        }

        for index in 0..tx.inputs.len() {
            let posted = self.read(&gateway, "input_signatures", json!([{ "type": "u64", "value": proposal.to_string() }, { "type": "u64", "value": index.to_string() }]))?;
            let script = raw(posted.first().ok_or("no witness script")?);
            let threshold = posted.get(1).map_or(0, whole) as usize;
            let signatures: Vec<Vec<u8>> = posted[2..]
                .chunks(2)
                .filter_map(|pair| <[u8; 64]>::try_from(raw(pair.get(1)?)).ok())
                .map(|signature| [der(&signature), vec![0x01]].concat())
                .take(threshold)
                .collect();

            if signatures.len() < threshold {
                return Err(format!("input {index} has {} of the {threshold} signatures it needs", signatures.len()).into());
            }

            // CHECKMULTISIG's extra pop, then the signatures, then the script.
            tx.inputs[index].witness = [vec![Vec::new()], signatures, vec![script]].concat();
        }

        let base = self.args.esplora.as_deref().ok_or("give --esplora")?;
        let mut answer = self.agent.post(format!("{}/tx", base.trim_end_matches('/'))).send(to_hex(&tx.encode()).trim_start_matches("0x").to_string()).map_err(|err| format!("Esplora would not take it: {err}"))?;
        let accepted = answer.status().is_success();
        let said = answer.body_mut().read_to_string()?;

        if !accepted {
            return Err(format!("Esplora would not take it: {}", said.trim()).into());
        }

        println!("broadcast {}", said.trim());

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use {super::*, std::time::Duration};

    #[test]
    fn any_success_from_fee_estimates_is_an_answer_as_mempool_space_says_203() {
        let body = || Ok("{\"1\":1}".to_string());

        for status in [200, 203] {
            assert_eq!(fee_answer(status, body).unwrap(), "{\"1\":1}", "{status}");
        }

        assert_eq!(fee_answer(404, body).unwrap(), "{}", "an Esplora with no estimates");

        for status in [301, 429, 500, 503] {
            assert!(fee_answer(status, body).unwrap_err().contains(&status.to_string()), "{status}");
        }
    }

    #[test]
    fn what_the_network_asks_is_read_from_esplora_s_estimates() {
        let rates = fee_rates_from(r#"{"1":87.5,"2":40.1,"3":20.3,"6":9.0,"144":1.0,"1008":1.0}"#).unwrap().unwrap();

        assert_eq!(rates, FeeRates { fastest: 87.5, soon: 20.3 });
        // Without an estimate for that many blocks, the next there is; without any later, the fastest.
        assert_eq!(fee_rates_from(r#"{"1":8,"6":2}"#).unwrap().unwrap(), FeeRates { fastest: 8.0, soon: 2.0 });
        assert_eq!(fee_rates_from(r#"{"1":8}"#).unwrap().unwrap(), FeeRates { fastest: 8.0, soon: 8.0 });
        assert_eq!(fee_rates_from("{}").unwrap(), None, "regtest's: no rate at all");
        assert_eq!(fee_rates_from(r#"{"1":0,"2":-3}"#).unwrap(), None, "nothing that is a rate");
        assert!(fee_rates_from("<html>slow down</html>").is_err(), "an answer that is no such thing is no answer");
    }

    #[test]
    fn a_throttled_esplora_is_asked_again_waiting_longer_each_time() {
        let mut answers = vec![Err("timeout".to_string()), Ok("<html>slow down</html>".to_string()), Ok("00000000a1".to_string())].into_iter();
        let mut waited = Vec::new();

        assert_eq!(asked_again(6, |wait| waited.push(wait), || answers.next().unwrap()), Ok("00000000a1".to_string()));
        assert_eq!(waited, [Duration::from_secs(2), Duration::from_secs(4)], "twice as long each time");

        let mut waited = Vec::new();
        let failed = asked_again(3, |wait| waited.push(wait), || Ok("  <!doctype html>".to_string())).unwrap_err();

        assert!(failed.contains("web page") && failed.contains("3 times"), "{failed}");
        assert_eq!(waited.len(), 2);
        assert_eq!(asked_again(0, |_| {}, || Ok("1".to_string())), Ok("1".to_string()), "asked once at least");
    }

    #[test]
    fn deposit_addresses_are_written_for_the_bitcoin_the_chain_follows() {
        let mainnet = GENESIS_BLOCKS[0].0;
        let testnet4 = GENESIS_BLOCKS[1].0;

        // The relay says first, whatever Esplora serves; Esplora says when the relay does not.
        assert_eq!(pick_network(Some("test"), Some(mainnet), None).unwrap().0, Followed::Test);
        assert_eq!(pick_network(Some("regtest"), None, None).unwrap().0, Followed::Regtest);
        assert_eq!(pick_network(None, Some(testnet4), None).unwrap().0, Followed::Test);
        assert_eq!(pick_network(None, Some(&mainnet.to_uppercase()), None).unwrap().0, Followed::Main);
        // Told, it goes only where nothing says, and is refused where the chain says another.
        assert_eq!(pick_network(None, Some("00ff"), Some("test")).unwrap().0, Followed::Test);
        assert!(pick_network(Some("test"), None, Some("main")).unwrap_err().to_string().contains("not the Bitcoin here"));
        assert!(pick_network(None, Some(mainnet), Some("regtest")).is_err());
        assert!(pick_network(None, None, Some("testnet")).is_err(), "the relay's names alone");
        // Nothing said: main, saying so.
        let (network, why) = pick_network(None, None, None).unwrap();
        assert_eq!(network, Followed::Main);
        assert!(why.contains("nothing says"), "{why}");

        // One deposit script, as each network writes it.
        let script = p2wsh(&[0x51]);
        assert!(Followed::Main.address(&script).starts_with("bc1q"));
        assert!(Followed::Test.address(&script).starts_with("tb1q"));
        assert!(Followed::Regtest.address(&script).starts_with("bcrt1q"));
        assert_eq!(crate::structs::bech32::decode_segwit(&Followed::Regtest.address(&script)).map(|(hrp, program)| (hrp, program.len())), Some(("bcrt".to_string(), 32)));
    }
}
