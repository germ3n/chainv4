//! `chainv4 bridge eth`: Ethereum's beacon chain for anchoring. relay-start starts the Ethereum
//! relay (contracts/examples/eth_relay) from a beacon API's bootstrap at a root it trusts; relay-sync
//! feeds it the light client updates and the newest finality update, run on a loop as the
//! chainv4-eth-relay service. Nothing a beacon API serves is taken on its word: the relay checks every
//! signature and branch. The anchorer's Ethereum leg is here too: writing an anchor as calldata of a
//! transaction to the key's own address, and proving it once a finality update finalizes its block.

use {
    super::Bridge,
    crate::cli::CliResult,
    crate::structs::address::Address,
    crate::structs::hex::{from_hex, to_hex},
    crate::structs::tx::TxCallArgumentType,
    crate::structs::u256::u256,
    ethlight::{LightClientUpdate, Network},
    serde_json::{Value, json},
};

#[derive(clap::Subcommand)]
pub enum EthAction {
    /// Starts the relay, deployed from contracts/examples/eth_relay, at a beacon block root you
    /// trust: a finalized checkpoint's, whose bootstrap --beacon serves. Its network is --network,
    /// or else the one the chain's anchoring names.
    RelayStart {
        relay: Address,
        /// The trusted beacon block root: 0x and 64 hex digits.
        root: String,
        /// mainnet, sepolia or hoodi; without it, the network anchoring's eth_start names.
        #[arg(long)]
        network: Option<String>,
    },
    /// Submits to the relay the best update of each period from its own to now, then the newest
    /// finality update, each only when the relay would take it, a transaction each.
    RelaySync {
        relay: Address,
        /// Seconds between passes, for ever; 0 for one pass. An epoch is 384 seconds.
        #[arg(long, default_value_t = 0)]
        every: u64,
    },
}

/// A beacon network as anchoring's settings write it: a preset's name, or a definition's SSZ.
pub(super) fn network_from(value: &Value) -> CliResult<Network> {
    let named = |name: &str| crate::params::anchoring::EthNetwork::preset(name).ok_or_else(|| format!("{name} is not a network: mainnet, sepolia or hoodi"));
    let network = match value.as_str() {
        Some(text) if text.starts_with("0x") => crate::params::anchoring::EthNetwork::Custom(from_hex(text).ok_or("anchoring's network is not hex")?),
        Some(name) => named(name)?,
        None => return Err("anchoring names no Ethereum network: give --network".into()),
    };

    Ok(network.definition()?)
}

fn bytes(value: &[u8]) -> TxCallArgumentType {
    TxCallArgumentType::Bytes(value.to_vec())
}

fn bytes_json(value: &[u8]) -> Value {
    json!({ "type": "bytes", "value": to_hex(value) })
}

/// A read's u64 at a place.
fn number(values: &[Value], at: usize) -> Option<u64> {
    let value = &values.get(at)?["value"];

    value.as_u64().or_else(|| value.as_str()?.parse().ok())
}

impl Bridge {
    /// The beacon network the chain's anchoring names.
    fn anchored_network(&self) -> CliResult<Network> {
        network_from(&self.anchors()?["settings"]["eth_start"]["network"])
    }

    pub(super) fn eth(&self, action: &EthAction) -> CliResult<()> {
        match action {
            EthAction::RelayStart { relay, root, network } => {
                let root: [u8; 32] = from_hex(root).and_then(|bytes| bytes.try_into().ok()).ok_or("the root is 0x and 64 hex digits")?;
                let (definition, name) = match network {
                    Some(name) => (network_from(&json!(name))?, network_from(&json!(name))?.name.into_owned()),
                    None => (self.anchored_network()?, String::new()),
                };
                let bootstrap = self.args.beacon.beacon(&self.args.proxy)?.bootstrap(&root)?;
                ethlight::LightClientBootstrap::decode(&bootstrap, &definition).map_err(|err| format!("the beacon API's bootstrap: {err}"))?;
                eprintln!("starting the relay on {} at {}", definition.name, to_hex(&root));

                self.call(*relay, "start", u256::ZERO, vec![bytes(&bootstrap), TxCallArgumentType::String(name), bytes(&root)])
            }
            EthAction::RelaySync { relay, every } => loop {
                if let Err(why) = self.eth_relay_pass(relay) {
                    eprintln!("the Ethereum relay's pass: {why}");

                    if *every == 0 {
                        return Err(why);
                    }
                }

                if *every == 0 {
                    return Ok(());
                }

                std::thread::sleep(std::time::Duration::from_secs(*every));
            },
        }
    }

    /// The relay's network, as it follows it: the one anchoring names when it bears the relay's
    /// name, else the preset of that name.
    fn relay_network(&self, relay: &Address) -> CliResult<Network> {
        let name = self.read(relay, "network_name", json!([]))?.first().and_then(|value| value["value"].as_str().map(str::to_string)).ok_or("the relay has not started")?;

        match self.anchored_network() {
            Ok(network) if network.name == name.as_str() => Ok(network),
            _ => Ok(Network::by_name(&name).cloned().ok_or_else(|| format!("the relay follows {name}, which anchoring does not define and no preset is"))?),
        }
    }

    /// Submits an update the relay would take, as SSZ: whether it took it.
    fn submit_update(&self, relay: &Address, update: &[u8]) -> CliResult<bool> {
        match self.read(relay, "submit", json!([bytes_json(update)])) {
            Ok(_) => {
                self.call(*relay, "submit", u256::ZERO, vec![bytes(update)])?;

                Ok(true)
            }
            Err(why) => {
                log::info!("the relay would not take an update: {why}");

                Ok(false)
            }
        }
    }

    /// One pass of the relay's feeder: each period's best update from the relay's own, then the
    /// newest finality update.
    fn eth_relay_pass(&self, relay: &Address) -> CliResult<()> {
        let beacon = self.args.beacon.beacon(&self.args.proxy)?;
        let network = self.relay_network(relay)?;
        let now = crate::node::now_ms() / 1_000;
        let current = network.period_at_slot(network.slot_at(now));
        let period = number(&self.read(relay, "period", json!([]))?, 0).ok_or("the relay gives no period")?;

        if period <= current {
            for update in beacon.updates(period, current - period + 1)? {
                if self.submit_update(relay, &update)? {
                    println!("submitted a period's update");
                }
            }
        }

        let held = number(&self.read(relay, "finalized", json!([]))?, 0).unwrap_or(0);
        let update = LightClientUpdate::decode_finality(&beacon.finality_update()?, &network).map_err(|err| format!("the beacon API's finality update: {err}"))?;

        if update.finalized_header.beacon.slot > held && self.submit_update(relay, &update.encode(&network))? {
            println!("the relay holds slot {} finalized", update.finalized_header.beacon.slot);
        }

        Ok(())
    }
}

#[cfg(feature = "anchoring-eth")]
pub(super) use anchorer::EthPending;

#[cfg(feature = "anchoring-eth")]
mod anchorer {
    use {
        super::*,
        crate::anchor::{Payload, ethereum::Native},
        crate::cli::{bridge::anchor::Step, send::TxArgs, swap::rpc::from_key},
        crate::structs::tx::{AnchorProof, EthAnchorProof},
        crate::swap::evm::Call,
    };

    /// How long before an epoch's first slot an anchor is sent, so it lands in that slot's block:
    /// the block an epoch's checkpoint is, which a finality update then finalizes itself.
    const LEAD_SECS: u64 = 8;

    /// An anchor written into Ethereum and not yet shown to the chain.
    pub(in crate::cli::bridge) struct EthPending {
        pub(in crate::cli::bridge) tx_hash: String,
        pub(in crate::cli::bridge) epoch: u64,
        pub(in crate::cli::bridge) root: String,
    }

    fn hex_number(value: &Value) -> Option<u64> {
        u64::from_str_radix(value.as_str()?.trim_start_matches("0x"), 16).ok()
    }

    impl Bridge {
        /// Sends the anchor's 80 bytes as calldata of a transaction from --evm-key to itself,
        /// timed to land in an epoch's first slot, and waits for it to be mined.
        pub(in crate::cli::bridge) fn write_eth_anchor(&self, payload: &Payload, anchors: &Value) -> CliResult<EthPending> {
            let network = network_from(&anchors["settings"]["eth_start"]["network"])?;
            let (rpc, secret) = (self.rpc()?, self.evm_secret()?);
            let now = crate::node::now_ms() / 1_000;
            let per_epoch = network.slots_per_epoch * network.seconds_per_slot;
            let mut boundary = (network.slot_at(now) / network.slots_per_epoch + 1) * network.slots_per_epoch;

            if network.genesis_time + boundary * network.seconds_per_slot < now + LEAD_SECS {
                boundary += network.slots_per_epoch;
            }

            let send_at = network.genesis_time + boundary * network.seconds_per_slot - LEAD_SECS;
            println!("writing epoch {}'s checkpoint into Ethereum in {} s, to land in slot {boundary}, an epoch's first ({per_epoch} s an epoch)", payload.epoch, send_at.saturating_sub(now));
            std::thread::sleep(std::time::Duration::from_secs(send_at.saturating_sub(now)));
            let call = Call { to: from_key(&secret)?, value: u256::ZERO, input: payload.encode().to_vec(), what: "Write the anchor into Ethereum" };
            let receipt = rpc.send(&call, false, &secret)?;
            let tx_hash = receipt["transactionHash"].as_str().ok_or("the anchor's receipt names no transaction")?.to_string();

            Ok(EthPending { tx_hash, epoch: payload.epoch, root: anchors["root"].as_str().unwrap_or_default().to_string() })
        }

        /// Shows a pending Ethereum anchor to the chain once a finality update finalizes its block
        /// itself: the update submitted to the relay should it not hold the block, the
        /// transaction's place proven from its block's transactions, and Tx::Anchor sent. An anchor
        /// whose block is no epoch's first, or that a finality update went past before a pass saw
        /// it, cannot be proven from what a beacon API serves, and is past use.
        pub(in crate::cli::bridge) fn show_eth_anchor(&self, written: &EthPending, anchors: &Value) -> CliResult<Step> {
            let settings = &anchors["settings"];
            let network = network_from(&settings["eth_start"]["network"])?;
            let relay: Address = settings["eth_relay"].as_str().ok_or("anchoring names no Ethereum relay")?.parse().map_err(|err| format!("anchoring's eth_relay: {err}"))?;
            let rpc = self.rpc()?;
            let receipt = rpc.ask("eth_getTransactionReceipt", json!([written.tx_hash]))?;

            if receipt.is_null() {
                println!("{} is not mined: writing it again", written.tx_hash);

                return Ok(Step::Done);
            }

            let (number, index) = (hex_number(&receipt["blockNumber"]).ok_or("a receipt with no block")?, hex_number(&receipt["transactionIndex"]).ok_or("a receipt with no place")?);
            let block = rpc.ask("eth_getBlockByNumber", json!([format!("{number:#x}"), false]))?;
            let slot = network.slot_at(hex_number(&block["timestamp"]).ok_or("a block with no time")?);

            if !slot.is_multiple_of(network.slots_per_epoch) {
                println!("{} landed in slot {slot}, no epoch's first: proving it would take an ancestry proof no beacon API's light client routes serve. Writing it again", written.tx_hash);

                return Ok(Step::Done);
            }

            let update = LightClientUpdate::decode_finality(&self.args.beacon.beacon(&self.args.proxy)?.finality_update()?, &network).map_err(|err| format!("the beacon API's finality update: {err}"))?;
            let finalized = update.finalized_header.beacon.slot;

            if finalized < slot {
                println!("{} is in slot {slot}, finalized to {finalized}: waiting", written.tx_hash);

                return Ok(Step::Wait);
            }

            if finalized > slot || to_hex(&update.finalized_header.execution.block_hash) != block["hash"].as_str().unwrap_or_default() {
                println!("finality went past slot {slot} before a pass saw it, or its checkpoint is another block: writing it again");

                return Ok(Step::Done);
            }

            let root = update.finalized_header.beacon.hash_tree_root(&Native);
            let held = self.read(&relay, "header", json!([bytes_json(&root)]))?;

            if held.first().is_none_or(|value| value["type"].as_str() != Some("bytes")) && !self.submit_update(&relay, &update.encode(&network))? {
                return Err("the relay holds no such header and would not take its finality update: run bridge eth relay-sync".into());
            }

            let count = block["transactions"].as_array().map_or(0, Vec::len);
            let mut transactions = Vec::with_capacity(count);

            for from in (0..count).step_by(100) {
                let calls: Vec<(&str, Value)> = (from..count.min(from + 100)).map(|at| ("eth_getRawTransactionByBlockNumberAndIndex", json!([format!("{number:#x}"), format!("{at:#x}")]))).collect();

                for raw in rpc.ask_batch(&calls)? {
                    transactions.push(raw.as_str().and_then(from_hex).ok_or("a raw transaction that is not hex")?);
                }
            }

            if ethlight::tx::transactions_root(&Native, &transactions) != update.finalized_header.execution.transactions_root {
                return Err("the EVM chain's transactions do not make the block's transactions root".into());
            }

            let tx = transactions.get(index as usize).cloned().ok_or("the anchor is past its block's transactions")?;
            let proof = AnchorProof::Ethereum(EthAnchorProof {
                header: update.finalized_header.encode(network.fork_at_slot(slot).fork),
                root,
                tx,
                index,
                branch: ethlight::tx::transaction_branch(&Native, &transactions, index),
                ancestry: None,
                update: update.encode(&network),
            });
            let encoded = wincode::serialize(&proof).map_err(|err| format!("encoding the proof: {err}"))?;
            self.tx(TxArgs::Anchor { proof: to_hex(&encoded) })?;

            Ok(Step::Done)
        }
    }
}
