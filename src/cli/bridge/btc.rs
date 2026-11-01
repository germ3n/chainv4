//! `chainv4 bridge btc`: Bitcoin's side of the bridges. The light client contract
//! (contracts/examples/btc_relay) is started at a checkpoint and kept in sync from an Esplora API;
//! the Bitcoin gateway (contracts/lua/btc_gateway.lua) mints wBTC for deposits shown with their
//! proofs, burns it for withdrawals, and pays them out through releases proposed, signed by the
//! federation input by input, assembled, broadcast and proven here.

use {
    super::Bridge,
    crate::bridge::{BTC_DUST, NamedWithdrawal, ReleaseFacts, SpentOutput, btc_cancel_digest, btc_fee_share, btc_release_vbytes, check_btc_release, sign_digest},
    crate::cli::CliResult,
    crate::crypto::bitcoin::sha256d,
    crate::crypto::bitcoin::tx::{Network, Transaction, TxIn, TxOut, address, der, p2wsh},
    crate::structs::address::Address,
    crate::structs::hex::{from_hex, to_hex},
    crate::structs::tx::TxCallArgumentType,
    crate::structs::u256::u256,
    serde_json::{Value, json},
};

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
    /// An address's deposit address on Bitcoin, under the federation in force.
    Address {
        recipient: Address,
        /// main or test: which addresses to write it as.
        #[arg(long, default_value = "main")]
        network: String,
    },
    /// Shows a deposit, by its transaction and output, minting its wBTC to the address it pays.
    Deposit { txid: String, vout: u32, recipient: Address },
    /// Burns wBTC to be paid out to a Bitcoin address, bc1q…, or an output script, 0x…. What arrives
    /// is the amount less its share of its release's fee.
    Withdraw {
        address: String,
        sats: u64,
        /// The most it may lose to that share, in satoshis: a tenth of it unless told.
        #[arg(long)]
        most_fee: Option<u64>,
    },
    /// Changes the most a queued withdrawal of yours may lose to fees, as fees rise past it.
    SetMostFee { withdrawal: u64, most_fee: u64 },
    /// Proposes a release paying the queued withdrawals from what the gateway holds, with this fee,
    /// which comes off them evenly. One that agreed to lose less than its share waits.
    Propose {
        #[arg(long)]
        fee: u64,
    },
    /// Proposes a sweep of everything older federations hold to the one in force, with this fee.
    Sweep {
        #[arg(long)]
        fee: u64,
    },
    /// A federation signer's pass, with --evm-key: goes over every open release against the
    /// gateway's state, and signs every input of those that stand.
    Sign,
    /// A federation signer's vote to cancel a release with an input under its threshold of
    /// signatures, with --evm-key; once enough voted, its inputs and withdrawals go back.
    Cancel { proposal: u64 },
    /// Assembles an open release's witnesses from the signatures posted, and broadcasts it.
    Finish { proposal: u64 },
    /// Shows a release confirmed on Bitcoin, spending its inputs and holding its change.
    Released { proposal: u64 },
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

fn whole(value: &Value) -> u64 {
    text(value).parse().unwrap_or(0)
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
                let hash = self.esplora(&format!("/block-height/{height}"))?;
                let header = from_hex(&format!("0x{}", self.esplora(&format!("/block/{}/header", hash.trim()))?.trim())).ok_or("Esplora gave a header that is not hex")?;
                self.call(*relay, "start", u256::ZERO, vec![bytes(header), number(*height), TxCallArgumentType::String(network.clone())])
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
            BtcAction::Address { recipient, network: name } => {
                let scripts = self.read(&self.btc_gateway()?, "deposit_script", json!([{ "type": "address", "value": recipient.to_string() }]))?;
                println!("{}", address(&raw(scripts.get(1).ok_or("the gateway gave no deposit script")?), network(name)?));

                Ok(())
            }
            BtcAction::Deposit { txid, vout, recipient } => {
                let Proof { tx, block, index, path, coinbase, coinbase_path } = self.proof(txid)?;
                let args = vec![bytes(block.to_vec()), bytes(tx), number(index), bytes(path), bytes(coinbase), bytes(coinbase_path), number(u64::from(*vout)), TxCallArgumentType::Address(*recipient), TxCallArgumentType::Nil];
                self.call(self.btc_gateway()?, "deposit", u256::ZERO, args)
            }
            BtcAction::SetMostFee { withdrawal, most_fee } => self.call(self.btc_gateway()?, "set_most_fee", u256::ZERO, vec![number(*withdrawal), number(*most_fee)]),
            BtcAction::Withdraw { address: text, sats, most_fee } => {
                let script = match text.strip_prefix("0x") {
                    Some(_) => from_hex(text).ok_or("the script is not hex")?,
                    None => {
                        let (_, program) = crate::structs::bech32::decode_segwit(text).ok_or("give a native segwit address, bc1q…, or an output script, 0x…")?;
                        [&[0x00, program.len() as u8][..], &program].concat()
                    }
                };
                self.call(self.btc_gateway()?, "withdraw", u256::ZERO, vec![bytes(script), number(*sats), most_fee.map_or(TxCallArgumentType::Nil, number)])
            }
            BtcAction::Propose { fee } => self.propose(*fee, false),
            BtcAction::Sweep { fee } => self.propose(*fee, true),
            BtcAction::Sign => self.sign_releases(),
            BtcAction::Cancel { proposal } => self.vote_cancel(*proposal),
            BtcAction::Finish { proposal } => self.finish(*proposal),
            BtcAction::Released { proposal } => {
                let found = self.read(&self.btc_gateway()?, "proposal", json!([{ "type": "u64", "value": proposal.to_string() }]))?;
                let txid = shown(&raw(found.get(1).ok_or("no such release")?));
                let Proof { tx, block, index, path, coinbase, coinbase_path } = self.proof(&txid)?;
                self.call(self.btc_gateway()?, "released", u256::ZERO, vec![number(*proposal), bytes(block.to_vec()), bytes(tx), number(index), bytes(path), bytes(coinbase), bytes(coinbase_path)])
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

    /// Proposes a release of the queued withdrawals, or a sweep of older federations' outputs. A
    /// withdrawal from a cancelled release brings the input it must spend; one whose input is in
    /// another release waits for that one. A sweep takes as much as the fee pays at the floor, for
    /// another sweep to take the rest.
    fn propose(&self, fee: u64, sweep: bool) -> CliResult<()> {
        let gateway = self.btc_gateway()?;
        let version = self.status()?["federation"]["version"].as_u64().ok_or("this chain has no federation")?;
        let change = p2wsh(&raw(self.read(&gateway, "change_script", json!([]))?.first().ok_or("no change script")?));
        let (_, floor) = self.fee_limits(&gateway)?;
        let held = self.unspent(&gateway)?;
        let mut anchors = std::collections::HashSet::new();
        // Each withdrawal it takes: its id, script, what it burned and the most it may lose.
        let mut ids: Vec<(u64, Vec<u8>, u64, u64)> = Vec::new();

        if !sweep {
            let count = whole(self.read(&gateway, "withdrawal_count", json!([]))?.first().ok_or("no count")?);

            for id in 1..=count {
                let withdrawal = self.read(&gateway, "withdrawal", json!([{ "type": "u64", "value": id.to_string() }]))?;

                if withdrawal.get(2).map(text).as_deref() == Some("queued") && ids.len() < MOST_WITHDRAWALS {
                    if let Some(anchor) = self.anchor(&gateway, id)? {
                        if !held.iter().any(|(outpoint, ..)| *outpoint == anchor) {
                            println!("withdrawal {id} waits: the input it must spend, {anchor}, is in another release");

                            continue;
                        }

                        anchors.insert(anchor);
                    }

                    ids.push((id, raw(&withdrawal[0]), whole(&withdrawal[1]), whole(&withdrawal[4])));
                }
            }

            if ids.is_empty() {
                return Err("no withdrawals are queued".into());
            }

            // One that agreed to lose less than its share waits, and the others each pay more for
            // it: until the shares stand.
            while let Some(at) = ids.iter().enumerate().position(|(at, (_, _, _, most))| btc_fee_share(fee, ids.len() as u64, at) > *most) {
                println!("withdrawal {} waits: its share of a fee of {fee} is past the {} it agreed to lose", ids[at].0, ids[at].3);
                ids.remove(at);

                if ids.is_empty() {
                    return Err(format!("every queued withdrawal agreed to lose less than its share of a fee of {fee}").into());
                }
            }
        }

        let mut outputs: Vec<TxOut> = ids.iter().enumerate().map(|(at, (_, script, sats, _))| TxOut { value: sats - btc_fee_share(fee, ids.len() as u64, at), script: script.clone() }).collect();
        // Withdrawals pay their own fee, so a release spends just what they burned; a sweep's fee
        // is the federation's.
        let burned = ids.iter().map(|(_, _, sats, _)| *sats).sum::<u64>();
        let wanted = if sweep { fee } else { burned + BTC_DUST };
        let mut inputs = Vec::new();
        let mut sizes = Vec::new();
        let mut taken = 0;
        // What must be spent comes first.
        let (needed, others): (Vec<_>, Vec<_>) = held.into_iter().partition(|(outpoint, ..)| anchors.contains(outpoint));

        for (outpoint, sats, at) in needed.into_iter().chain(others) {
            if (sweep && at >= version) || (!sweep && (taken >= wanted || taken == burned) && !anchors.contains(&outpoint)) {
                continue;
            }

            let spent = self.spent_output(&gateway, &outpoint)?;
            sizes.push((spent.threshold, spent.script_len));

            // Each input and its one output, and the fixed parts around them.
            if sweep && !inputs.is_empty() && btc_release_vbytes(10 + (inputs.len() + 1) * 41 + 43, &sizes) * floor > fee {
                sizes.pop();
                break;
            }

            let (txid, vout) = outpoint.split_once(':').ok_or("an outpoint that does not read")?;
            // The gateway keys what it holds by the id in Bitcoin's own byte order.
            let id: [u8; 32] = from_hex(&format!("0x{txid}")).and_then(|bytes| bytes.try_into().ok()).ok_or("an outpoint that does not read")?;
            inputs.push(TxIn { txid: id, vout: vout.parse()?, script_sig: Vec::new(), sequence: 0xffff_ffff, witness: Vec::new() });
            taken += sats;
        }

        // Spending just what was burned leaves no change; anything else leaves change of at least
        // dust, and it never goes to the fee, as that would come off the backing.
        if inputs.is_empty() || (taken < wanted && (sweep || taken != burned)) {
            return Err(format!("the gateway holds {taken} satoshis to spend, short of {wanted}").into());
        }

        if sweep {
            outputs.push(TxOut { value: taken - fee, script: change });
        } else if taken > burned {
            outputs.push(TxOut { value: taken - burned, script: change });
        }

        let tx = Transaction { version: 2, inputs, outputs, lock_time: 0 };
        let least = btc_release_vbytes(tx.base().len(), &sizes) * floor;

        if fee < least {
            return Err(format!("a fee of {fee} is under the floor: this release pays at least {least}").into());
        }

        let mut args = vec![TxCallArgumentType::Bytes(tx.base())];
        args.extend(ids.iter().map(|(id, ..)| TxCallArgumentType::U64(*id)));
        println!("proposing {} paying {} withdrawals, txid {}", if sweep { "a sweep" } else { "a release" }, ids.len(), shown(&sha256d(&tx.base())));

        self.call(gateway, "propose", u256::ZERO, args)
    }

    /// What a release is, from the gateway's state alone: what the signer goes over.
    fn release_facts(&self, gateway: &Address, found: &[Value]) -> CliResult<ReleaseFacts> {
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

        Ok(ReleaseFacts { raw: release, txid: raw(found.get(1).ok_or("no transaction id")?), version, withdrawals, inputs, change_script, fee_cap, fee_floor })
    }

    /// Goes over every open release against the gateway's state, and signs each input of those
    /// that stand and that this signer has not signed, nor voted to cancel. One refused is said and
    /// passed over, for the next one.
    fn sign_releases(&self) -> CliResult<()> {
        let (gateway, secret) = (self.btc_gateway()?, self.evm_secret()?);
        let count = whole(self.read(&gateway, "proposal_count", json!([]))?.first().ok_or("no count")?);
        let my_key = crate::crypto::secp256k1::public_key(&secret).ok_or("--evm-key is no key")?;
        // Two open releases naming one withdrawal would pay it twice, should the gateway ever let
        // them: the first keeps it.
        let mut paying = std::collections::HashSet::new();

        for id in 1..=count {
            let found = self.read(&gateway, "proposal", json!([{ "type": "u64", "value": id.to_string() }]))?;

            if found.get(4).map(text).as_deref() != Some("open") {
                continue;
            }

            if let Err(why) = self.sign_release(&gateway, id, &found, &my_key, &secret, &mut paying) {
                eprintln!("not signing release {id}: {why}");
            }
        }

        Ok(())
    }

    fn sign_release(&self, gateway: &Address, id: u64, found: &[Value], my_key: &[u8; 33], secret: &[u8; 32], paying: &mut std::collections::HashSet<u64>) -> CliResult<()> {
        if self.read(gateway, "voted", json!([{ "type": "u64", "value": id.to_string() }, { "type": "bytes", "value": to_hex(my_key) }]))?.first().is_some_and(|value| value["value"] == true) {
            return Err("this signer voted to cancel it".into());
        }

        let facts = self.release_facts(gateway, found)?;
        check_btc_release(&facts)?;

        for withdrawal in &facts.withdrawals {
            if !paying.insert(withdrawal.id) {
                return Err(format!("another open release pays withdrawal {}", withdrawal.id).into());
            }
        }

        let (tx, _) = Transaction::read(&facts.raw)?;

        for (index, (_, held)) in facts.inputs.iter().enumerate() {
            let posted = self.read(gateway, "input_signatures", json!([{ "type": "u64", "value": id.to_string() }, { "type": "u64", "value": index.to_string() }]))?;
            let script = raw(posted.first().ok_or("no witness script")?);
            let signature = crate::crypto::secp256k1::sign_hash(secret, &sha256d(&tx.script_preimage(index, &script, held.sats)));

            if posted.iter().skip(2).any(|value| raw(value) == signature) {
                continue;
            }

            println!("signing release {id}, input {index}");
            self.call(*gateway, "sign", u256::ZERO, vec![TxCallArgumentType::U64(id), TxCallArgumentType::U64(index as u64), TxCallArgumentType::Bytes(signature.to_vec())])?;
        }

        Ok(())
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
        println!("broadcast {}", answer.body_mut().read_to_string()?.trim());

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use {super::*, std::time::Duration};

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
}
