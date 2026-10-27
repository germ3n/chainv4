//! chainv4: a blockchain with Lua and WebAssembly contracts. The node binary, and clients such
//! as the explorer, are built on this library.

#[cfg(feature = "abi")]
pub mod abi;
#[cfg(feature = "http")]
pub mod api;
#[cfg(feature = "node")]
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
#[cfg(feature = "node")]
pub mod mempool;
#[cfg(feature = "node")]
pub mod mining;
#[cfg(feature = "node")]
pub mod net;
#[cfg(feature = "node")]
pub mod node;
pub mod registry;
pub mod structs;
#[cfg(feature = "swap")]
pub mod swap;
#[cfg(feature = "vm")]
pub mod vm;
