//! The chain's directory (docs/governance.md#directory): the contracts it announces for the apps
//! and tools to use, the bridges' gateways here and Gateway.sol on each EVM chain, the swap
//! contract, the DEX and its EVM router, and the NFT factory. Genesis may name them, and
//! governance replaces them with Change::Directory, or on a chain without governance the admin
//! genesis names, with Tx::SetDirectory; GET /status shows them, so the apps never take one from a
//! name someone else could register first.

use {
    crate::{crypto::hashable::Hashable, structs::address::Address},
    crate::db::{Column, DbError, State},
    r#macro::document,
    wincode::{SchemaRead, SchemaWrite},
};

#[document(
    summary = "Where the directory in force is kept, in the Validators column: one entry, genesis's or governance's, so a light client proves it.",
    category = "Governance",
    see_also = "Directory, Changes::directory",
)]
pub const DIRECTORY_KEY: &[u8] = b"change:directory";

#[document(
    summary = "Where the directory's admin is kept, in the Validators column, on a chain without governance whose genesis names one: gone once the admin renounces, so a light client proves who it is, or that there is none.",
    category = "Governance",
    see_also = "DirectoryAdmin, Tx::SetDirectory",
)]
pub const DIRECTORY_ADMIN_KEY: &[u8] = b"directory:admin";

#[document(
    summary = "Who sets the directory on a chain without governance: the address genesis named, or the one it was handed to. In state while there is one.",
    category = "Governance",
    see_also = "Genesis::directory_admin, Tx::SetDirectory, Tx::HandDirectory",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct DirectoryAdmin {
    #[document(summary = "The admin's address: any kind, a multisig's too.")]
    pub admin: Address,
}

impl Hashable for DirectoryAdmin {}

#[document(parent = "DirectoryAdmin", summary = "The directory's admin in a state, or None: on a chain with governance, a genesis that named none, or once renounced.")]
pub fn directory_admin(state: &State) -> Result<Option<Address>, DbError> {
    Ok(state.get_value::<DirectoryAdmin>(Column::Validators, DIRECTORY_ADMIN_KEY)?.map(|kept| kept.admin))
}

#[document(parent = "DirectoryAdmin", summary = "Makes an address the directory's admin, or with None leaves it none.")]
pub fn write_directory_admin(state: &State, admin: Option<Address>) {
    match admin {
        Some(admin) => state.put_value(Column::Validators, DIRECTORY_ADMIN_KEY.to_vec(), &DirectoryAdmin { admin }),
        None => state.delete(Column::Validators, DIRECTORY_ADMIN_KEY.to_vec()),
    }
}

#[document(summary = "The most EVM chains the directory may name a Gateway.sol on.", category = "Governance", see_also = "Directory")]
pub const MOST_EVM_GATEWAYS: usize = 32;

#[document(summary = "Gateway.sol on one EVM chain: the chain's id and the contract's address there.", category = "Bridges", see_also = "Directory")]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct EvmGateway {
    #[document(summary = "The EVM chain's id, as its CHAINID answers.")]
    pub chain_id: u64,
    #[document(summary = "Gateway.sol's 20-byte address there.")]
    pub gateway: [u8; 20],
}

#[document(
    summary = "The contracts the chain announces for the apps and tools to use: the bridges' gateways, the swap contract, the DEX and the NFT factory. Each is None while there is none. What clients trust, before what a node's operator names and never from a name someone else could register first.",
    category = "Governance",
    see_also = "Genesis::directory, Change::Directory",
)]
#[derive(Clone, Debug, Default, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct Directory {
    #[document(summary = "The bridges' gateway here, contracts/lua/gateway.lua: the signature board and the EVM chains' bridge.")]
    pub gateway: Option<Address>,
    #[document(summary = "The Bitcoin gateway here, contracts/lua/btc_gateway.lua, which names its relay and wBTC.")]
    pub btc_gateway: Option<Address>,
    #[document(summary = "Gateway.sol on each EVM chain, in the order of their chain ids, each chain once: at most MOST_EVM_GATEWAYS.")]
    pub evm_gateways: Vec<EvmGateway>,
    #[document(summary = "The swap contract, contracts/lua/swap.lua, that new offers use.")]
    pub swap: Option<Address>,
    #[document(summary = "The native DEX, contracts/lua/dex.lua.")]
    pub dex: Option<Address>,
    #[document(summary = "The EVM DEX's router on this chain's EVM, 20 bytes.")]
    pub dex_router: Option<[u8; 20]>,
    #[document(summary = "The split router, contracts/lua/router.lua, that divides one trade between both DEXes in one transaction.")]
    pub dex_split: Option<Address>,
    #[document(summary = "The NFT factory, contracts/lua/nft_factory.lua, that new collections are made with.")]
    pub nft_factory: Option<Address>,
}

impl Hashable for Directory {}

impl Directory {
    #[document(
        parent = "Directory",
        summary = "Why it cannot be a chain's: a contract here that is not a contract, more than MOST_EVM_GATEWAYS EVM chains, their ids not in order, repeated or 0, or an EVM address of zero.",
    )]
    pub fn problem(&self) -> Option<&'static str> {
        if [self.gateway, self.btc_gateway, self.swap, self.dex, self.dex_split, self.nft_factory].iter().flatten().any(|address| !matches!(address, Address::Contract(_))) {
            return Some("the directory's contracts here are contracts");
        }

        if self.evm_gateways.len() > MOST_EVM_GATEWAYS {
            return Some("the directory names Gateway.sol on at most 32 EVM chains");
        }

        if self.evm_gateways.iter().any(|evm| evm.chain_id == 0 || evm.gateway == [0; 20]) || self.dex_router == Some([0; 20]) {
            return Some("each EVM contract has a chain id and an address");
        }

        if self.evm_gateways.windows(2).any(|pair| pair[0].chain_id >= pair[1].chain_id) {
            return Some("the EVM gateways go in the order of their chain ids, each chain once");
        }

        None
    }

    #[document(parent = "Directory", summary = "Gateway.sol on an EVM chain, by its id, if the directory names one.")]
    pub fn evm_gateway(&self, chain_id: u64) -> Option<[u8; 20]> {
        self.evm_gateways.iter().find(|evm| evm.chain_id == chain_id).map(|evm| evm.gateway)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_directory_holds_together_or_says_why_not() {
        let good = Directory {
            gateway: Some(Address::Contract([1; 32])),
            btc_gateway: Some(Address::Contract([2; 32])),
            evm_gateways: vec![EvmGateway { chain_id: 1, gateway: [3; 20] }, EvmGateway { chain_id: 8453, gateway: [4; 20] }],
            swap: Some(Address::Contract([5; 32])),
            dex: Some(Address::Contract([6; 32])),
            dex_router: Some([7; 20]),
            dex_split: Some(Address::Contract([9; 32])),
            nft_factory: Some(Address::Contract([8; 32])),
        };

        assert_eq!(good.problem(), None);
        assert_eq!(Directory::default().problem(), None, "none at all, as a chain clears them");
        assert_eq!((good.evm_gateway(8453), good.evm_gateway(10)), (Some([4; 20]), None));

        let evm = |pairs: &[(u64, u8)]| pairs.iter().map(|(chain_id, byte)| EvmGateway { chain_id: *chain_id, gateway: [*byte; 20] }).collect::<Vec<_>>();
        let key = Some(Address::Ed25519([1; 32]));

        for (bad, why) in [
            (Directory { gateway: key, ..good.clone() }, "contracts"),
            (Directory { btc_gateway: key, ..good.clone() }, "contracts"),
            (Directory { swap: key, ..good.clone() }, "contracts"),
            (Directory { dex: key, ..good.clone() }, "contracts"),
            (Directory { nft_factory: key, ..good.clone() }, "contracts"),
            (Directory { evm_gateways: (1..=33).map(|chain_id| EvmGateway { chain_id, gateway: [1; 20] }).collect(), ..good.clone() }, "at most 32"),
            (Directory { evm_gateways: evm(&[(0, 1)]), ..good.clone() }, "chain id"),
            (Directory { evm_gateways: evm(&[(1, 0)]), ..good.clone() }, "an address"),
            (Directory { dex_router: Some([0; 20]), ..good.clone() }, "an address"),
            (Directory { evm_gateways: evm(&[(5, 1), (1, 2)]), ..good.clone() }, "order"),
            (Directory { evm_gateways: evm(&[(5, 1), (5, 2)]), ..good.clone() }, "order"),
        ] {
            assert!(bad.problem().is_some_and(|problem| problem.contains(why)), "{bad:?}");
        }
    }
}
