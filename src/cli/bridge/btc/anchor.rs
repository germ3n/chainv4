//! The anchorer (docs/anchoring.md): writes the chain's newest finalized checkpoint into a Bitcoin
//! transaction's OP_RETURN, from a key of its own, every anchoring.every epochs; once the relay
//! holds the block as deep as anchoring.depth, shows it to the chain with Tx::Anchor, which pays
//! the reward. With anchoring.ethereum on and --eth-rpc and --evm-key given, it writes the same 80
//! bytes to Ethereum too, as a transaction to itself, which the chain does not count yet.

use {
    super::{Followed, internal},
    crate::anchor::Payload,
    crate::cli::{CliResult, bridge::Bridge, send::TxArgs},
    crate::crypto::bitcoin::{hash160, sign, tx::{Transaction, TxIn, TxOut, der, p2wpkh}},
    crate::structs::hex::{from_hex, to_hex},
    crate::structs::tx::{AnchorProof, BtcAnchorProof},
    serde_json::{Value, json},
};

/// What an anchor's transaction weighs, in virtual bytes: its frame, each P2WPKH input, the
/// OP_RETURN output of 80 bytes, and the change back to the key.
const FRAME_VBYTES: u64 = 11;
const INPUT_VBYTES: u64 = 68;
const PAYLOAD_VBYTES: u64 = 8 + 1 + 83;
const CHANGE_VBYTES: u64 = 31;
/// Less change than this goes to the fee: a P2WPKH output's dust.
const CHANGE_DUST: u64 = 294;
/// The rate paid when Esplora has none to say, in satoshis a virtual byte.
const FALLBACK_RATE: u64 = 2;

/// An anchor written to Bitcoin and not yet shown to the chain.
pub(in crate::cli::bridge) struct Pending {
    txid: String,
    epoch: u64,
    root: String,
}

/// The list's newest entry's epoch in a GET /anchors answer.
fn newest_epoch(anchors: &Value) -> Option<u64> {
    anchors["entries"].as_array()?.last()?["epoch"].as_u64()
}

impl Bridge {
    fn anchors(&self) -> CliResult<Value> {
        let answer = self.agent.get(format!("{}/anchors", self.node)).call().map_err(|err| format!("GET /anchors: {err}"))?.body_mut().read_to_string()?;

        Ok(serde_json::from_str(&answer).map_err(|_| "GET /anchors answered something that is not JSON")?)
    }

    /// One pass of the anchorer: shows the pending anchor once it is deep enough, or writes the
    /// next one once it is due.
    pub(in crate::cli::bridge) fn anchor_pass(&self, secret: &[u8; 32], pending: &mut Option<Pending>, rate: Option<u64>) -> CliResult<()> {
        let anchors = self.anchors()?;
        let settings = &anchors["settings"];

        if settings["bitcoin"].as_bool() != Some(true) {
            println!("anchoring to Bitcoin is off on this chain: nothing to do");

            return Ok(());
        }

        if let Some(written) = pending.take() {
            let recorded = anchors["entries"].as_array().into_iter().flatten().any(|entry| entry["epoch"].as_u64() == Some(written.epoch));

            if recorded {
                println!("epoch {}'s anchor is recorded", written.epoch);

                return Ok(());
            }

            if anchors["root"].as_str() != Some(written.root.as_str()) {
                println!("another anchor was recorded since {} was written: writing a new one", written.txid);
            } else {
                match self.show_anchor(&written, &anchors)? {
                    true => return Ok(()),
                    false => {
                        *pending = Some(written);

                        return Ok(());
                    }
                }
            }
        }

        let finality = self.status()?["finality"]["finalized"].clone();
        let (Some(epoch), Some(block)) = (finality["epoch"].as_u64(), finality["block"].as_str()) else {
            println!("the node shows no finalized checkpoint yet");

            return Ok(());
        };
        let every = settings["every"].as_u64().unwrap_or(1).max(1);

        if epoch == 0 || newest_epoch(&anchors).is_some_and(|newest| epoch < newest.saturating_add(every)) {
            return Ok(());
        }

        let root = anchors["root"].as_str().ok_or("GET /anchors gave no root")?.to_string();
        let payload = Payload {
            chain: from_hex(anchors["chain_tag"].as_str().unwrap_or_default()).and_then(|tag| tag.try_into().ok()).ok_or("GET /anchors gave no chain tag")?,
            epoch,
            block: from_hex(block).and_then(|bytes| bytes.try_into().ok()).ok_or("the node's finalized block is not 32 bytes")?,
            root: from_hex(&root).and_then(|bytes| bytes.try_into().ok()).ok_or("GET /anchors gave a root that is not 32 bytes")?,
        };
        let network = settings["btc_start"]["network"].as_str().ok_or("anchoring names no Bitcoin network")?;
        let txid = self.write_anchor(secret, &payload, Followed::named(network)?, rate)?;
        println!("wrote epoch {epoch}'s checkpoint {block} into Bitcoin: {txid}");

        #[cfg(feature = "anchoring-eth")]
        if settings["ethereum"].as_bool() == Some(true)
            && !self.args.eth_rpc.is_empty()
            && self.args.evm_key.is_some()
            && let Err(why) = self.write_ethereum_anchor(&payload)
        {
            eprintln!("writing the anchor to Ethereum: {why}");
        }

        *pending = Some(Pending { txid, epoch, root });

        Ok(())
    }

    /// Shows a pending anchor to the chain, once its block is deep enough in the relay: whether it
    /// was shown.
    fn show_anchor(&self, written: &Pending, anchors: &Value) -> CliResult<bool> {
        let status: Value = serde_json::from_str(&self.esplora(&format!("/tx/{}/status", written.txid))?).map_err(|_| "Esplora gave a status that is not JSON")?;

        if status["confirmed"].as_bool() != Some(true) {
            return Ok(false);
        }

        let block = internal(status["block_hash"].as_str().ok_or("Esplora gave no block hash")?)?;
        let relay = anchors["settings"]["relay"].as_str().ok_or("anchoring names no relay")?.parse().map_err(|err| format!("anchoring's relay: {err}"))?;
        let depth = self.read(&relay, "confirmations", json!([{ "type": "bytes", "value": to_hex(&block) }]))?;
        let depth = depth.first().and_then(|value| value["value"].as_str().and_then(|text| text.parse::<u64>().ok()).or_else(|| value["value"].as_u64())).unwrap_or(0);
        let needed = anchors["settings"]["depth"].as_u64().unwrap_or(6);

        if depth < needed {
            println!("{} is {depth} deep in the relay, of the {needed} anchoring asks", written.txid);

            return Ok(false);
        }

        let proof = self.proof(&written.txid)?;
        let to_hashes = |path: &[u8]| path.chunks(32).map(|chunk| chunk.try_into().expect("32 bytes")).collect::<Vec<[u8; 32]>>();
        let proof = AnchorProof::Bitcoin(BtcAnchorProof { block: proof.block, raw: proof.tx, index: proof.index, path: to_hashes(&proof.path), coinbase: proof.coinbase, coinbase_path: to_hashes(&proof.coinbase_path) });
        let encoded = wincode::serialize(&proof).map_err(|err| format!("encoding the proof: {err}"))?;
        self.tx(TxArgs::Anchor { proof: to_hex(&encoded) })?;

        Ok(true)
    }

    /// Writes an anchor into Bitcoin from the key's native segwit coins, the change back to it.
    fn write_anchor(&self, secret: &[u8; 32], payload: &Payload, network: Followed, rate: Option<u64>) -> CliResult<String> {
        let public = crate::crypto::secp256k1::public_key(secret).ok_or("the Bitcoin key is no key")?;
        let hash = hash160(&public);
        let script = p2wpkh(&hash);
        let address = network.address(&script);
        let coins: Value = serde_json::from_str(&self.esplora(&format!("/address/{address}/utxo"))?).map_err(|_| "Esplora gave coins that are not JSON")?;
        let mut coins: Vec<(String, u32, u64)> = coins
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|coin| Some((coin["txid"].as_str()?.to_string(), u32::try_from(coin["vout"].as_u64()?).ok()?, coin["value"].as_u64()?)))
            .collect();
        coins.sort_by_key(|(_, _, value)| std::cmp::Reverse(*value));
        let rate = match rate {
            Some(rate) => rate,
            None => self.fee_rates().ok().flatten().map_or(FALLBACK_RATE, |rates| rates.soon.ceil().max(1.0) as u64),
        };
        let (mut chosen, mut total) = (Vec::new(), 0u64);

        for coin in coins {
            total += coin.2;
            chosen.push(coin);

            if total >= rate * (FRAME_VBYTES + INPUT_VBYTES * chosen.len() as u64 + PAYLOAD_VBYTES + CHANGE_VBYTES) + CHANGE_DUST {
                break;
            }
        }

        let fee = rate * (FRAME_VBYTES + INPUT_VBYTES * chosen.len() as u64 + PAYLOAD_VBYTES + CHANGE_VBYTES);

        if total < fee {
            return Err(format!("{address} holds {total} satoshis, short of the {fee} an anchor costs at {rate} a virtual byte: send it some").into());
        }

        let mut outputs = vec![TxOut { value: 0, script: payload.script() }];

        if total - fee >= CHANGE_DUST {
            outputs.push(TxOut { value: total - fee, script });
        }

        let inputs = chosen.iter().map(|(txid, vout, _)| Ok(TxIn { txid: internal(txid)?, vout: *vout, script_sig: Vec::new(), sequence: 0xffff_fffd, witness: Vec::new() })).collect::<CliResult<Vec<_>>>()?;
        let mut tx = Transaction { version: 2, inputs, outputs, lock_time: 0 };

        for (index, (_, _, value)) in chosen.iter().enumerate() {
            let signature = sign(secret, &tx.preimage(index, &hash, *value));
            tx.inputs[index].witness = vec![[der(&signature), vec![0x01]].concat(), public.to_vec()];
        }

        let base = self.args.esplora.as_deref().ok_or("give --esplora")?;
        let mut answer = self.agent.post(format!("{}/tx", base.trim_end_matches('/'))).send(to_hex(&tx.encode()).trim_start_matches("0x").to_string()).map_err(|err| format!("Esplora would not take the anchor: {err}"))?;
        let accepted = answer.status().is_success();
        let said = answer.body_mut().read_to_string()?;

        match accepted {
            true => Ok(said.trim().to_string()),
            false => Err(format!("Esplora would not take the anchor: {}", said.trim()).into()),
        }
    }

    #[cfg(feature = "anchoring-eth")]
    /// Writes an anchor's 80 bytes to the EVM chain of the one --eth-rpc, as a transaction to the
    /// --evm-key's own address: there for anyone to read, counted by nothing yet.
    fn write_ethereum_anchor(&self, payload: &Payload) -> CliResult<()> {
        let secret = self.evm_secret()?;
        let me = crate::cli::swap::rpc::from_key(&secret)?;
        let rpc = crate::cli::swap::rpc::Rpc::new(self.agent.clone(), self.args.eth_rpc.first().ok_or("give --eth-rpc")?);
        let call = crate::swap::evm::Call { to: me, value: crate::structs::u256::u256::ZERO, input: payload.encode().to_vec(), what: "Write the anchor to Ethereum" };
        rpc.send(&call, false, &secret)?;

        Ok(())
    }
}
