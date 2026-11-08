//! Bitcoin's transactions, as its network encodes them, and the pieces around them a wallet
//! needs: its networks, BIP143's hash of what a segwit input signs, scripts as addresses, DER
//! signatures, and the compact sizes and reading that every encoding is made of. Shared by the
//! wallet's PSBT signer and the swap tools, which build and sign Bitcoin's side of a swap.

use {
    super::sha256d,
    crate::structs::bech32,
    crate::structs::hex::to_hex,
    r#macro::document,
};

#[document(
    summary = "Signing all of the transaction, its inputs and outputs: the sighash type BIP143's hash here is for.",
    category = "Bitcoin",
)]
pub const SIGHASH_ALL: u32 = 1;

#[document(summary = "SIGHASH_NONE: signs no output.", category = "Bitcoin")]
pub const SIGHASH_NONE: u32 = 2;

#[document(summary = "SIGHASH_SINGLE: signs only the output at the input's own place.", category = "Bitcoin")]
pub const SIGHASH_SINGLE: u32 = 3;

#[document(summary = "SIGHASH_ANYONECANPAY, or'ed onto another type: signs only its own input, so others may add theirs, as a crowdfunded BitVM challenge needs.", category = "Bitcoin")]
pub const SIGHASH_ANYONECANPAY: u32 = 0x80;

#[document(
    summary = "Which Bitcoin: its own network, or its test networks, testnet, signet and regtest, whose coins are worth nothing. Their keys are others, so the one never spends the other's coins.",
    category = "Bitcoin",
)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Network {
    #[default]
    Main,
    Test,
}

impl Network {
    pub const ALL: [Network; 2] = [Network::Main, Network::Test];

    #[document(parent = "Network", summary = "Its coin type in the keys' path, as SLIP-44 has it.")]
    pub fn coin(self) -> u32 {
        match self {
            Network::Main => 0,
            // SLIP-44's for every test network, as mnemonic::BITCOIN_TESTNET_COIN_TYPE.
            Network::Test => 1,
        }
    }

    pub fn from_coin(coin: u32) -> Option<Network> {
        Network::ALL.into_iter().find(|network| network.coin() == coin)
    }

    #[document(parent = "Network", summary = "What the pairing and the review call it, large.")]
    pub fn name(self) -> &'static str {
        match self {
            Network::Main => "Bitcoin",
            Network::Test => "Bitcoin testnet",
        }
    }

    #[document(parent = "Network", summary = "What the switch between them calls it.")]
    pub fn short(self) -> &'static str {
        match self {
            Network::Main => "Mainnet",
            Network::Test => "Testnet",
        }
    }

    pub fn symbol(self) -> &'static str {
        match self {
            Network::Main => "BTC",
            Network::Test => "tBTC",
        }
    }

    #[document(parent = "Network", summary = "The account's path: m/84'/0'/0', or m/84'/1'/0'.")]
    pub fn path(self) -> String {
        format!("m/84'/{}'/0'", self.coin())
    }

    #[document(
        parent = "Network",
        summary = "Its segwit addresses' human-readable part: bc1…, or tb1… on testnet and signet.",
    )]
    pub fn hrp(self) -> &'static str {
        match self {
            Network::Main => "bc",
            Network::Test => "tb",
        }
    }

    #[document(
        parent = "Network",
        summary = "Its legacy addresses' version bytes: paying a key's hash, 1…, or m… and n…, and a script's, 3…, or 2….",
    )]
    pub fn versions(self) -> (u8, u8) {
        match self {
            Network::Main => (0x00, 0x05),
            Network::Test => (0x6f, 0xc4),
        }
    }
}


#[document(summary = "A Bitcoin transaction, as the network encodes it.", category = "Bitcoin")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transaction {
    pub version: u32,
    pub inputs: Vec<TxIn>,
    pub outputs: Vec<TxOut>,
    pub lock_time: u32,
}

#[document(
    summary = "An input: the output it spends, by its transaction's id, as hashed, and its index there.",
    category = "Bitcoin",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TxIn {
    pub txid: [u8; 32],
    pub vout: u32,
    pub script_sig: Vec<u8>,
    pub sequence: u32,
    #[document(summary = "Its witness, segwit's signature and what goes with it: none until signed.")]
    pub witness: Vec<Vec<u8>>,
}

#[document(
    summary = "An output: an amount, in satoshis, and the script that spends it.",
    category = "Bitcoin",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TxOut {
    pub value: u64,
    pub script: Vec<u8>,
}

impl Transaction {
    #[document(
        parent = "Transaction",
        summary = "Reads one from all of these bytes, with its witnesses, as BIP144 writes them, or without; says which.",
    )]
    pub fn read(bytes: &[u8]) -> Result<(Transaction, bool), String> {
        let mut r = Reader::new(bytes);
        let version = r.u32()?;
        // The marker and flag, 0 and 1, where a transaction with witnesses counts its inputs.
        let witnessed = matches!(bytes.get(4..6), Some([0, 1]));

        if witnessed {
            r.take(2)?;
        }

        let mut inputs = Vec::new();

        for _ in 0..r.compact()? {
            inputs.push(TxIn {
                txid: r.take(32)?.try_into().expect("32 bytes"),
                vout: r.u32()?,
                script_sig: r.sized()?.to_vec(),
                sequence: r.u32()?,
                witness: Vec::new(),
            });
        }

        let mut outputs = Vec::new();

        for _ in 0..r.compact()? {
            outputs.push(TxOut::read(&mut r)?);
        }

        if witnessed {
            for input in &mut inputs {
                for _ in 0..r.compact()? {
                    input.witness.push(r.sized()?.to_vec());
                }
            }
        }

        let lock_time = r.u32()?;

        match r.done() {
            true => Ok((Transaction { version, inputs, outputs, lock_time }, witnessed)),
            false => Err("a transaction with more after it than it holds".to_string()),
        }
    }

    #[document(
        parent = "Transaction",
        summary = "Its encoding without witnesses: what its id hashes, and a PSBT holds unsigned.",
    )]
    pub fn base(&self) -> Vec<u8> {
        let mut out = self.version.to_le_bytes().to_vec();
        compact(&mut out, self.inputs.len());

        for input in &self.inputs {
            out.extend(input.txid);
            out.extend(input.vout.to_le_bytes());
            compact(&mut out, input.script_sig.len());
            out.extend(&input.script_sig);
            out.extend(input.sequence.to_le_bytes());
        }

        compact(&mut out, self.outputs.len());

        for output in &self.outputs {
            output.write(&mut out);
        }

        out.extend(self.lock_time.to_le_bytes());

        out
    }

    #[document(
        parent = "Transaction",
        summary = "Its encoding as the network takes it: with its witnesses, as BIP144 writes them, when any input has one, and as base otherwise. What is broadcast.",
    )]
    pub fn encode(&self) -> Vec<u8> {
        if self.inputs.iter().all(|input| input.witness.is_empty()) {
            return self.base();
        }

        let base = self.base();
        // The marker and flag after the version; the witnesses before the lock time.
        let mut out = [&base[..4], &[0, 1]].concat();
        out.extend(&base[4..base.len() - 4]);

        for input in &self.inputs {
            compact(&mut out, input.witness.len());

            for item in &input.witness {
                compact(&mut out, item.len());
                out.extend(item);
            }
        }

        out.extend(self.lock_time.to_le_bytes());

        out
    }

    #[document(
        parent = "Transaction",
        summary = "Its size in virtual bytes, as fees are paid by: a base byte is four weight, a witness byte one.",
    )]
    pub fn vsize(&self) -> u64 {
        let (base, all) = (self.base().len() as u64, self.encode().len() as u64);

        (base * 3 + all).div_ceil(4)
    }

    #[document(
        parent = "Transaction",
        summary = "Its id, as an input names it: the double SHA-256 of it without witnesses, as hashed, the reverse of how explorers write it.",
    )]
    pub fn txid(&self) -> [u8; 32] {
        sha256d(&self.base())
    }

    #[document(
        parent = "Transaction",
        summary = "What BIP143 signs for an input that spends a native segwit output of a key's hash: all of the transaction, as SIGHASH_ALL has it, the input's place in it, and the amount it spends.",
    )]
    pub fn preimage(&self, index: usize, key_hash: &[u8; 20], value: u64) -> Vec<u8> {
        // The script code: the key hash's pay-to-key-hash script, as BIP143 has it for P2WPKH.
        let code = [&[0x76, 0xa9, 0x14][..], key_hash, &[0x88, 0xac]].concat();

        self.script_preimage(index, &code, value)
    }

    #[document(
        parent = "Transaction",
        summary = "The same for an input that spends a script's native segwit output, P2WSH: its script code is the witness script itself, which the hash covers whole.",
    )]
    pub fn script_preimage(&self, index: usize, code: &[u8], value: u64) -> Vec<u8> {
        self.sighash_preimage(index, code, value, SIGHASH_ALL)
    }

    #[document(
        parent = "Transaction",
        summary = "What BIP143 signs for a P2WSH input under any sighash type: SIGHASH_ALL, SIGHASH_NONE or SIGHASH_SINGLE, each with SIGHASH_ANYONECANPAY or not. ANYONECANPAY leaves the other inputs out, so others may add theirs; SINGLE covers only the output at the input's place, NONE no output, so others may add outputs too.",
        params = { index = "The input's place.", code = "Its witness script.", value = "What it spends.", sighash = "The sighash type, as it ends the signature." },
    )]
    pub fn sighash_preimage(&self, index: usize, code: &[u8], value: u64, sighash: u32) -> Vec<u8> {
        let anyone = sighash & SIGHASH_ANYONECANPAY != 0;
        let kind = sighash & 0x1f;
        let zero = [0u8; 32];
        let (mut prevouts, mut sequences, mut outputs) = (Vec::new(), Vec::new(), Vec::new());

        for input in &self.inputs {
            prevouts.extend(input.txid);
            prevouts.extend(input.vout.to_le_bytes());
            sequences.extend(input.sequence.to_le_bytes());
        }

        let hash_outputs = match kind {
            SIGHASH_SINGLE => match self.outputs.get(index) {
                Some(output) => {
                    output.write(&mut outputs);
                    sha256d(&outputs)
                }
                None => zero,
            },
            SIGHASH_NONE => zero,
            _ => {
                for output in &self.outputs {
                    output.write(&mut outputs);
                }
                sha256d(&outputs)
            }
        };
        let hash_prevouts = if anyone { zero } else { sha256d(&prevouts) };
        let hash_sequences = if anyone || kind == SIGHASH_SINGLE || kind == SIGHASH_NONE { zero } else { sha256d(&sequences) };

        let input = &self.inputs[index];
        let mut out = self.version.to_le_bytes().to_vec();
        out.extend(hash_prevouts);
        out.extend(hash_sequences);
        out.extend(input.txid);
        out.extend(input.vout.to_le_bytes());
        compact(&mut out, code.len());
        out.extend(code);
        out.extend(value.to_le_bytes());
        out.extend(input.sequence.to_le_bytes());
        out.extend(hash_outputs);
        out.extend(self.lock_time.to_le_bytes());
        out.extend(sighash.to_le_bytes());

        out
    }
}

impl TxOut {
    pub fn read(r: &mut Reader) -> Result<TxOut, String> {
        Ok(TxOut {
            value: r.u64()?,
            script: r.sized()?.to_vec(),
        })
    }

    pub fn write(&self, out: &mut Vec<u8>) {
        out.extend(self.value.to_le_bytes());
        compact(out, self.script.len());
        out.extend(&self.script);
    }
}


#[document(
    summary = "A script as its address, on a network: a key's or a script's hash in base58, 1… or 3…, a witness program in bech32, bc1q…, or bech32m, bc1p…. Data, and scripts with no address, are told as what they are.",
    category = "Bitcoin",
)]
pub fn address(script: &[u8], network: Network) -> String {
    let (key_hash, script_hash) = network.versions();

    match script {
        [0x76, 0xa9, 0x14, hash @ .., 0x88, 0xac] if hash.len() == 20 => base58check(key_hash, hash),
        [0xa9, 0x14, hash @ .., 0x87] if hash.len() == 20 => base58check(script_hash, hash),
        [0x00, size @ (0x14 | 0x20), program @ ..] if program.len() == usize::from(*size) => bech32::encode_segwit(network.hrp(), program),
        [version @ 0x51..=0x60, size @ 2..=40, program @ ..] if program.len() == usize::from(*size) => bech32::encode_witness(network.hrp(), version - 0x50, program),
        [0x6a, ..] => format!("data, burned: {}", to_hex(&script[1..])),
        _ => format!("a script with no address: {}", to_hex(script)),
    }
}


#[document(
    summary = "The native segwit script paying a key's hash: 0, and the 20 bytes.",
    category = "Bitcoin",
)]
pub fn p2wpkh(hash: &[u8; 20]) -> Vec<u8> {
    [&[0x00, 0x14][..], hash].concat()
}

#[document(
    summary = "The native segwit script paying a script's SHA-256, P2WSH: 0, and the 32 bytes, which bc1q… writes longer than a key's.",
    category = "Bitcoin",
)]
pub fn p2wsh(script: &[u8]) -> Vec<u8> {
    [&[0x00, 0x20][..], &crate::crypto::bitcoin::sha256(script)].concat()
}


#[document(
    summary = "A signature, r then s, as DER writes it, as Bitcoin's scripts take it: each a whole number in its fewest bytes, a 0 before one whose top bit is set.",
    category = "Bitcoin",
)]
pub fn der(signature: &[u8; 64]) -> Vec<u8> {
    let integer = |bytes: &[u8]| {
        let trimmed = &bytes[bytes.iter().position(|byte| *byte != 0).unwrap_or(bytes.len() - 1)..];

        match trimmed[0] & 0x80 {
            0 => trimmed.to_vec(),
            _ => [&[0][..], trimmed].concat(),
        }
    };
    let (r, s) = (integer(&signature[..32]), integer(&signature[32..]));
    let mut out = vec![0x30, (4 + r.len() + s.len()) as u8, 0x02, r.len() as u8];
    out.extend(r);
    out.extend([0x02, s.len() as u8]);
    out.extend(s);

    out
}

#[document(
    summary = "A DER signature read back into r then s, 32 bytes each: what der writes, and only that, each number in its fewest bytes. None for anything else.",
    category = "Bitcoin",
)]
pub fn from_der(der: &[u8]) -> Option<[u8; 64]> {
    let (&[0x30, length], rest) = der.split_first_chunk::<2>()? else {
        return None;
    };

    if usize::from(length) != rest.len() {
        return None;
    }

    let mut out = [0u8; 64];
    let mut rest = rest;

    for half in out.chunks_mut(32) {
        let (&[0x02, size], after) = rest.split_first_chunk::<2>()? else {
            return None;
        };
        let (number, after) = after.split_at_checked(usize::from(size))?;
        // A 0 only before a top bit set, and none past 32 bytes.
        let number = match number {
            [0, second, ..] if second & 0x80 != 0 => &number[1..],
            [first, ..] if first & 0x80 == 0 && (number.len() == 1 || *first != 0) => number,
            _ => return None,
        };

        if number.len() > 32 {
            return None;
        }

        half[32 - number.len()..].copy_from_slice(number);
        rest = after;
    }

    rest.is_empty().then_some(out)
}

#[document(
    summary = "Bytes behind a version byte, with a checksum, in base58: Bitcoin's legacy addresses.",
    category = "Bitcoin",
)]
pub fn base58check(version: u8, payload: &[u8]) -> String {
    const ALPHABET: &[u8; 58] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
    let mut bytes = vec![version];
    bytes.extend(payload);
    let check = sha256d(&bytes);
    bytes.extend(&check[..4]);

    // The digits, least first: each byte multiplies them by 256 and adds itself.
    let mut digits: Vec<u8> = Vec::new();

    for byte in &bytes {
        let mut carry = u32::from(*byte);

        for digit in &mut digits {
            carry += u32::from(*digit) << 8;
            *digit = (carry % 58) as u8;
            carry /= 58;
        }

        while carry > 0 {
            digits.push((carry % 58) as u8);
            carry /= 58;
        }
    }

    // Leading zero bytes are 1s, as they are no digits.
    let zeros = bytes.iter().take_while(|byte| **byte == 0).count();

    "1".repeat(zeros) + &digits.iter().rev().map(|digit| char::from(ALPHABET[usize::from(*digit)])).collect::<String>()
}

#[document(
    summary = "The output script a native segwit address pays: bc1q… or tb1q…, a key's P2WPKH or a script's P2WSH, on the network named.",
    category = "Bitcoin",
    returns = "Why it is not one of those on that network, otherwise.",
)]
pub fn script_of(text: &str, network: Network) -> Result<Vec<u8>, String> {
    let (hrp, program) = crate::structs::bech32::decode_segwit(text.trim()).ok_or("a native segwit address, bc1q… or tb1q…")?;

    if hrp != network.hrp() {
        return Err(format!("an address on {}, {}1q…", network.name(), network.hrp()));
    }

    match program.len() {
        20 | 32 => Ok([&[0x00, program.len() as u8][..], &program].concat()),
        _ => Err("a native segwit address of a key or a script".to_string()),
    }
}

#[document(summary = "Bitcoin's compact size, in its fewest bytes.", category = "Bitcoin")]
pub fn compact(out: &mut Vec<u8>, size: usize) {
    match size {
        0..0xfd => out.push(size as u8),
        0xfd..=0xffff => {
            out.push(0xfd);
            out.extend((size as u16).to_le_bytes());
        }
        0x1_0000..=0xffff_ffff => {
            out.push(0xfe);
            out.extend((size as u32).to_le_bytes());
        }
        _ => {
            out.push(0xff);
            out.extend((size as u64).to_le_bytes());
        }
    }
}

#[document(
    summary = "Reading bytes in order, refusing to read past their end.",
    category = "Bitcoin",
)]
pub struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Reader<'a> {
        Reader { bytes, at: 0 }
    }

    pub fn take(&mut self, count: usize) -> Result<&'a [u8], String> {
        let end = self.at.checked_add(count).filter(|end| *end <= self.bytes.len()).ok_or("cut short: it ends before what it holds")?;
        let taken = &self.bytes[self.at..end];
        self.at = end;

        Ok(taken)
    }

    pub fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().expect("4 bytes")))
    }

    pub fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().expect("8 bytes")))
    }

    #[document(
        parent = "Reader",
        summary = "A compact size, as it must be written: in its fewest bytes, as Bitcoin reads it.",
    )]
    pub fn compact(&mut self) -> Result<u64, String> {
        let (size, least) = match self.take(1)?[0] {
            0xfd => (u64::from(u16::from_le_bytes(self.take(2)?.try_into().expect("2 bytes"))), 0xfd),
            0xfe => (u64::from(u32::from_le_bytes(self.take(4)?.try_into().expect("4 bytes"))), 0x1_0000),
            0xff => (u64::from_le_bytes(self.take(8)?.try_into().expect("8 bytes")), 0x1_0000_0000),
            small => return Ok(u64::from(small)),
        };

        match size >= least {
            true => Ok(size),
            false => Err("a size not written in its fewest bytes".to_string()),
        }
    }

    #[document(parent = "Reader", summary = "A compact size, then that many bytes.")]
    pub fn sized(&mut self) -> Result<&'a [u8], String> {
        let size = self.compact()?;

        self.take(usize::try_from(size).map_err(|_| "a size past what this device holds")?)
    }

    pub fn done(&self) -> bool {
        self.at == self.bytes.len()
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn der_reads_back_what_it_writes_and_nothing_else() {
        let mut signature = [0u8; 64];
        signature[31] = 1;
        signature[32] = 0x80;
        signature[63] = 7;

        assert_eq!(from_der(&der(&signature)), Some(signature));
        let written = der(&signature);
        assert_eq!(from_der(&written[..written.len() - 1]), None, "cut short");
        assert_eq!(from_der(&[written.clone(), vec![0]].concat()), None, "with more after");
        // A needless leading zero.
        assert_eq!(from_der(&[0x30, 0x07, 0x02, 0x02, 0x00, 0x01, 0x02, 0x01, 0x07]), None);
    }

    #[test]
    fn a_transaction_with_witnesses_reads_and_writes_back_whole() {
        let mut tx = Transaction {
            version: 2,
            inputs: vec![TxIn { txid: [1; 32], vout: 3, script_sig: Vec::new(), sequence: 0xffff_fffd, witness: vec![vec![0xaa; 72], vec![0x02; 33]] }],
            outputs: vec![TxOut { value: 50_000, script: p2wpkh(&[9; 20]) }],
            lock_time: 0,
        };
        let encoded = tx.encode();
        let (read, witnessed) = Transaction::read(&encoded).unwrap();

        assert!(witnessed);
        assert_eq!(read, tx);
        assert_eq!(read.txid(), sha256d(&tx.base()), "the id leaves the witness out");
        assert!(tx.vsize() < encoded.len() as u64, "witness bytes weigh a quarter");

        tx.inputs[0].witness.clear();
        assert_eq!(tx.encode(), tx.base(), "without witnesses, the old encoding");
    }
}
