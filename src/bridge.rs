//! Bridges for the apps and the federation's tools (docs/bridges.md): a transfer as both
//! gateways, contracts/lua/gateway.lua here and contracts/evm/Gateway.sol on EVM chains, hash it,
//! and as the federation's signers sign it.

use {
    crate::abi::{AbiType, AbiValue, encode_values},
    crate::crypto::hasher::Hasher,
    crate::structs::address::Address,
    crate::structs::hash::HashType,
    crate::structs::tx::TxCallArgumentType,
    crate::structs::u256::u256,
    r#macro::document,
};

#[cfg(feature = "bitcoin")]
#[document(
    summary = "A Bitcoin multisig script, as the Bitcoin gateway writes it: the threshold, each key, their count and OP_CHECKMULTISIG. Bare, it is a federation's change; under a recipient, its deposit script.",
    category = "Bridges",
    see_also = "btc_deposit_script",
)]
pub fn btc_multisig(threshold: u32, keys: &[[u8; 33]]) -> Vec<u8> {
    // Numbers to 16 as their own opcodes, past it as a one-byte push, as the gateway does.
    let small = |number: usize| match number {
        0..=16 => vec![0x50 + number as u8],
        _ => vec![1, number as u8],
    };
    let mut script = small(threshold as usize);

    for key in keys {
        script.push(33);
        script.extend_from_slice(key);
    }

    script.extend(small(keys.len()));
    script.push(0xae);

    script
}

#[cfg(feature = "bitcoin")]
#[document(
    summary = "An address's deposit script under a federation, as the Bitcoin gateway writes it: its 33 bytes, dropped; the chain's id and the gateway, dropped; then the multisig. Paid as its P2WSH, the gateway mints wBTC to the address for it, on this chain alone, as a proof of a payment outlives a chain; computed here, so a wallet never takes the address to pay from a node.",
    params = {
        recipient = "Who the deposit is for.",
        chain_id = "The chain's id, crypto::ethereum::chain_id of its genesis.",
        gateway = "The Bitcoin gateway.",
        threshold = "How many of the federation must sign.",
        keys = "The federation's keys.",
    },
    category = "Bridges",
    see_also = "btc_multisig",
)]
pub fn btc_deposit_script(recipient: &Address, chain_id: u64, gateway: &Address, threshold: u32, keys: &[[u8; 33]]) -> Vec<u8> {
    [&[33][..], &recipient.to_abi(), &[0x75, 41], &chain_id.to_le_bytes(), &gateway.to_abi(), &[0x75], &btc_multisig(threshold, keys)].concat()
}

#[cfg(feature = "bitcoin")]
#[document(
    summary = "What a federation signer signs to vote to cancel a release on the Bitcoin gateway: double SHA-256 of a domain, the chain's id, the gateway, the release and its transaction's id, as the gateway makes it.",
    category = "Bridges",
    see_also = "sign_digest",
)]
pub fn btc_cancel_digest(chain_id: u64, gateway: &Address, proposal: u64, txid: &[u8; 32]) -> [u8; 32] {
    crate::crypto::bitcoin::sha256d(&[b"chainv4-btc-cancel".as_slice(), &chain_id.to_le_bytes(), &gateway.to_abi(), &proposal.to_le_bytes(), txid].concat())
}

#[cfg(feature = "bitcoin")]
#[document(
    summary = "The most virtual bytes a release can be once signed, as the Bitcoin gateway reckons it to hold its fee to the floor: its bytes without witnesses, and for each input, a threshold of signatures of at most 73 bytes and its witness script.",
    params = {
        base = "The length of the transaction without its witnesses.",
        inputs = "Each input's threshold, and the length of its witness script.",
    },
    category = "Bridges",
)]
pub fn btc_release_vbytes(base: usize, inputs: &[(u32, usize)]) -> u64 {
    let witness = |(threshold, script): &(u32, usize)| 2 + *threshold as usize * 74 + if *script < 0xfd { 1 } else { 3 } + *script;
    let weight = 4 * base + 2 + inputs.iter().map(witness).sum::<usize>();

    weight.div_ceil(4) as u64
}

#[cfg(feature = "bitcoin")]
#[document(
    summary = "A withdrawal a release names, as the Bitcoin gateway says it stands.",
    category = "Bridges",
    see_also = "check_btc_release",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NamedWithdrawal {
    pub id: u64,
    #[document(summary = "The output script it asked to be paid to.")]
    pub script: Vec<u8>,
    pub sats: u64,
    #[document(summary = "queued, proposed or paid.")]
    pub state: String,
    #[document(summary = "The outpoint its release must spend, after one paying it was cancelled.")]
    pub anchor: Option<String>,
}

#[cfg(feature = "bitcoin")]
#[document(
    summary = "An output the Bitcoin gateway holds that a release spends, as it says it stands.",
    category = "Bridges",
    see_also = "check_btc_release",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpentOutput {
    pub sats: u64,
    #[document(summary = "The federation it was paid to.")]
    pub version: u64,
    #[document(summary = "unspent, pending or spent.")]
    pub state: String,
    #[document(summary = "How many of that federation must sign.")]
    pub threshold: u32,
    #[document(summary = "The length of the witness script that spends it.")]
    pub script_len: usize,
}

#[cfg(feature = "bitcoin")]
#[document(
    summary = "Everything a federation signer checks a release against, each read from the Bitcoin gateway's state rather than taken from what proposed it.",
    category = "Bridges",
    see_also = "check_btc_release",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseFacts {
    #[document(summary = "The transaction as the gateway keeps it, without witnesses.")]
    pub raw: Vec<u8>,
    #[document(summary = "The transaction's id the gateway keeps.")]
    pub txid: Vec<u8>,
    #[document(summary = "The federation it was proposed under.")]
    pub version: u64,
    #[document(summary = "The withdrawals it names, in order.")]
    pub withdrawals: Vec<NamedWithdrawal>,
    #[document(summary = "What each input spends, in order, with its outpoint as the gateway writes it.")]
    pub inputs: Vec<(String, SpentOutput)>,
    #[document(summary = "The bare multisig of the federation it was proposed under.")]
    pub change_script: Vec<u8>,
    #[document(summary = "The most fee a release may pay, in satoshis.")]
    pub fee_cap: u64,
    #[document(summary = "The least it may pay a virtual byte.")]
    pub fee_floor: u64,
}

#[cfg(feature = "bitcoin")]
#[document(
    summary = "Whether a federation signer should sign a release. The gateway checks all of this when it is proposed; a signer goes over it again, as what it signs pays real bitcoin and a fault in the contract should not be enough to take it: each withdrawal named once, paid as asked, and still waiting; each input once, held, pending; only change besides, of the size and to the federation it should be; a fee between the floor and the cap; and nothing Bitcoin would refuse or delay.",
    returns = "Why not, or nothing when it may sign.",
    category = "Bridges",
)]
pub fn check_btc_release(facts: &ReleaseFacts) -> Result<(), String> {
    use crate::crypto::bitcoin::tx::{Transaction, p2wsh};

    let (tx, witnessed) = Transaction::read(&facts.raw)?;

    if witnessed || tx.base() != facts.raw {
        return Err("its transaction is not written as Bitcoin writes it".to_string());
    }

    if crate::crypto::bitcoin::sha256d(&facts.raw).as_slice() != facts.txid.as_slice() {
        return Err("its transaction is not the id the gateway keeps".to_string());
    }

    if tx.version != 2 || tx.lock_time != 0 {
        return Err("it is not version 2 with no lock time".to_string());
    }

    if tx.inputs.len() != facts.inputs.len() {
        return Err("the gateway's account of its inputs does not match it".to_string());
    }

    let mut spends = std::collections::HashSet::new();
    let mut spent = 0;

    for (input, (outpoint, held)) in tx.inputs.iter().zip(&facts.inputs) {
        if !spends.insert(outpoint) {
            return Err(format!("it spends {outpoint} twice"));
        }

        if input.sequence != 0xffff_ffff || !input.script_sig.is_empty() {
            return Err(format!("its input {outpoint} has a delay or a script"));
        }

        if held.state != "pending" {
            return Err(format!("the gateway says its input {outpoint} is {}", held.state));
        }

        if facts.withdrawals.is_empty() && held.version >= facts.version {
            return Err("a sweep of what is no older federation's".to_string());
        }

        spent += held.sats;
    }

    let mut paid = 0;
    let mut named = std::collections::HashSet::new();

    for (at, withdrawal) in facts.withdrawals.iter().enumerate() {
        if !named.insert(withdrawal.id) {
            return Err(format!("it names withdrawal {} twice", withdrawal.id));
        }

        if withdrawal.state != "proposed" {
            return Err(format!("withdrawal {} is {}", withdrawal.id, withdrawal.state));
        }

        let output = tx.outputs.get(at).ok_or_else(|| format!("it does not pay withdrawal {}", withdrawal.id))?;

        if output.script != withdrawal.script || output.value != withdrawal.sats {
            return Err(format!("it does not pay withdrawal {} as asked", withdrawal.id));
        }

        if let Some(anchor) = &withdrawal.anchor
            && !spends.contains(anchor)
        {
            return Err(format!("withdrawal {} was in a cancelled release this does not conflict with", withdrawal.id));
        }

        paid += output.value;
    }

    match &tx.outputs[facts.withdrawals.len().min(tx.outputs.len())..] {
        [] if facts.withdrawals.is_empty() => return Err("it pays nothing".to_string()),
        [] => {}
        [change] if change.script == p2wsh(&facts.change_script) && change.value >= BTC_DUST => paid += change.value,
        [_] => return Err("its last output is not change to its federation".to_string()),
        _ => return Err("it pays more than its withdrawals and change".to_string()),
    }

    let fee = spent.checked_sub(paid).ok_or("it pays more than it spends")?;
    let sizes: Vec<(u32, usize)> = facts.inputs.iter().map(|(_, held)| (held.threshold, held.script_len)).collect();
    let least = btc_release_vbytes(facts.raw.len(), &sizes) * facts.fee_floor;

    if fee == 0 || fee > facts.fee_cap || fee < least {
        return Err(format!("its fee of {fee} is not from {least}, the floor for its size, to the cap of {}", facts.fee_cap));
    }

    Ok(())
}

#[cfg(feature = "bitcoin")]
#[document(summary = "What change is at least, in satoshis: Bitcoin's nodes relay nothing with an output under its dust.", category = "Bridges")]
pub const BTC_DUST: u64 = 546;

#[document(summary = "What both gateways hash a transfer's digest under.", category = "Bridges")]
pub const DOMAIN: &str = "chainv4-gateway";

#[document(summary = "Gateway.sol's gateway, as solc wrote it for Paris: what chainv4's tools deploy on an EVM chain, its constructor's arguments after it.", category = "Bridges")]
pub const GATEWAY_CODE: &str = include_str!("../contracts/evm/Gateway.bin");

#[document(
    summary = "The templates the EVM gateway clones each wrapped asset from, deployed once before it: a wrapped ERC-20, ERC-721 and ERC-1155.",
    category = "Bridges",
)]
pub const WRAPPED_CODES: [&str; 3] = [
    include_str!("../contracts/evm/WrappedToken.bin"),
    include_str!("../contracts/evm/WrappedNft.bin"),
    include_str!("../contracts/evm/WrappedMulti.bin"),
];

#[document(
    summary = "What a transfer moves.",
    category = "Bridges",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssetKind {
    #[document(summary = "A chain's coin: DEV here, ETH on Ethereum.")]
    Coin = 0,
    #[document(summary = "A fungible token: a token-standard token here, an ERC-20 there.")]
    Token = 1,
    #[document(summary = "One NFT: an NFT collection's here, an ERC-721's there.")]
    Nft = 2,
    #[document(summary = "An amount of one id: a multi-token collection's here, an ERC-1155's there.")]
    Multi = 3,
}

impl AssetKind {
    #[document(parent = "AssetKind", summary = "The kind its number names, as the gateways write it.")]
    pub fn from_number(number: u64) -> Option<AssetKind> {
        Some(match number {
            0 => AssetKind::Coin,
            1 => AssetKind::Token,
            2 => AssetKind::Nft,
            3 => AssetKind::Multi,
            _ => return None,
        })
    }
}

#[document(
    summary = "A transfer between two gateways, as each writes it and the federation signs it. Chains are named by their EVM chain ids, chainv4's included; addresses on chainv4 as their 33 bytes (Address::to_abi), an EVM chain's as their 20, and a gateway on chainv4 as its identity().",
    category = "Bridges",
    see_also = "Transfer::digest",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transfer {
    #[document(summary = "The gateway it leaves from, on the source chain: 20 bytes on an EVM chain, a gateway here as its identity(), its 33 bytes and when it was set up.")]
    pub from: Vec<u8>,
    #[document(summary = "The gateway it arrives at, on the target chain, written as from is.")]
    pub to: Vec<u8>,
    pub source: u64,
    pub target: u64,
    #[document(summary = "The source gateway's count of transfers it sent, from 1.")]
    pub nonce: u64,
    pub kind: AssetKind,
    #[document(summary = "The chain the asset is native to.")]
    pub origin: u64,
    #[document(summary = "The asset's address on its origin chain; empty for a coin.")]
    pub asset: Vec<u8>,
    #[document(summary = "An NFT's or multi-token id's id; 0 otherwise.")]
    pub id: u256,
    #[document(summary = "How much: 1 for an NFT.")]
    pub amount: u256,
    #[document(summary = "Who gets it on the target chain.")]
    pub recipient: Vec<u8>,
    pub name: String,
    pub symbol: String,
    pub decimals: u8,
    #[document(summary = "An NFT's or multi-token id's URI on its origin chain; empty otherwise.")]
    pub uri: String,
}

fn keccak(bytes: &[u8]) -> [u8; 32] {
    Hasher::digest(HashType::Keccak256, bytes).to_bytes().try_into().expect("32 bytes")
}

impl Transfer {
    #[document(
        parent = "Transfer",
        summary = "Whether it is for this gateway on this chain, from its peer: what Gateway.sol's logs still hold after a chain's reset is for another, and no signer's business.",
        params = { here = "This chain's id.", identity = "The gateway here, as its identity() writes it.", peer = "The gateway it left, on its chain." },
    )]
    pub fn arrives_at(&self, here: u64, identity: &[u8], peer: &[u8]) -> bool {
        self.target == here && self.to == identity && self.from == peer
    }

    #[document(
        parent = "Transfer",
        summary = "The key Gateway.sol marks it delivered by: keccak256 of its source chain, its source gateway's hash and its nonce.",
    )]
    pub fn delivered_key(&self) -> Vec<u8> {
        let values = [AbiValue::Uint(u256::from(self.source)), AbiValue::FixedBytes(keccak(&self.from).to_vec()), AbiValue::Uint(u256::from(self.nonce))];

        keccak(&encode_values(&[AbiType::Uint(64), AbiType::FixedBytes(32), AbiType::Uint(64)], &values).expect("a key encodes")).to_vec()
    }

    #[document(
        parent = "Transfer",
        summary = "What the federation signs: keccak256 of the domain, the head (gateways, chains, nonce, kind and origin, hashed) and the body (asset, id, amount, recipient and names, hashed), ABI-encoded, as both gateways compute it.",
    )]
    pub fn digest(&self) -> [u8; 32] {
        let number = |value: u64| AbiValue::Uint(u256::from(value));
        let head = encode_values(
            &[AbiType::Bytes, AbiType::Bytes, AbiType::Uint(64), AbiType::Uint(64), AbiType::Uint(64), AbiType::Uint(8), AbiType::Uint(64)],
            &[
                AbiValue::Bytes(self.from.clone()),
                AbiValue::Bytes(self.to.clone()),
                number(self.source),
                number(self.target),
                number(self.nonce),
                number(self.kind as u64),
                number(self.origin),
            ],
        )
        .expect("a head encodes");
        let body = encode_values(
            &[AbiType::Bytes, AbiType::Uint(256), AbiType::Uint(256), AbiType::Bytes, AbiType::String, AbiType::String, AbiType::Uint(8), AbiType::String],
            &[
                AbiValue::Bytes(self.asset.clone()),
                AbiValue::Uint(self.id),
                AbiValue::Uint(self.amount),
                AbiValue::Bytes(self.recipient.clone()),
                AbiValue::String(self.name.clone()),
                AbiValue::String(self.symbol.clone()),
                number(u64::from(self.decimals)),
                AbiValue::String(self.uri.clone()),
            ],
        )
        .expect("a body encodes");
        let outer = encode_values(
            &[AbiType::String, AbiType::FixedBytes(32), AbiType::FixedBytes(32)],
            &[AbiValue::String(DOMAIN.to_string()), AbiValue::FixedBytes(keccak(&head).to_vec()), AbiValue::FixedBytes(keccak(&body).to_vec())],
        )
        .expect("a digest encodes");

        keccak(&outer)
    }

    #[document(
        parent = "Transfer",
        summary = "A signer's signature over its digest: r, s and v, 65 bytes, v 27 or 28, as ecrecover takes it and the gateway here recovers.",
    )]
    pub fn sign(&self, secret: &[u8; 32]) -> [u8; 65] {
        sign_digest(secret, &self.digest())
    }

    #[document(
        parent = "Transfer",
        summary = "The arguments the gateway here's attest takes for it, the signature last: what a signer's transaction carries.",
    )]
    pub fn attest_args(&self, signature: &[u8; 65]) -> Vec<TxCallArgumentType> {
        let number = |value: u64| TxCallArgumentType::U64(value);

        vec![
            number(self.source),
            number(self.target),
            number(self.nonce),
            number(self.kind as u64),
            number(self.origin),
            TxCallArgumentType::Bytes(self.asset.clone()),
            TxCallArgumentType::U256(self.id),
            TxCallArgumentType::U256(self.amount),
            TxCallArgumentType::Bytes(self.recipient.clone()),
            TxCallArgumentType::String(self.name.clone()),
            TxCallArgumentType::String(self.symbol.clone()),
            number(u64::from(self.decimals)),
            TxCallArgumentType::String(self.uri.clone()),
            TxCallArgumentType::Bytes(signature.to_vec()),
        ]
    }
}

#[document(summary = "A transfer as Gateway.sol's functions take and give it: one ABI tuple.", category = "Bridges")]
pub const TRANSFER_TUPLE: &str = "(bytes,bytes,uint64,uint64,uint64,uint8,uint64,bytes,uint256,uint256,bytes,string,string,uint8,string)";

fn selector(signature: &str) -> Vec<u8> {
    keccak(signature.as_bytes())[..4].to_vec()
}

fn types(signature: &str) -> Vec<AbiType> {
    crate::abi::parse_signature(signature).expect("the gateway's signatures parse").1
}

impl Transfer {
    #[document(parent = "Transfer", summary = "It as Gateway.sol's Transfer tuple.")]
    pub fn evm_tuple(&self) -> AbiValue {
        let number = |value: u64| AbiValue::Uint(u256::from(value));

        AbiValue::Tuple(vec![
            AbiValue::Bytes(self.from.clone()),
            AbiValue::Bytes(self.to.clone()),
            number(self.source),
            number(self.target),
            number(self.nonce),
            number(self.kind as u64),
            number(self.origin),
            AbiValue::Bytes(self.asset.clone()),
            AbiValue::Uint(self.id),
            AbiValue::Uint(self.amount),
            AbiValue::Bytes(self.recipient.clone()),
            AbiValue::String(self.name.clone()),
            AbiValue::String(self.symbol.clone()),
            number(u64::from(self.decimals)),
            AbiValue::String(self.uri.clone()),
        ])
    }

    #[document(parent = "Transfer", summary = "It from Gateway.sol's Transfer tuple, as its sent(nonce) returns it.", returns = "The transfer, or None for values that are not one.")]
    pub fn from_evm(value: &AbiValue) -> Option<Transfer> {
        let AbiValue::Tuple(fields) = value else {
            return None;
        };
        let bytes = |index: usize| match fields.get(index)? {
            AbiValue::Bytes(bytes) => Some(bytes.clone()),
            _ => None,
        };
        let wide = |index: usize| match fields.get(index)? {
            AbiValue::Uint(number) => Some(*number),
            _ => None,
        };
        let small = |index: usize| wide(index).and_then(|number| u64::try_from(number).ok());
        let text = |index: usize| match fields.get(index)? {
            AbiValue::String(text) => Some(text.clone()),
            _ => None,
        };

        Some(Transfer {
            from: bytes(0)?,
            to: bytes(1)?,
            source: small(2)?,
            target: small(3)?,
            nonce: small(4)?,
            kind: AssetKind::from_number(small(5)?)?,
            origin: small(6)?,
            asset: bytes(7)?,
            id: wide(8)?,
            amount: wide(9)?,
            recipient: bytes(10)?,
            name: text(11)?,
            symbol: text(12)?,
            decimals: u8::try_from(small(13)?).ok()?,
            uri: text(14)?,
        })
    }

    #[document(parent = "Transfer", summary = "Calldata delivering it to Gateway.sol with the federation's signatures: what a relayer sends.")]
    pub fn deliver_input(&self, signatures: &[[u8; 65]]) -> Vec<u8> {
        let signature = format!("deliver({TRANSFER_TUPLE},bytes[])");
        let values = [self.evm_tuple(), AbiValue::Array(signatures.iter().map(|signature| AbiValue::Bytes(signature.to_vec())).collect())];

        [selector(&signature), encode_values(&types(&signature), &values).expect("a delivery encodes")].concat()
    }
}

#[document(
    summary = "Calldata sending something through Gateway.sol to another chain: the coin with the call, or an ERC-20, ERC-721 or ERC-1155 the gateway may move.",
    params = {
        target = "The chain it goes to.",
        recipient = "Who gets it there: a chainv4 address's 33 bytes.",
        kind = "What it is.",
        asset = "The token or collection; zero for the coin.",
        id = "An NFT's or multi-token id's id.",
        amount = "How much; 1 for an NFT.",
    },
    category = "Bridges",
)]
pub fn send_input(target: u64, recipient: &[u8], kind: AssetKind, asset: [u8; 20], id: u256, amount: u256) -> Vec<u8> {
    let signature = "send(uint64,bytes,uint8,address,uint256,uint256)";
    let values = [
        AbiValue::Uint(u256::from(target)),
        AbiValue::Bytes(recipient.to_vec()),
        AbiValue::Uint(u256::from(kind as u64)),
        AbiValue::Address(asset),
        AbiValue::Uint(id),
        AbiValue::Uint(amount),
    ];

    [selector(signature), encode_values(&types(signature), &values).expect("a send encodes")].concat()
}

#[document(
    summary = "Gateway.sol's constructor arguments, to follow its code: the federation as Ethereum addresses, its threshold and version, what the chain's coin is called, and the wrapped templates.",
    category = "Bridges",
)]
pub fn gateway_constructor(signers: &[[u8; 20]], threshold: u64, version: u64, coin: (&str, &str), templates: [[u8; 20]; 3]) -> Vec<u8> {
    encode_values(
        &types("(address[],uint256,uint64,string,string,address[3])"),
        &[
            AbiValue::Array(signers.iter().map(|signer| AbiValue::Address(*signer)).collect()),
            AbiValue::Uint(u256::from(threshold)),
            AbiValue::Uint(u256::from(version)),
            AbiValue::String(coin.0.to_string()),
            AbiValue::String(coin.1.to_string()),
            AbiValue::Array(templates.iter().map(|template| AbiValue::Address(*template)).collect()),
        ],
    )
    .expect("a constructor encodes")
}

#[document(
    summary = "What the federation in force signs to hand Gateway.sol the next one: the gateway, its chain, the next version, signers and threshold.",
    category = "Bridges",
)]
pub fn signers_digest(gateway: [u8; 20], chain_id: u64, version: u64, signers: &[[u8; 20]], threshold: u64) -> [u8; 32] {
    keccak(
        &encode_values(
            &types("(string,address,uint256,uint64,address[],uint256)"),
            &[
                AbiValue::String("chainv4-gateway-signers".to_string()),
                AbiValue::Address(gateway),
                AbiValue::Uint(u256::from(chain_id)),
                AbiValue::Uint(u256::from(version)),
                AbiValue::Array(signers.iter().map(|signer| AbiValue::Address(*signer)).collect()),
                AbiValue::Uint(u256::from(threshold)),
            ],
        )
        .expect("a signer set encodes"),
    )
}

#[document(summary = "Calldata handing Gateway.sol the next federation, with the signatures of the one in force over signers_digest.", category = "Bridges")]
pub fn set_signers_input(signers: &[[u8; 20]], threshold: u64, version: u64, signatures: &[[u8; 65]]) -> Vec<u8> {
    let signature = "setSigners(address[],uint256,uint64,bytes[])";
    let values = [
        AbiValue::Array(signers.iter().map(|signer| AbiValue::Address(*signer)).collect()),
        AbiValue::Uint(u256::from(threshold)),
        AbiValue::Uint(u256::from(version)),
        AbiValue::Array(signatures.iter().map(|signature| AbiValue::Bytes(signature.to_vec())).collect()),
    ];

    [selector(signature), encode_values(&types(signature), &values).expect("a signer set encodes")].concat()
}

#[document(summary = "A 32-byte digest signed as the federation signs: r, s and v, v 27 or 28.", category = "Bridges")]
pub fn sign_digest(secret: &[u8; 32], digest: &[u8; 32]) -> [u8; 65] {
    let (rs, parity) = crate::crypto::secp256k1::sign_recoverable(secret, digest);
    let mut signature = [0; 65];
    signature[..64].copy_from_slice(&rs);
    signature[64] = 27 + parity;

    signature
}

#[document(
    summary = "An address on chainv4 as the gateways write it: its 33 bytes, its kind's tag first.",
    category = "Bridges",
)]
pub fn chain_bytes(address: &Address) -> Vec<u8> {
    address.to_abi().to_vec()
}

#[cfg(test)]
mod tests;
