//! The anchorer's Bitcoin leg (docs/anchoring.md): writes a checkpoint into a Bitcoin transaction's
//! OP_RETURN, paid from a key's native segwit coins, and once the relay holds its block as deep as
//! anchoring.depth, shows it to the chain with Tx::Anchor, which pays the reward. The anchorer
//! itself, deciding when, is bridge anchor (super::super::anchor).

use {
    super::{Followed, internal},
    crate::anchor::Payload,
    crate::cli::{CliResult, bridge::Bridge, send::TxArgs},
    crate::crypto::bitcoin::{hash160, sign, tx::{Transaction, TxIn, TxOut, der, p2wpkh}},
    crate::structs::hex::to_hex,
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
/// The rate paid when Bitcoin's source has none to say, in satoshis a virtual byte.
const FALLBACK_RATE: u64 = 2;

/// An anchor written to Bitcoin and not yet shown to the chain.
pub(in crate::cli::bridge) struct Pending {
    pub(in crate::cli::bridge) txid: String,
    pub(in crate::cli::bridge) epoch: u64,
    pub(in crate::cli::bridge) root: String,
}

impl Bridge {
    /// Writes an anchor into Bitcoin, for the network anchoring's start names.
    pub(in crate::cli::bridge) fn write_btc_anchor(&self, secret: &[u8; 32], payload: &Payload, anchors: &Value, rate: Option<u64>) -> CliResult<String> {
        let network = anchors["settings"]["btc_start"]["network"].as_str().ok_or("anchoring names no Bitcoin network")?;

        self.write_anchor(secret, payload, Followed::named(network)?, rate)
    }

    /// Shows a pending anchor to the chain, once its block is deep enough in the relay: whether it
    /// was shown.
    pub(in crate::cli::bridge) fn show_btc_anchor(&self, written: &Pending, anchors: &Value) -> CliResult<bool> {
        let status = self.btc_api()?.tx_status(&written.txid)?;

        if !status.confirmed {
            return Ok(false);
        }

        let block = internal(status.block_hash.as_deref().ok_or("Bitcoin's source gave no block hash")?)?;
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
        let bitcoin = self.btc_api()?;
        let public = crate::crypto::secp256k1::public_key(secret).ok_or("the Bitcoin key is no key")?;
        let hash = hash160(&public);
        let script = p2wpkh(&hash);
        let address = network.address(&script);
        let mut coins: Vec<(String, u32, u64)> = bitcoin.coins(&address)?.into_iter().map(|coin| (coin.txid, coin.vout, coin.value)).collect();
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

        Ok(bitcoin.broadcast(&to_hex(&tx.encode())).map_err(|why| format!("Bitcoin would not take the anchor: {why}"))?)
    }
}
