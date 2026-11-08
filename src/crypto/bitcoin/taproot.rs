//! Taproot, as BIP340, BIP341 and BIP342 have it: Schnorr signatures over x-only keys, script trees
//! of tapscript leaves, the output key tweaked to commit to its tree, control blocks that prove a
//! leaf is in it, and the signature hash both paths sign. What BitVM's phase 2 graph is built of,
//! its disprove scripts many leaves of one tree.

use {
    super::tx::{Transaction, TxOut, compact},
    k256::elliptic_curve::PrimeField,
    k256::elliptic_curve::sec1::ToSec1Point,
    k256::schnorr::{Signature, SigningKey, VerifyingKey},
    k256::{AffinePoint, ProjectivePoint, Scalar},
    r#macro::document,
    sha2::{Digest, Sha256},
};

#[document(summary = "The leaf version of a tapscript leaf, BIP342's.", category = "Bitcoin")]
pub const TAPSCRIPT: u8 = 0xc0;

#[document(
    summary = "BIP341's unspendable internal key H, the x of a point nobody knows the discrete log of: a tree's output key with it has no key path, so it is spent by its scripts alone.",
    category = "Bitcoin",
)]
pub const UNSPENDABLE: [u8; 32] = [
    0x50, 0x92, 0x9b, 0x74, 0xc1, 0xa0, 0x49, 0x54, 0xb7, 0x8b, 0x4b, 0x60, 0x35, 0xe9, 0x7a, 0x5e, 0x07, 0x8a, 0x5a, 0x0f, 0x28, 0xec, 0x96, 0xd5, 0x47, 0xbf, 0xee, 0x9a, 0xce, 0x80, 0x3a, 0xc0,
];

#[document(summary = "SIGHASH_DEFAULT: all of the transaction, as SIGHASH_ALL, with a 64-byte signature.", category = "Bitcoin")]
pub const SIGHASH_DEFAULT: u8 = 0x00;

#[document(summary = "A BIP340 tagged hash: SHA-256 of the tag's SHA-256 twice, then the message.", category = "Bitcoin")]
pub fn tagged_hash(tag: &str, message: &[u8]) -> [u8; 32] {
    let tag = Sha256::digest(tag.as_bytes());

    Sha256::new().chain_update(tag).chain_update(tag).chain_update(message).finalize().into()
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

#[document(summary = "A key's x-only form, BIP340's: its compressed form without the parity byte.", category = "Bitcoin")]
pub fn x_only(key: &[u8; 33]) -> [u8; 32] {
    key[1..].try_into().expect("32 bytes")
}

#[document(summary = "A tapscript leaf's hash: TapLeaf of its version and its script, the script's length first.", category = "Bitcoin")]
pub fn leaf_hash(script: &[u8]) -> [u8; 32] {
    let mut message = vec![TAPSCRIPT];
    compact(&mut message, script.len());
    message.extend_from_slice(script);

    tagged_hash("TapLeaf", &message)
}

#[document(summary = "A branch's hash: TapBranch of its two children, the lower first.", category = "Bitcoin")]
pub fn branch_hash(a: &[u8; 32], b: &[u8; 32]) -> [u8; 32] {
    let (low, high) = if a <= b { (a, b) } else { (b, a) };

    tagged_hash("TapBranch", &[low.as_slice(), high].concat())
}

#[document(
    summary = "A script tree of tapscript leaves, balanced, each leaf's path up to the root kept for its control block.",
    category = "Bitcoin",
    see_also = "TaprootOutput",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScriptTree {
    #[document(summary = "Its root's hash.")]
    pub root: [u8; 32],
    #[document(summary = "Each leaf's script, in the order given.")]
    pub leaves: Vec<Vec<u8>>,
    #[document(summary = "Each leaf's path: the hashes beside it, from the leaf up.")]
    pub paths: Vec<Vec<[u8; 32]>>,
}

impl ScriptTree {
    #[document(parent = "ScriptTree", summary = "The tree of these leaves, paired level by level, an odd one out carried up. None for no leaves.")]
    pub fn of(leaves: Vec<Vec<u8>>) -> Option<ScriptTree> {
        if leaves.is_empty() {
            return None;
        }

        let mut paths = vec![Vec::new(); leaves.len()];
        // Each node with the leaves under it.
        let mut level: Vec<([u8; 32], Vec<usize>)> = leaves.iter().enumerate().map(|(at, leaf)| (leaf_hash(leaf), vec![at])).collect();

        while level.len() > 1 {
            let mut up = Vec::with_capacity(level.len().div_ceil(2));

            for pair in level.chunks(2) {
                match pair {
                    [(a, under_a), (b, under_b)] => {
                        under_a.iter().for_each(|leaf| paths[*leaf].push(*b));
                        under_b.iter().for_each(|leaf| paths[*leaf].push(*a));
                        up.push((branch_hash(a, b), [under_a.clone(), under_b.clone()].concat()));
                    }
                    [one] => up.push(one.clone()),
                    _ => unreachable!("chunks of two"),
                }
            }

            level = up;
        }

        Some(ScriptTree { root: level[0].0, leaves, paths })
    }
}

#[document(
    summary = "A taproot output: its internal key, the tree it commits to if any, and the output key tweaked from both, with its parity.",
    category = "Bitcoin",
    see_also = "ScriptTree",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaprootOutput {
    #[document(summary = "The internal key, x-only.")]
    pub internal: [u8; 32],
    #[document(summary = "The script tree, or None for a key-path-only output.")]
    pub tree: Option<ScriptTree>,
    #[document(summary = "The output key, x-only: what the output's script names.")]
    pub output: [u8; 32],
    #[document(summary = "Whether the output key's y is odd, as a control block says.")]
    pub odd: bool,
}

/// The tweak BIP341 adds to the internal key: TapTweak of the key and the tree's root, if any.
fn tweak(internal: &[u8; 32], root: Option<&[u8; 32]>) -> Option<Scalar> {
    let message = match root {
        Some(root) => [internal.as_slice(), root].concat(),
        None => internal.to_vec(),
    };

    Option::from(Scalar::from_repr(tagged_hash("TapTweak", &message).into()))
}

impl TaprootOutput {
    #[document(
        parent = "TaprootOutput",
        summary = "The output of an internal key and a tree: the key lifted to its even point, plus TapTweak of it and the root times the generator.",
        returns = "None for an x that is on no point, or a tweak past the group's order, which BIP341 refuses.",
    )]
    pub fn new(internal: [u8; 32], tree: Option<ScriptTree>) -> Option<TaprootOutput> {
        let point: AffinePoint = *VerifyingKey::from_bytes(&internal.into()).ok()?.as_affine();
        let tweak = tweak(&internal, tree.as_ref().map(|tree| &tree.root))?;
        let output = (ProjectivePoint::from(point) + ProjectivePoint::GENERATOR * tweak).to_affine();
        let encoded = output.to_sec1_point(true);
        let bytes = encoded.as_bytes();

        Some(TaprootOutput { internal, tree, output: bytes[1..].try_into().ok()?, odd: bytes[0] == 0x03 })
    }

    #[document(parent = "TaprootOutput", summary = "Its script: OP_1, then the output key.")]
    pub fn script(&self) -> Vec<u8> {
        [&[0x51, 0x20][..], &self.output].concat()
    }

    #[document(parent = "TaprootOutput", summary = "The control block a leaf is spent by: its version and the output key's parity, the internal key, and the leaf's path.")]
    pub fn control_block(&self, leaf: usize) -> Option<Vec<u8>> {
        let tree = self.tree.as_ref()?;
        let mut block = vec![TAPSCRIPT | u8::from(self.odd)];
        block.extend_from_slice(&self.internal);
        tree.paths.get(leaf)?.iter().for_each(|node| block.extend_from_slice(node));

        Some(block)
    }

    #[document(parent = "TaprootOutput", summary = "The witness that spends it by a leaf: the stack the leaf's script takes, then the script, then its control block.")]
    pub fn script_witness(&self, leaf: usize, stack: Vec<Vec<u8>>) -> Option<Vec<Vec<u8>>> {
        let script = self.tree.as_ref()?.leaves.get(leaf)?.clone();
        let mut witness = stack;
        witness.push(script);
        witness.push(self.control_block(leaf)?);

        Some(witness)
    }
}

#[document(
    summary = "A secret key tweaked to sign for a taproot output by its key path: negated first if its point's y is odd, as BIP340 keys are even, then the tweak added.",
    category = "Bitcoin",
    returns = "None for a secret that is no key.",
)]
pub fn tweaked_secret(secret: &[u8; 32], root: Option<&[u8; 32]>) -> Option<[u8; 32]> {
    let key = SigningKey::from_bytes(&(*secret).into()).ok()?;
    let internal: [u8; 32] = key.verifying_key().to_bytes().into();
    // SigningKey keeps the secret negated to its even point already.
    let even: Scalar = *key.as_nonzero_scalar().as_ref();
    let tweaked = even + tweak(&internal, root)?;

    Some(tweaked.to_repr().into())
}

#[document(summary = "A key's x-only form from its secret, as BIP340 has it: of its even point.", category = "Bitcoin")]
pub fn x_only_of(secret: &[u8; 32]) -> Option<[u8; 32]> {
    Some(SigningKey::from_bytes(&(*secret).into()).ok()?.verifying_key().to_bytes().into())
}

#[document(summary = "A BIP340 Schnorr signature of a 32-byte message, with aux_rand all zeros: deterministic, as presigned graphs want.", category = "Bitcoin")]
pub fn schnorr_sign(secret: &[u8; 32], message: &[u8; 32]) -> Option<[u8; 64]> {
    let key = SigningKey::from_bytes(&(*secret).into()).ok()?;

    Some(key.sign_raw(message, &[0; 32]).ok()?.to_bytes())
}

#[document(summary = "Whether a BIP340 signature holds for an x-only key and a 32-byte message.", category = "Bitcoin")]
pub fn schnorr_verify(key: &[u8; 32], message: &[u8; 32], signature: &[u8; 64]) -> bool {
    let (Ok(key), Ok(signature)) = (VerifyingKey::from_bytes(&(*key).into()), Signature::try_from(signature.as_slice())) else {
        return false;
    };

    key.verify_raw(message, &signature).is_ok()
}

#[document(
    summary = "What BIP341 signs for an input, by its key path or, with a leaf, by a tapscript leaf (BIP342): the transaction's version and lock time, the spent outputs' outpoints, amounts, scripts and sequences unless ANYONECANPAY, the outputs as the type says, this input's place, and for a leaf its hash.",
    category = "Bitcoin",
    params = {
        tx = "The transaction.",
        index = "The input's place.",
        spent = "Every input's spent output, in order: BIP341 signs all their amounts and scripts.",
        sighash = "The type: SIGHASH_DEFAULT, or a legacy type, ALL, NONE or SINGLE, with ANYONECANPAY or not.",
        leaf = "For a script path, the leaf's hash; None for the key path.",
    },
)]
pub fn sighash(tx: &Transaction, index: usize, spent: &[TxOut], sighash: u8, leaf: Option<&[u8; 32]>) -> [u8; 32] {
    let anyone = sighash & 0x80 != 0;
    let kind = sighash & 0x03;
    let mut message = vec![0x00, sighash];
    message.extend(tx.version.to_le_bytes());
    message.extend(tx.lock_time.to_le_bytes());

    if !anyone {
        let (mut prevouts, mut amounts, mut scripts, mut sequences) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for (input, output) in tx.inputs.iter().zip(spent) {
            prevouts.extend(input.txid);
            prevouts.extend(input.vout.to_le_bytes());
            amounts.extend(output.value.to_le_bytes());
            compact(&mut scripts, output.script.len());
            scripts.extend(&output.script);
            sequences.extend(input.sequence.to_le_bytes());
        }
        message.extend(sha256(&prevouts));
        message.extend(sha256(&amounts));
        message.extend(sha256(&scripts));
        message.extend(sha256(&sequences));
    }

    // ALL, and DEFAULT, sign every output.
    if kind != 2 && kind != 3 {
        let mut outputs = Vec::new();
        tx.outputs.iter().for_each(|output| output.write(&mut outputs));
        message.extend(sha256(&outputs));
    }

    let ext = u8::from(leaf.is_some());
    message.push(ext << 1);

    let input = &tx.inputs[index];
    if anyone {
        message.extend(input.txid);
        message.extend(input.vout.to_le_bytes());
        message.extend(spent[index].value.to_le_bytes());
        compact(&mut message, spent[index].script.len());
        message.extend(&spent[index].script);
        message.extend(input.sequence.to_le_bytes());
    } else {
        message.extend((index as u32).to_le_bytes());
    }

    if kind == 3 {
        let mut output = Vec::new();
        if let Some(own) = tx.outputs.get(index) {
            own.write(&mut output);
        }
        message.extend(sha256(&output));
    }

    if let Some(leaf) = leaf {
        message.extend(leaf);
        message.push(0x00);
        message.extend(u32::MAX.to_le_bytes());
    }

    tagged_hash("TapSighash", &message)
}

#[document(summary = "A Schnorr signature as a witness takes it: 64 bytes for SIGHASH_DEFAULT, else the type after them.", category = "Bitcoin")]
pub fn witness_signature(signature: &[u8; 64], sighash: u8) -> Vec<u8> {
    match sighash {
        SIGHASH_DEFAULT => signature.to_vec(),
        other => [signature.as_slice(), &[other]].concat(),
    }
}

#[cfg(test)]
mod tests;
