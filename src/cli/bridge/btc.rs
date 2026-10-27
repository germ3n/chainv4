//! `chainv4 bridge btc`: Bitcoin's side of the bridges. The light client contract
//! (contracts/examples/btc_relay) is started at a checkpoint and kept in sync from an Esplora API;
//! the Bitcoin gateway (contracts/lua/btc_gateway.lua) mints wBTC for deposits shown with their
//! proofs, burns it for withdrawals, and pays them out through releases proposed, signed by the
//! federation input by input, assembled, broadcast and proven here.

use {
    super::Bridge,
    crate::cli::CliResult,
    crate::crypto::bitcoin::sha256d,
    crate::crypto::bitcoin::tx::{Network, Transaction, TxIn, TxOut, address, der, p2wsh},
    crate::structs::address::Address,
    crate::structs::hex::{from_hex, to_hex},
    crate::structs::tx::TxCallArgumentType,
    crate::structs::u256::u256,
    serde_json::{Value, json},
};

/// An output smaller than this goes to the fee rather than to change: Bitcoin's dust, about.
const DUST: u64 = 546;
/// The most withdrawals one release pays, as the gateway allows.
const MOST_WITHDRAWALS: usize = 50;

#[derive(clap::Subcommand)]
pub enum BtcAction {
    /// Starts the relay, deployed from contracts/examples/btc_relay, at the header Esplora has at
    /// a height: on main, a difficulty period's first, a height divisible by 2016.
    RelayStart {
        relay: Address,
        height: u64,
        /// main, regtest or test.
        #[arg(long, default_value = "main")]
        network: String,
    },
    /// Submits to the relay every header Esplora has past the relay's tip.
    RelaySync {
        relay: Address,
        /// Headers a transaction.
        #[arg(long, default_value_t = 500)]
        batch: usize,
    },
    /// Deploys the Bitcoin gateway, with wBTC's template, on a relay, printing where.
    Deploy {
        relay: Address,
        /// The most fee a release may pay, in satoshis.
        #[arg(long, default_value_t = 50_000)]
        fee_cap: u64,
        /// How long an old federation's deposit addresses stay open, in hours.
        #[arg(long, default_value_t = 168)]
        grace_hours: u64,
    },
    /// An address's deposit address on Bitcoin, under the federation in force.
    Address {
        recipient: Address,
        /// main or test: which addresses to write it as.
        #[arg(long, default_value = "main")]
        network: String,
    },
    /// Shows a deposit, by its transaction and output, minting its wBTC to the address it pays.
    Deposit { txid: String, vout: u32, recipient: Address },
    /// Burns wBTC to be paid out to a Bitcoin address, bc1q…, or an output script, 0x….
    Withdraw { address: String, sats: u64 },
    /// Proposes a release paying the queued withdrawals from what the gateway holds, with this fee.
    Propose {
        #[arg(long)]
        fee: u64,
    },
    /// Proposes a sweep of everything older federations hold to the one in force, with this fee.
    Sweep {
        #[arg(long)]
        fee: u64,
    },
    /// A federation signer's pass: signs every input of every open release, with --evm-key.
    Sign,
    /// Assembles an open release's witnesses from the signatures posted, and broadcasts it.
    Finish { proposal: u64 },
    /// Shows a release confirmed on Bitcoin, spending its inputs and holding its change.
    Released { proposal: u64 },
}

/// A transaction without its witness, its block, its place in it, and its Merkle path there.
struct Proof {
    tx: Vec<u8>,
    block: [u8; 32],
    index: u64,
    path: Vec<u8>,
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

fn network(name: &str) -> CliResult<Network> {
    match name {
        "main" => Ok(Network::Main),
        "test" => Ok(Network::Test),
        other => Err(format!("{other} is not a network: main or test").into()),
    }
}

/// A read's value as text, a number, or bytes.
fn text(value: &Value) -> String {
    value["value"].as_str().map_or_else(|| value["value"].to_string(), str::to_string)
}

fn raw(value: &Value) -> Vec<u8> {
    match value["type"].as_str() {
        Some("bytes") => value["value"].as_str().and_then(from_hex).unwrap_or_default(),
        _ => value["value"].as_str().unwrap_or_default().as_bytes().to_vec(),
    }
}

fn whole(value: &Value) -> u64 {
    text(value).parse().unwrap_or(0)
}

impl Bridge {
    fn esplora(&self, path: &str) -> CliResult<String> {
        let base = self.args.esplora.as_deref().ok_or("give --esplora: an Esplora API, such as https://mempool.space/api")?;
        let mut answer = self.agent.get(format!("{}{path}", base.trim_end_matches('/'))).call().map_err(|err| format!("Esplora {path}: {err}"))?;

        Ok(answer.body_mut().read_to_string()?)
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
        let hex = self.esplora(&format!("/tx/{txid}/hex"))?;
        let (tx, _) = Transaction::read(&from_hex(&format!("0x{}", hex.trim())).ok_or("Esplora gave a transaction that is not hex")?)?;
        let proof: Value = serde_json::from_str(&self.esplora(&format!("/tx/{txid}/merkle-proof"))?).map_err(|_| "Esplora gave a proof that is not JSON")?;
        let height = proof["block_height"].as_u64().ok_or("the transaction is not in a block yet")?;
        let block = internal(&self.esplora(&format!("/block-height/{height}"))?)?;
        let path = proof["merkle"].as_array().into_iter().flatten().map(|sibling| internal(sibling.as_str().unwrap_or_default())).collect::<CliResult<Vec<_>>>()?;

        Ok(Proof { tx: tx.base(), block, index: proof["pos"].as_u64().unwrap_or(0), path: path.concat() })
    }

    pub(super) fn btc(&self, action: &BtcAction) -> CliResult<()> {
        let bytes = TxCallArgumentType::Bytes;
        let number = TxCallArgumentType::U64;

        match action {
            BtcAction::RelayStart { relay, height, network } => {
                let hash = self.esplora(&format!("/block-height/{height}"))?;
                let header = from_hex(&format!("0x{}", self.esplora(&format!("/block/{}/header", hash.trim()))?.trim())).ok_or("Esplora gave a header that is not hex")?;
                self.call(*relay, "start", u256::ZERO, vec![bytes(header), number(*height), TxCallArgumentType::String(network.clone())])
            }
            BtcAction::RelaySync { relay, batch } => {
                let tip = self.read(relay, "tip", json!([]))?;
                let mut height = tip.get(1).map_or(0, whole);
                let top: u64 = self.esplora("/blocks/tip/height")?.trim().parse().map_err(|_| "Esplora gave a height that is not a number")?;

                while height < top {
                    let mut headers = Vec::new();

                    while height < top && headers.len() / 80 < *batch {
                        height += 1;
                        let hash = self.esplora(&format!("/block-height/{height}"))?;
                        headers.extend(from_hex(&format!("0x{}", self.esplora(&format!("/block/{}/header", hash.trim()))?.trim())).ok_or("Esplora gave a header that is not hex")?);
                    }

                    println!("submitting headers to {height}");
                    self.call(*relay, "submit", u256::ZERO, vec![bytes(headers)])?;
                }

                Ok(())
            }
            BtcAction::Deploy { relay, fee_cap, grace_hours } => {
                let me = crate::cli::signer(self.args.key.as_deref())?.public().address();
                let template = self.deploy_lua(&me, "wrapped.lua", super::WRAPPED_LUA)?;
                let gateway = self.deploy_lua(&me, "btc_gateway.lua", include_str!("../../../contracts/lua/btc_gateway.lua"))?;
                self.call(gateway, "setup", u256::ZERO, vec![TxCallArgumentType::Address(*relay), TxCallArgumentType::Address(template), number(*fee_cap), number(grace_hours * 3_600_000)])?;
                println!("btc gateway {gateway}");

                Ok(())
            }
            BtcAction::Address { recipient, network: name } => {
                let scripts = self.read(&self.btc_gateway()?, "deposit_script", json!([{ "type": "address", "value": recipient.to_string() }]))?;
                println!("{}", address(&raw(scripts.get(1).ok_or("the gateway gave no deposit script")?), network(name)?));

                Ok(())
            }
            BtcAction::Deposit { txid, vout, recipient } => {
                let Proof { tx, block, index, path } = self.proof(txid)?;
                let args = vec![bytes(block.to_vec()), bytes(tx), number(index), bytes(path), number(u64::from(*vout)), TxCallArgumentType::Address(*recipient), TxCallArgumentType::Nil];
                self.call(self.btc_gateway()?, "deposit", u256::ZERO, args)
            }
            BtcAction::Withdraw { address: text, sats } => {
                let script = match text.strip_prefix("0x") {
                    Some(_) => from_hex(text).ok_or("the script is not hex")?,
                    None => {
                        let (_, program) = crate::structs::bech32::decode_segwit(text).ok_or("give a native segwit address, bc1q…, or an output script, 0x…")?;
                        [&[0x00, program.len() as u8][..], &program].concat()
                    }
                };
                self.call(self.btc_gateway()?, "withdraw", u256::ZERO, vec![bytes(script), number(*sats)])
            }
            BtcAction::Propose { fee } => self.propose(*fee, false),
            BtcAction::Sweep { fee } => self.propose(*fee, true),
            BtcAction::Sign => self.sign_releases(),
            BtcAction::Finish { proposal } => self.finish(*proposal),
            BtcAction::Released { proposal } => {
                let found = self.read(&self.btc_gateway()?, "proposal", json!([{ "type": "u64", "value": proposal.to_string() }]))?;
                let txid = shown(&raw(found.get(1).ok_or("no such release")?));
                let Proof { tx, block, index, path } = self.proof(&txid)?;
                self.call(self.btc_gateway()?, "released", u256::ZERO, vec![number(*proposal), bytes(block.to_vec()), bytes(tx), number(index), bytes(path)])
            }
        }
    }

    /// What the gateway holds unspent: each outpoint, its satoshis and federation version.
    fn unspent(&self, gateway: &Address) -> CliResult<Vec<(String, u64, u64)>> {
        let count = whole(self.read(gateway, "utxo_count", json!([]))?.first().ok_or("no count")?);
        let mut found = Vec::new();

        for n in 0..count {
            let utxo = self.read(gateway, "utxo", json!([{ "type": "u64", "value": n.to_string() }]))?;

            if utxo.get(4).map(text).as_deref() == Some("unspent") {
                found.push((text(&utxo[0]), whole(&utxo[1]), whole(&utxo[2])));
            }
        }

        Ok(found)
    }

    /// Proposes a release of the queued withdrawals, or a sweep of older federations' outputs.
    fn propose(&self, fee: u64, sweep: bool) -> CliResult<()> {
        let gateway = self.btc_gateway()?;
        let version = self.status()?["federation"]["version"].as_u64().ok_or("this chain has no federation")?;
        let change = p2wsh(&raw(self.read(&gateway, "change_script", json!([]))?.first().ok_or("no change script")?));
        let mut ids = Vec::new();
        let mut outputs = Vec::new();

        if !sweep {
            let count = whole(self.read(&gateway, "withdrawal_count", json!([]))?.first().ok_or("no count")?);

            for id in 1..=count {
                let withdrawal = self.read(&gateway, "withdrawal", json!([{ "type": "u64", "value": id.to_string() }]))?;

                if withdrawal.get(2).map(text).as_deref() == Some("queued") && ids.len() < MOST_WITHDRAWALS {
                    ids.push(id);
                    outputs.push(TxOut { value: whole(&withdrawal[1]), script: raw(&withdrawal[0]) });
                }
            }

            if ids.is_empty() {
                return Err("no withdrawals are queued".into());
            }
        }

        let wanted = outputs.iter().map(|output| output.value).sum::<u64>() + fee;
        let mut inputs = Vec::new();
        let mut held = 0;

        for (outpoint, sats, at) in self.unspent(&gateway)? {
            if (sweep && at >= version) || (!sweep && held >= wanted) {
                continue;
            }

            let (txid, vout) = outpoint.split_once(':').ok_or("an outpoint that does not read")?;
            // The gateway keys what it holds by the id in Bitcoin's own byte order.
            let id: [u8; 32] = from_hex(&format!("0x{txid}")).and_then(|bytes| bytes.try_into().ok()).ok_or("an outpoint that does not read")?;
            inputs.push(TxIn { txid: id, vout: vout.parse()?, script_sig: Vec::new(), sequence: 0xffff_ffff, witness: Vec::new() });
            held += sats;
        }

        if inputs.is_empty() || held < wanted {
            return Err(format!("the gateway holds {held} satoshis to spend, short of {wanted}").into());
        }

        // Change back to the federation, or to the fee when it is dust.
        if held - wanted > DUST || sweep {
            outputs.push(TxOut { value: held - wanted, script: change });
        }

        let tx = Transaction { version: 2, inputs, outputs, lock_time: 0 };
        let mut args = vec![TxCallArgumentType::Bytes(tx.base())];
        args.extend(ids.iter().map(|id| TxCallArgumentType::U64(*id)));
        println!("proposing {} paying {} withdrawals, txid {}", if sweep { "a sweep" } else { "a release" }, ids.len(), shown(&sha256d(&tx.base())));

        self.call(gateway, "propose", u256::ZERO, args)
    }

    /// Signs every input of every open release that this signer has not signed.
    fn sign_releases(&self) -> CliResult<()> {
        let (gateway, secret) = (self.btc_gateway()?, self.evm_secret()?);
        let count = whole(self.read(&gateway, "proposal_count", json!([]))?.first().ok_or("no count")?);
        let unspent_or_pending = |outpoint: &str| -> CliResult<u64> {
            let total = whole(self.read(&gateway, "utxo_count", json!([]))?.first().ok_or("no count")?);

            for n in 0..total {
                let utxo = self.read(&gateway, "utxo", json!([{ "type": "u64", "value": n.to_string() }]))?;

                if text(&utxo[0]) == outpoint {
                    return Ok(whole(&utxo[1]));
                }
            }

            Err(format!("the gateway does not hold {outpoint}").into())
        };

        for id in 1..=count {
            let found = self.read(&gateway, "proposal", json!([{ "type": "u64", "value": id.to_string() }]))?;

            if found.get(4).map(text).as_deref() != Some("open") {
                continue;
            }

            let (tx, _) = Transaction::read(&raw(&found[0]))?;

            for (index, input) in tx.inputs.iter().enumerate() {
                let posted = self.read(&gateway, "input_signatures", json!([{ "type": "u64", "value": id.to_string() }, { "type": "u64", "value": index.to_string() }]))?;
                let script = raw(posted.first().ok_or("no witness script")?);
                let sats = unspent_or_pending(&format!("{}:{}", to_hex(&input.txid).trim_start_matches("0x"), input.vout))?;
                let signature = crate::crypto::secp256k1::sign_hash(&secret, &sha256d(&tx.script_preimage(index, &script, sats)));

                if posted.iter().skip(2).any(|value| raw(value) == signature) {
                    continue;
                }

                println!("signing release {id}, input {index}");
                self.call(gateway, "sign", u256::ZERO, vec![TxCallArgumentType::U64(id), TxCallArgumentType::U64(index as u64), TxCallArgumentType::Bytes(signature.to_vec())])?;
            }
        }

        Ok(())
    }

    /// Assembles a release's witnesses, each its threshold of signatures in the keys' order and
    /// its script, and broadcasts it through Esplora.
    fn finish(&self, proposal: u64) -> CliResult<()> {
        let gateway = self.btc_gateway()?;
        let found = self.read(&gateway, "proposal", json!([{ "type": "u64", "value": proposal.to_string() }]))?;
        let (mut tx, _) = Transaction::read(&raw(found.first().ok_or("no such release")?))?;

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
        println!("broadcast {}", answer.body_mut().read_to_string()?.trim());

        Ok(())
    }
}
