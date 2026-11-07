//! The client side of the binary: making keys, signing transactions and submitting them to a
//! node's HTTP API, and checking contract code's storage layout before it is sent.

use {
    crate::consensus::DevAuthority,
    crate::crypto::keypair::{KeyType, Keypair},
    crate::structs::address::Address,
    crate::structs::amount::Amount,
    crate::structs::bundle::{Bundle, bundle},
    crate::structs::digits::ungrouped,
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
#[cfg(feature = "lvm")]
mod calibrate;
#[cfg(all(feature = "lvm", feature = "node"))]
mod test;
#[cfg(feature = "client")]
mod claim;
#[cfg(feature = "client")]
pub(crate) mod multisig;
#[cfg(feature = "client")]
mod send;
#[cfg(feature = "client")]
pub(crate) mod outside;
#[cfg(feature = "client")]
mod batch;
#[cfg(all(feature = "client", feature = "swap"))]
mod swap;
#[cfg(feature = "updates")]
pub(crate) mod update;
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
#[cfg(feature = "lvm")]
pub use calibrate::{CalibrateArgs, calibrate};
#[cfg(all(feature = "lvm", feature = "node"))]
pub use test::{TestArgs, test};
#[cfg(feature = "client")]
pub use claim::{ClaimArgs, claim};
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
#[cfg(feature = "updates")]
pub use update::{ReleaseArgs, UpdateArgs, release, update};

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
    /// Seal the key file under a passphrase, asked twice and not shown, or read from the file
    /// CHAINV4_KEY_PASSPHRASE_FILE names: everything that reads the key then asks for it.
    #[cfg(feature = "sealed-keys")]
    #[arg(long)]
    seal: bool,
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

    #[cfg(feature = "sealed-keys")]
    let line = match args.seal {
        true => zeroize::Zeroizing::new(crate::crypto::sealed::seal_key_file(&keypair, &new_passphrase(&args.out, input)?)?),
        false => zeroize::Zeroizing::new(keypair.to_key_file()),
    };
    #[cfg(not(feature = "sealed-keys"))]
    let line = zeroize::Zeroizing::new(keypair.to_key_file());

    write_key_line(&args.out, &line)?;
    println!("wrote {}{}", args.out.display(), if crate::cli::sealed(&line) { ", sealed" } else { "" });

    for (label, value) in described(&keypair) {
        println!("{label} {value}");
    }

    Ok(())
}

#[derive(clap::Args)]
pub struct KeyArgs {
    #[command(subcommand)]
    command: KeyCommand,
}

#[derive(clap::Subcommand)]
enum KeyCommand {
    /// Prints each key file's scheme, public key and address, and its EVM or testnet address where
    /// it has one: what a federation, a multisig or a genesis names it by. Never the secret.
    Show {
        /// The key files.
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
    /// Seals a plain key file under a passphrase, in place: asked twice and not shown, or read from
    /// the file CHAINV4_KEY_PASSPHRASE_FILE names.
    #[cfg(feature = "sealed-keys")]
    Encrypt {
        /// The key file.
        file: PathBuf,
    },
    /// Opens a sealed key file with its passphrase and writes it back plain, in place.
    #[cfg(feature = "sealed-keys")]
    Decrypt {
        /// The key file.
        file: PathBuf,
    },
}

/// What a key file holds, shown without its secret.
pub fn key(args: KeyArgs) -> CliResult<()> {
    match args.command {
        KeyCommand::Show { files } => {
            for (at, path) in files.iter().enumerate() {
                if files.len() > 1 {
                    if at > 0 {
                        println!();
                    }
                    println!("{}", path.display());
                }

                for (label, value) in described(&read_key(path)?) {
                    println!("{label} {value}");
                }

                if sealed(&read_secret(path, "the key file")?) {
                    println!("sealed yes");
                }
            }

            Ok(())
        }
        #[cfg(feature = "sealed-keys")]
        KeyCommand::Encrypt { file } => {
            let text = read_secret(&file, "the key file")?;

            if sealed(&text) {
                return Err(format!("{} is sealed already", file.display()).into());
            }

            let keypair = Keypair::from_key_file(&text).map_err(|err| format!("{}: {err}", file.display()))?;
            let line = zeroize::Zeroizing::new(crate::crypto::sealed::seal_key_file(&keypair, &new_passphrase(&file, &mut std::io::stdin().lock())?)?);
            replace_key(&file, &line)?;
            println!("sealed {}", file.display());

            Ok(())
        }
        #[cfg(feature = "sealed-keys")]
        KeyCommand::Decrypt { file } => {
            let text = read_secret(&file, "the key file")?;

            if !sealed(&text) {
                return Err(format!("{} is not sealed", file.display()).into());
            }

            let keypair = read_key(&file)?;
            replace_key(&file, &zeroize::Zeroizing::new(keypair.to_key_file()))?;
            println!("wrote {} plain", file.display());

            Ok(())
        }
    }
}

/// A key as others name it, never its secret, a label and a value a line: its scheme; its public
/// key, in hex, as a federation or multisig member lists it (secp256k1 and bitcoin keys
/// compressed); its address here; and a secp256k1 key's address on EVM chains, or a bitcoin key's
/// on Bitcoin's test networks.
fn described(keypair: &Keypair) -> Vec<(&'static str, String)> {
    let public = keypair.public();
    let address = public.address();
    let shown = public.to_string();
    let mut lines = vec![
        ("scheme", public.key_type().name().to_string()),
        ("public", shown.rsplit(':').next().unwrap_or(&shown).to_string()),
        ("address", address.to_string()),
    ];

    match address {
        crate::structs::address::Address::Secp256k1(bytes) => lines.push(("evm", crate::structs::hex::to_hex(&bytes))),
        crate::structs::address::Address::Bitcoin(bytes) => lines.push(("testnet", crate::structs::bech32::encode_segwit("tb", &bytes))),
        _ => {}
    }

    lines
}

/// The key to sign with: a key file, or the development key when there is none. A sealed one
/// opens with the passphrase in the file CHAINV4_KEY_PASSPHRASE_FILE names, or asked for.
pub fn signer(key: Option<&Path>) -> CliResult<Keypair> {
    signer_with(key, None)
}

/// signer, with a sealed key's passphrase in passphrase_file before the environment's: what a
/// node's --key-passphrase-file names.
pub fn signer_with(key: Option<&Path>, passphrase_file: Option<&Path>) -> CliResult<Keypair> {
    match key {
        Some(path) => read_key_with(path, passphrase_file),
        None => Ok(DevAuthority::dev_keypair()),
    }
}

#[doc = "The environment variable naming a file that holds the passphrase sealed key files open with, for a node, a service or a script: the file alone, never the passphrase itself, which would show among the process's environment."]
pub const PASSPHRASE_FILE_ENV: &str = "CHAINV4_KEY_PASSPHRASE_FILE";

/// A file holding a secret, read into room made to its size first, so the string never grows and
/// leaves an unwiped copy behind; wiped as it drops.
pub(crate) fn read_secret(path: &Path, what: &str) -> CliResult<zeroize::Zeroizing<String>> {
    use std::io::Read;

    let mut file = fs::File::open(path).map_err(|err| format!("cannot read {what} {}: {err}", path.display()))?;
    let size = file.metadata().map_or(0, |meta| meta.len() as usize);
    let mut text = zeroize::Zeroizing::new(String::with_capacity(size + 1));
    file.read_to_string(&mut text).map_err(|err| format!("cannot read {what} {}: {err}", path.display()))?;

    Ok(text)
}

/// Whether a key file's text is sealed under a passphrase.
pub(crate) fn sealed(text: &str) -> bool {
    text.trim_start().starts_with("chainv4-sealed-key:")
}

/// A key from a key file's text, read from path: a sealed one opened with its passphrase.
pub(crate) fn key_from_text(text: &str, path: &Path, passphrase_file: Option<&Path>) -> CliResult<Keypair> {
    #[cfg(feature = "sealed-keys")]
    if sealed(text) {
        let passphrase = passphrase_for(path, passphrase_file, &mut std::io::stdin().lock())?;

        return Ok(crate::crypto::sealed::open_key_file(text, &passphrase).map_err(|err| format!("{}: {err}", path.display()))?);
    }

    #[cfg(not(feature = "sealed-keys"))]
    if sealed(text) {
        let _ = passphrase_file;

        return Err(format!("{} is sealed, and this build was made without sealed-keys", path.display()).into());
    }

    Ok(Keypair::from_key_file(text).map_err(|err| format!("{}: {err}", path.display()))?)
}

/// The passphrase a sealed key file at path opens with: from passphrase_file, else the file the
/// environment names, else asked on a terminal, not shown.
#[cfg(feature = "sealed-keys")]
fn passphrase_for(path: &Path, passphrase_file: Option<&Path>, input: &mut dyn std::io::BufRead) -> CliResult<zeroize::Zeroizing<String>> {
    let named = passphrase_file.map(Path::to_path_buf).or_else(|| std::env::var_os(PASSPHRASE_FILE_ENV).map(PathBuf::from));

    match named {
        Some(file) => passphrase_in(&file),
        None if std::io::IsTerminal::is_terminal(&std::io::stdin()) => secret_line(&format!("the passphrase for {}", path.display()), input),
        None => Err(format!("{} is sealed: name a file holding its passphrase with --key-passphrase-file or {PASSPHRASE_FILE_ENV}, or run it on a terminal to be asked", path.display()).into()),
    }
}

/// A passphrase kept in a file: its first line, without the line's end.
#[cfg(feature = "sealed-keys")]
fn passphrase_in(file: &Path) -> CliResult<zeroize::Zeroizing<String>> {
    let text = read_secret(file, "the passphrase file")?;
    let line = text.lines().next().unwrap_or_default();

    if line.is_empty() {
        return Err(format!("the passphrase file {} is empty", file.display()).into());
    }

    Ok(zeroize::Zeroizing::new(line.to_string()))
}

/// A new passphrase to seal a key file at path with: from the file the environment names, else
/// asked twice, not shown, the two the same and not empty.
#[cfg(feature = "sealed-keys")]
fn new_passphrase(path: &Path, input: &mut dyn std::io::BufRead) -> CliResult<zeroize::Zeroizing<String>> {
    if let Some(file) = std::env::var_os(PASSPHRASE_FILE_ENV) {
        return passphrase_in(Path::new(&file));
    }

    let first = secret_line(&format!("a passphrase to seal {} with", path.display()), input)?;
    let again = secret_line("the same again", input)?;

    if first.is_empty() {
        return Err("no passphrase: leave the key file plain instead".into());
    }

    if *first != *again {
        return Err("the two passphrases differ: nothing was written".into());
    }

    Ok(first)
}

/// A line of input, asked for on standard error and, on a terminal, not shown as it is typed.
#[cfg(feature = "sealed-keys")]
fn secret_line(what: &str, input: &mut dyn std::io::BufRead) -> CliResult<zeroize::Zeroizing<String>> {
    eprint!("{what}: ");
    std::io::stderr().flush()?;
    let hidden = Hidden::start();
    // Room for any passphrase made up front, so reading one never grows the string and leaves a
    // copy of it behind.
    let mut line = zeroize::Zeroizing::new(String::with_capacity(4096));
    input.read_line(&mut line)?;
    drop(hidden);

    if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        eprintln!();
    }

    let kept = line.trim_end_matches(['\n', '\r']).len();
    line.truncate(kept);

    Ok(line)
}

/// The terminal's echo, off while it lives, on a Unix terminal; nothing elsewhere.
#[cfg(feature = "sealed-keys")]
struct Hidden {
    #[cfg(unix)]
    was: Option<libc::termios>,
}

#[cfg(feature = "sealed-keys")]
impl Hidden {
    fn start() -> Hidden {
        #[cfg(unix)]
        {
            let mut was = std::mem::MaybeUninit::<libc::termios>::uninit();

            // SAFETY: tcgetattr fills the struct when it returns 0, and is only asked of a
            // terminal; tcsetattr takes the copy it gave, with echo off.
            unsafe {
                if libc::isatty(libc::STDIN_FILENO) == 1 && libc::tcgetattr(libc::STDIN_FILENO, was.as_mut_ptr()) == 0 {
                    let was = was.assume_init();
                    let mut quiet = was;
                    quiet.c_lflag &= !libc::ECHO;
                    quiet.c_lflag |= libc::ECHONL;

                    if libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &quiet) == 0 {
                        return Hidden { was: Some(was) };
                    }
                }
            }

            Hidden { was: None }
        }

        #[cfg(not(unix))]
        Hidden {}
    }
}

#[cfg(feature = "sealed-keys")]
impl Drop for Hidden {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(was) = &self.was {
            // SAFETY: puts back the settings tcgetattr gave.
            unsafe {
                libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, was);
            }
        }
    }
}

/// Writes a key file's line where one stands, readable by its owner only: beside it first, then
/// renamed over, so a failure leaves the old one whole.
#[cfg(feature = "sealed-keys")]
fn replace_key(path: &Path, line: &str) -> CliResult<()> {
    let beside = path.with_extension("key.new");
    let _ = fs::remove_file(&beside);
    write_key_line(&beside, line)?;
    fs::rename(&beside, path).map_err(|err| format!("cannot put {} in place: {err}", path.display()))?;

    Ok(())
}

/// A key file holds one line: the scheme, a colon and the 32-byte seed in hex, like an address;
/// or, sealed, chainv4-sealed-key and what opens it (crate::crypto::sealed).
fn write_key_line(path: &Path, line: &str) -> CliResult<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);

    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);

    let mut file = options
        .open(path)
        .map_err(|err| format!("cannot create {}: {err}", path.display()))?;
    writeln!(file, "{line}")?;

    Ok(())
}

/// A plain key file for this key, as write_key_line writes one.
#[cfg_attr(not(test), allow(dead_code))]
fn write_key(path: &Path, keypair: &Keypair) -> CliResult<()> {
    write_key_line(path, &zeroize::Zeroizing::new(keypair.to_key_file()))
}

fn read_key(path: &Path) -> CliResult<Keypair> {
    read_key_with(path, None)
}

pub(crate) fn read_key_with(path: &Path, passphrase_file: Option<&Path>) -> CliResult<Keypair> {
    let text = read_secret(path, "the key file")?;

    key_from_text(&text, path, passphrase_file)
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

/// A whole, non-negative amount in decimal, its digits grouped with `_` if wanted: 1_000_000.
pub fn parse_amount(text: &str) -> Result<u256, String> {
    let bad = || format!("{text} is not a whole amount");

    u256::from_str_radix(&ungrouped(text).ok_or_else(bad)?, 10).map_err(|_| bad())
}

/// An amount in whole coins, as people write one, such as 10 or 0.5: in the coin's smallest units,
/// at its decimals, grouped with `_` if wanted: 1_000.5. More decimal places than the coin has are
/// refused rather than rounded.
pub fn parse_coins(text: &str, decimals: u8) -> Result<u256, String> {
    let bad = || format!("{text} is not an amount of coins, such as 10 or 0.5");
    let plain = ungrouped(text.trim()).ok_or_else(bad)?;
    let (whole, fraction) = plain.split_once('.').unwrap_or((&plain, ""));

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
    let plain = ungrouped(text.trim()).ok_or_else(bad)?;
    let text = plain.as_ref();
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
