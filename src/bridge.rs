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
    summary = "An address's deposit script under a federation, as the Bitcoin gateway writes it: its 33 bytes, dropped, then the multisig. Paid as its P2WSH, the gateway mints wBTC to the address for it; computed here, so a wallet never takes the address to pay from a node.",
    category = "Bridges",
    see_also = "btc_multisig",
)]
pub fn btc_deposit_script(recipient: &Address, threshold: u32, keys: &[[u8; 33]]) -> Vec<u8> {
    [&[33][..], &recipient.to_abi(), &[0x75], &btc_multisig(threshold, keys)].concat()
}

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
    summary = "A transfer between two gateways, as each writes it and the federation signs it. Chains are named by their EVM chain ids, chainv4's included; addresses on chainv4 as their 33 bytes (Address::to_abi), an EVM chain's as their 20.",
    category = "Bridges",
    see_also = "Transfer::digest",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transfer {
    #[document(summary = "The gateway it leaves from, on the source chain.")]
    pub from: Vec<u8>,
    #[document(summary = "The gateway it arrives at, on the target chain.")]
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
