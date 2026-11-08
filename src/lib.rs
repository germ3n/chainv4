//! chainv4: a blockchain with Lua and WebAssembly contracts. The node binary, and clients such
//! as the explorer, are built on this library.

#[cfg(feature = "abi")]
pub mod abi;
#[cfg(all(feature = "anchoring", feature = "chain"))]
pub mod anchor;
#[cfg(feature = "http")]
pub mod api;
#[cfg(feature = "chain")]
pub mod chain;
#[cfg(feature = "node")]
pub mod cli;
pub mod consensus;
pub mod crypto;
pub mod db;
#[cfg(feature = "sha256")]
pub mod faucet;
pub mod governance;
pub mod light;
pub mod params;
pub mod proofs;
#[cfg(feature = "node")]
pub mod mempool;
#[cfg(any(feature = "node", feature = "worker"))]
pub mod mining;
#[cfg(feature = "node")]
pub mod net;
#[cfg(feature = "node")]
pub mod node;
pub mod registry;
#[cfg(feature = "release")]
pub mod release;
#[cfg(feature = "client")]
pub mod remote;
/// The Ethereum light client anchoring's settings name networks by: what the apps decode them with.
pub use ethlight;
pub mod structs;
#[cfg(feature = "swap")]
pub mod swap;
#[cfg(feature = "dex")]
pub mod dex;
#[cfg(feature = "nft")]
pub mod nft;
#[cfg(feature = "bridge")]
pub mod bridge;
#[cfg(feature = "bitvm")]
pub mod bitvm;
#[cfg(feature = "vm")]
pub mod vm;

#[cfg(test)]
mod pentest_decoders;
