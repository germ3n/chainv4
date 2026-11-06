//! Paying from an account's own Bitcoin coins, native segwit keys' alone: the coins picked largest
//! first, the fee at a rate per virtual byte, the change to a key of the account's, and each input
//! signed as BIP143 has it. Replaceable (BIP125), so a stuck one can be bumped.

use {
    chainv4::crypto::bitcoin::{
        hash160, sign,
        tx::{Transaction, TxIn, TxOut, der},
    },
    chainv4::crypto::keypair::{Keypair, PrivateKey, PublicKey},
};

/// What a transaction is before its inputs and outputs, and its witness marker: version, counts
/// and lock time, rounded up.
const FRAME_VBYTES: u64 = 11;
/// A native segwit key's input: its outpoint, sequence and empty script, and a signature and key in
/// the witness, at a quarter each, rounded up.
const INPUT_VBYTES: u64 = 68;
/// Below this an output is dust, which Bitcoin Core's peers refuse to pass on: a key's output.
pub const KEY_DUST: u64 = 294;
/// The same for a script's, P2WSH.
pub const SCRIPT_DUST: u64 = 330;
/// Sequence that signals the transaction may be replaced, and has no relative lock.
const REPLACEABLE: u32 = 0xffff_fffd;

/// A coin the account holds, and the key that spends it.
#[derive(Clone)]
pub struct Spendable {
    /// Its transaction's id, as explorers write it, and its index there.
    pub txid: String,
    pub vout: u32,
    pub value: u64,
    pub key: Keypair,
}

/// How much to send: an amount, or everything the coins hold less the fee.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Amount {
    Exactly(u64),
    All,
}

/// A transaction built and signed, and what it does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Built {
    pub raw: Vec<u8>,
    pub txid: String,
    /// What the payee gets, the fee, and the change, if there is any to keep.
    pub sent: u64,
    pub fee: u64,
    pub change: Option<u64>,
    /// The coins it spends, by txid and index.
    pub spends: Vec<(String, u32)>,
    pub vsize: u64,
}

fn output_vbytes(script: &[u8]) -> u64 {
    // Its amount, its script's length and the script.
    8 + 1 + script.len() as u64
}

/// The fee for a transaction of so many inputs and these outputs, at a rate.
fn fee(rate: u64, inputs: usize, outputs: &[&[u8]]) -> u64 {
    rate * (FRAME_VBYTES + INPUT_VBYTES * inputs as u64 + outputs.iter().map(|script| output_vbytes(script)).sum::<u64>())
}

/// Builds and signs a payment to a script from these coins, at a rate in satoshis a virtual byte,
/// the change to the change script when there is more than dust to keep: below it, the fee keeps
/// it. The coins go in largest first, as few as cover it.
pub fn pay(mut coins: Vec<Spendable>, to: &[u8], amount: Amount, rate: u64, change: &[u8]) -> Result<Built, String> {
    if rate == 0 {
        return Err("a fee rate of 1 or more satoshis a virtual byte".to_string());
    }

    let dust = if to.len() == 34 { SCRIPT_DUST } else { KEY_DUST };
    coins.sort_by_key(|coin| std::cmp::Reverse(coin.value));
    let held: u64 = coins.iter().map(|coin| coin.value).sum();

    if coins.is_empty() {
        return Err("no confirmed coins to spend".to_string());
    }

    let (chosen, sent, change_value) = match amount {
        Amount::All => {
            let fee = fee(rate, coins.len(), &[to]);
            let sent = held.checked_sub(fee).filter(|sent| *sent >= dust).ok_or_else(|| format!("the coins hold {held} satoshis, not enough over the {fee} fee to send"))?;

            (coins, sent, None)
        }
        Amount::Exactly(sent) => {
            if sent < dust {
                return Err(format!("{sent} satoshis is dust, below the {dust} Bitcoin's peers pass on"));
            }

            let (mut chosen, mut total) = (Vec::new(), 0u64);
            let mut found = None;

            for coin in coins {
                total += coin.value;
                chosen.push(coin);

                // With change, or without when what is left is dust the fee may as well keep.
                let with_change = fee(rate, chosen.len(), &[to, change]);
                let without = fee(rate, chosen.len(), &[to]);

                if total >= sent + with_change && total - sent - with_change >= KEY_DUST {
                    found = Some(Some(total - sent - with_change));
                    break;
                }

                if total >= sent + without {
                    found = Some(None);
                    break;
                }
            }

            let change_value = found.ok_or_else(|| format!("the coins hold {held} satoshis, short of {sent} and its fee at {rate} a virtual byte"))?;

            (chosen, sent, change_value)
        }
    };

    let mut outputs = vec![TxOut { value: sent, script: to.to_vec() }];

    if let Some(value) = change_value {
        outputs.push(TxOut { value, script: change.to_vec() });
    }

    let inputs = chosen
        .iter()
        .map(|coin| Ok(TxIn { txid: internal(&coin.txid)?, vout: coin.vout, script_sig: Vec::new(), sequence: REPLACEABLE, witness: Vec::new() }))
        .collect::<Result<Vec<_>, String>>()?;
    let mut tx = Transaction { version: 2, inputs, outputs, lock_time: 0 };

    for (index, coin) in chosen.iter().enumerate() {
        let (PrivateKey::Bitcoin(secret), PublicKey::Bitcoin(public)) = (coin.key.private(), coin.key.public()) else {
            return Err("a coin whose key is not a Bitcoin key".to_string());
        };
        let signature = sign(secret, &tx.preimage(index, &hash160(&public), coin.value));
        tx.inputs[index].witness = vec![[der(&signature), vec![0x01]].concat(), public.to_vec()];
    }

    let spent: u64 = chosen.iter().map(|coin| coin.value).sum();
    let mut txid = tx.txid();
    txid.reverse();

    Ok(Built {
        raw: tx.encode(),
        txid: txid.iter().map(|byte| format!("{byte:02x}")).collect(),
        sent,
        fee: spent - sent - change_value.unwrap_or(0),
        change: change_value,
        spends: chosen.iter().map(|coin| (coin.txid.clone(), coin.vout)).collect(),
        vsize: tx.vsize(),
    })
}

/// A transaction id as explorers write it, in the byte order it is hashed in.
fn internal(txid: &str) -> Result<[u8; 32], String> {
    let mut bytes: [u8; 32] = chainv4::structs::hex::from_hex(txid).and_then(|bytes| bytes.try_into().ok()).ok_or_else(|| format!("{txid} is not a transaction id"))?;
    bytes.reverse();

    Ok(bytes)
}

#[cfg(test)]
mod tests;
