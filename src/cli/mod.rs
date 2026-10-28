//! The client side of the binary: making keys, signing transactions and submitting them to a
//! node's HTTP API, and checking contract code's storage layout before it is sent.

use {
    crate::consensus::DevAuthority,
    crate::crypto::keypair::{KeyType, Keypair},
    crate::structs::address::Address,
    crate::structs::amount::Amount,
    crate::structs::bundle::{Bundle, bundle},
    crate::structs::tx::TxCallArgumentType,
    crate::structs::u256::u256,
    std::error::Error,
    std::fs,
    std::io::Write,
    std::path::{Path, PathBuf},
};

pub mod db;
#[cfg(all(feature = "client", feature = "dex", feature = "evm"))]
pub(super) mod dex;
#[cfg(all(feature = "client", feature = "evm"))]
mod evm;
#[cfg(all(feature = "client", feature = "nft"))]
pub(super) mod nft;
#[cfg(all(feature = "client", feature = "bridge", feature = "swap"))]
pub(super) mod bridge;
#[cfg(all(feature = "client", feature = "sha256"))]
mod faucet;
pub mod genesis;
mod layout;
#[cfg(feature = "client")]
mod light;
#[cfg(feature = "client")]
pub(crate) mod bench;
#[cfg(feature = "client")]
pub(crate) mod multisig;
#[cfg(feature = "client")]
mod send;
#[cfg(all(feature = "client", feature = "swap"))]
mod swap;
mod worker;

pub use db::{DbArgs, db};
pub use layout::{LayoutArgs, layout};
pub use worker::{WorkerArgs, worker};

#[cfg(feature = "gpu")]
pub use worker::gpus;

#[cfg(feature = "client")]
pub use light::{LightArgs, light};
#[cfg(feature = "client")]
pub use multisig::{MultisigArgs, multisig};
#[cfg(feature = "client")]
pub use send::{SendArgs, TlsArgs, TxArgs, send};
#[cfg(feature = "client")]
pub use bench::{BenchArgs, bench};
#[cfg(all(feature = "client", feature = "swap"))]
pub use swap::{SwapArgs, swap};
#[cfg(all(feature = "client", feature = "dex", feature = "evm"))]
pub use dex::{DexArgs, dex};
#[cfg(all(feature = "client", feature = "nft"))]
pub use nft::{NftArgs, nft};
#[cfg(all(feature = "client", feature = "bridge", feature = "swap"))]
pub use bridge::{BridgeArgs, bridge};
#[cfg(all(feature = "client", feature = "sha256"))]
pub use faucet::{FaucetArgs, faucet};

type CliResult<T> = Result<T, Box<dyn Error>>;

#[derive(clap::Args)]
pub struct KeygenArgs {
    /// The signature scheme: ed25519, edblake, secp256k1 or bitcoin.
    #[arg(long, default_value = "ed25519", value_parser = parse_scheme)]
    scheme: KeyType,
    /// Where to write the key. An existing file is never overwritten.
    #[arg(long, default_value = "chainv4.key")]
    out: PathBuf,
    /// Make a new seed phrase, print it once, and write the key of --account from it. Write the
    /// words down: they are every account's keys.
    #[cfg(feature = "mnemonic")]
    #[arg(long, conflicts_with = "from_phrase")]
    phrase: bool,
    /// Read a seed phrase from standard input, and write the key of --account from it: the same key
    /// the wallet derives from the same words.
    #[cfg(feature = "mnemonic")]
    #[arg(long)]
    from_phrase: bool,
    /// How many words a new phrase has: 12 or 24.
    #[cfg(feature = "mnemonic")]
    #[arg(long, default_value_t = 12, requires = "phrase", value_parser = parse_words)]
    words: usize,
    /// Which account of the phrase, from 0.
    #[cfg(feature = "mnemonic")]
    #[arg(long, default_value_t = 0, value_parser = clap::value_parser!(u32).range(0..1 << 31))]
    account: u32,
    /// Read BIP39's optional passphrase from standard input too, after any phrase: other words
    /// for the same phrase, which make other keys.
    #[cfg(feature = "mnemonic")]
    #[arg(long)]
    passphrase: bool,
}

/// Makes a key and writes it, readable only by its owner: a fresh one, or one from a seed phrase.
pub fn keygen(args: KeygenArgs) -> CliResult<()> {
    keygen_reading(args, &mut std::io::stdin().lock())
}

/// keygen, reading any phrase and passphrase from input.
fn keygen_reading(args: KeygenArgs, input: &mut dyn std::io::BufRead) -> CliResult<()> {
    #[cfg(not(feature = "mnemonic"))]
    let _ = input;
    #[cfg(feature = "mnemonic")]
    let keypair = match (args.phrase, args.from_phrase) {
        (false, false) => Keypair::generate(args.scheme)?,
        (made, _) => {
            use crate::crypto::mnemonic::Phrase;

            let mut lines = std::io::BufRead::lines(&mut *input);
            let mut ask = |what: &str| -> CliResult<String> {
                eprint!("{what}: ");
                std::io::stderr().flush()?;

                Ok(lines.next().transpose()?.unwrap_or_default())
            };
            let phrase = match made {
                true => {
                    let phrase = Phrase::generate(args.words)?;
                    println!("phrase {phrase}");
                    eprintln!("Write these {} words down, in order, and keep them where only you can read them: they are every account's keys.", args.words);

                    phrase
                }
                false => Phrase::parse(&ask("the seed phrase")?)?,
            };
            let passphrase = match args.passphrase {
                true => ask("the passphrase")?,
                false => String::new(),
            };

            phrase.seed(&passphrase).key(args.scheme, args.account)
        }
    };
    #[cfg(not(feature = "mnemonic"))]
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

/// A Lua contract's files put together, from its entry file on disk: each `--@include` read from
/// beside the file that names it.
fn lua_bundle(path: &Path) -> CliResult<Bundle> {
    let source = fs::read_to_string(path).map_err(|err| format!("cannot read {}: {err}", path.display()))?;
    let folder = path.parent().unwrap_or(Path::new(""));
    let entry = path.file_name().map_or_else(|| path.display().to_string(), |name| name.to_string_lossy().into_owned());

    Ok(bundle(&entry, &source, |included| fs::read_to_string(folder.join(included)).map_err(|err| err.to_string()))?)
}

fn parse_scheme(text: &str) -> Result<KeyType, String> {
    text.parse()
}

#[cfg(feature = "mnemonic")]
fn parse_words(text: &str) -> Result<usize, String> {
    match text {
        "12" => Ok(12),
        "24" => Ok(24),
        _ => Err("a new phrase has 12 or 24 words".to_string()),
    }
}

/// A whole, non-negative amount in decimal.
pub fn parse_amount(text: &str) -> Result<u256, String> {
    u256::from_str_radix(text, 10).map_err(|_| format!("{text} is not a whole amount"))
}

/// An amount in whole coins, as people write one, such as 10 or 0.5: in the coin's smallest units,
/// at its decimals. More decimal places than the coin has are refused rather than rounded.
pub fn parse_coins(text: &str, decimals: u8) -> Result<u256, String> {
    let bad = || format!("{text} is not an amount of coins, such as 10 or 0.5");
    let (whole, fraction) = text.trim().split_once('.').unwrap_or((text.trim(), ""));

    if whole.is_empty() && fraction.is_empty() || !whole.chars().chain(fraction.chars()).all(|c| c.is_ascii_digit()) {
        return Err(bad());
    }

    if fraction.len() > usize::from(decimals) {
        return Err(format!("{text} has more decimal places than the coin's {decimals}"));
    }

    let digits = format!("{whole}{fraction:0<width$}", width = usize::from(decimals));

    u256::from_str_radix(&digits, 10).map_err(|_| bad())
}

/// A span of time as people write one, such as 24h, 30m, 90s or 2d: in milliseconds.
pub fn parse_duration(text: &str) -> Result<u64, String> {
    let bad = || format!("{text} is not a span of time, such as 24h, 30m or 90s");
    let text = text.trim();
    let split = text.find(|c: char| !c.is_ascii_digit()).ok_or_else(bad)?;
    let (count, unit) = text.split_at(split);
    let count: u64 = count.parse().map_err(|_| bad())?;
    let unit = match unit {
        "s" => 1_000,
        "m" => 60_000,
        "h" => 3_600_000,
        "d" => 86_400_000,
        _ => return Err(bad()),
    };

    count.checked_mul(unit).ok_or_else(bad)
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
    TxCallArgumentType::parse_typed(text)
}

/// Where the next deploy from an address lands, on a chain with this hash.
pub fn deployed_at(sender: &Address, nonce: u64, hash: crate::structs::hash::HashType) -> Address {
    crate::registry::contracts::contract_address(sender, nonce, hash)
}

#[cfg(test)]
mod tests;
