//! The client side of the binary: making keys, signing transactions and submitting them to a
//! node's HTTP API, and checking contract code's storage layout before it is sent.

use {
    crate::consensus::DevAuthority,
    crate::crypto::keypair::{KeyType, Keypair},
    crate::structs::address::Address,
    crate::structs::amount::Amount,
    crate::structs::hex::from_hex,
    crate::structs::tx::TxCallArgumentType,
    crate::structs::u256::u256,
    std::error::Error,
    std::fs,
    std::io::Write,
    std::path::{Path, PathBuf},
};

pub mod genesis;
mod layout;
#[cfg(feature = "client")]
mod send;

pub use layout::{LayoutArgs, layout};

#[cfg(feature = "client")]
pub use send::{SendArgs, TxArgs, send};

type CliResult<T> = Result<T, Box<dyn Error>>;

#[derive(clap::Args)]
pub struct KeygenArgs {
    /// The signature scheme: ed25519 or edblake.
    #[arg(long, default_value = "ed25519", value_parser = parse_scheme)]
    scheme: KeyType,
    /// Where to write the key. An existing file is never overwritten.
    #[arg(long, default_value = "chainv4.key")]
    out: PathBuf,
}

/// Makes a key and writes it, readable only by its owner.
pub fn keygen(args: KeygenArgs) -> CliResult<()> {
    let keypair = Keypair::generate(args.scheme)?;
    write_key(&args.out, &keypair)?;
    println!("wrote {}", args.out.display());
    println!("address {}", keypair.public().address());

    Ok(())
}

/// The key to sign with: a key file, or the development key when there is none.
pub fn signer(key: Option<&Path>) -> CliResult<Keypair> {
    match key {
        Some(path) => read_key(path),
        None => Ok(DevAuthority::dev_keypair()),
    }
}

/// A key file holds one line: the scheme, a colon and the 32-byte seed in hex, like an address.
fn write_key(path: &Path, keypair: &Keypair) -> CliResult<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);

    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);

    let mut file = options
        .open(path)
        .map_err(|err| format!("cannot create {}: {err}", path.display()))?;
    writeln!(file, "{}", keypair.to_key_file())?;

    Ok(())
}

fn read_key(path: &Path) -> CliResult<Keypair> {
    let text = fs::read_to_string(path).map_err(|err| format!("cannot read {}: {err}", path.display()))?;

    Ok(Keypair::from_key_file(&text).map_err(|err| format!("{}: {err}", path.display()))?)
}

fn parse_scheme(text: &str) -> Result<KeyType, String> {
    match text {
        #[cfg(feature = "ed25519")]
        "ed25519" => Ok(KeyType::Ed25519),
        #[cfg(feature = "edblake")]
        "edblake" => Ok(KeyType::EdBlake),
        other => Err(format!("unknown scheme {other}")),
    }
}

/// A whole, non-negative amount in decimal.
pub fn parse_amount(text: &str) -> Result<u256, String> {
    u256::from_str_radix(text, 10).map_err(|_| format!("{text} is not a whole amount"))
}

/// The narrowest Amount that holds a value.
pub fn amount(value: u256) -> Amount {
    match u128::try_from(value) {
        Ok(small) => Amount::U128(small),
        Err(_) => Amount::U256(value),
    }
}

/// A call argument written as type:value, such as i64:-5, string:hi or address:ed25519:0x…, or
/// nil for no value.
pub fn parse_argument(text: &str) -> Result<TxCallArgumentType, String> {
    if text == "nil" {
        return Ok(TxCallArgumentType::Nil);
    }

    let (kind, value) = text
        .split_once(':')
        .ok_or_else(|| format!("{text} needs a type, such as i64:{text}"))?;
    let bad = |_| format!("{value} is not a valid {kind}");
    let argument = match kind {
        "address" => TxCallArgumentType::Address(value.parse().map_err(|err: crate::structs::address::ParseAddressError| err.to_string())?),
        "amount" => TxCallArgumentType::Amount(amount(parse_amount(value)?)),
        "bytes" => TxCallArgumentType::Bytes(from_hex(value).ok_or_else(|| format!("{value} is not hex"))?),
        "bool" => TxCallArgumentType::Bool(value.parse().map_err(|_| format!("{value} is not true or false"))?),
        "string" => TxCallArgumentType::String(value.to_string()),
        "number" => TxCallArgumentType::Number(value.parse().map_err(|_| format!("{value} is not a number"))?),
        "i64" => TxCallArgumentType::I64(value.parse().map_err(bad)?),
        "u64" => TxCallArgumentType::U64(value.parse().map_err(bad)?),
        "i128" => TxCallArgumentType::I128(value.parse().map_err(bad)?),
        "u128" => TxCallArgumentType::U128(value.parse().map_err(bad)?),
        "u256" => TxCallArgumentType::U256(parse_amount(value)?),
        other => {
            return Err(format!(
                "unknown type {other}; use address, amount, bytes, bool, string, number, i64, u64, i128, u128, u256, or nil alone"
            ));
        }
    };

    Ok(argument)
}

/// Where the next deploy from an address lands, on a chain with this hash.
pub fn deployed_at(sender: &Address, nonce: u64, hash: crate::structs::hash::HashType) -> Address {
    crate::registry::contracts::contract_address(sender, nonce, hash)
}

#[cfg(test)]
mod tests;
