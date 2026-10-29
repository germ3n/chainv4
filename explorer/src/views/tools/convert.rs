//! Convert: amounts between the smallest unit and the coin, at the chain's decimals; an address
//! and its EVM alias, both ways for a secp256k1 account, whose alias is its own 20 bytes; a public
//! key and the address it controls under each scheme it fits; and bech32 and the witness program
//! it writes, both ways.

use {
    crate::views::{self, Ctx},
    chainv4::abi::evm_address,
    chainv4::crypto::hasher::Hasher,
    chainv4::crypto::keypair::PublicKey,
    chainv4::crypto::verifiable::Verifiable,
    chainv4::structs::address::Address,
    chainv4::structs::bech32,
    chainv4::structs::hash::HashType,
    chainv4::structs::hex::{from_hex, to_hex},
    egui::{RichText, Ui},
};

#[derive(Default)]
pub struct ConvertState {
    pub smallest: String,
    pub whole: String,
    pub address: String,
    pub key: String,
    pub bech32: String,
    pub program: String,
    /// The human-readable part to write a program under: bc, tb or bcrt.
    pub hrp: Hrp,
}

/// Where a witness program goes: Bitcoin, its test networks, or regtest.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Hrp {
    #[default]
    Bitcoin,
    Test,
    Regtest,
}

impl Hrp {
    pub fn text(self) -> &'static str {
        match self {
            Hrp::Bitcoin => "bc",
            Hrp::Test => "tb",
            Hrp::Regtest => "bcrt",
        }
    }
}

/// Units in the smallest unit as a whole number of the coin, plainly: 1500000000000000000 at 18 is
/// 1.5, with no grouping, to copy.
pub fn plain_units(raw: &str, decimals: u8) -> String {
    let digits = raw.trim_start_matches('0');
    let decimals = usize::from(decimals);
    let padded = format!("{digits:0>width$}", width = decimals + 1);
    let (whole, fraction) = padded.split_at(padded.len() - decimals);
    let fraction = fraction.trim_end_matches('0');

    match fraction.is_empty() {
        true => whole.to_string(),
        false => format!("{whole}.{fraction}"),
    }
}

/// The smallest unit to the coin.
pub fn to_whole(raw: &str, decimals: u8) -> Result<String, String> {
    let raw = raw.trim().replace([',', '_'], "");

    match !raw.is_empty() && raw.chars().all(|digit| digit.is_ascii_digit()) {
        true => Ok(plain_units(&raw, decimals)),
        false => Err("the smallest unit is a whole number: digits alone".to_string()),
    }
}

/// The coin to the smallest unit.
pub fn to_smallest(text: &str, decimals: u8) -> Result<String, String> {
    crate::format::parse_units(text, decimals).ok_or_else(|| format!("not an amount with at most {decimals} decimals"))
}

/// An EVM address with EIP-55's checksum in its letters' case, as Ethereum's wallets write it.
pub fn checksummed(bytes: &[u8; 20]) -> String {
    let hex = to_hex(bytes)[2..].to_string();
    let hash = Hasher::digest(HashType::Keccak256, hex.as_bytes()).to_bytes();
    let letters: String = hex
        .chars()
        .enumerate()
        .map(|(at, letter)| {
            let nibble = (hash[at / 2] >> if at % 2 == 0 { 4 } else { 0 }) & 0xf;

            match nibble >= 8 {
                true => letter.to_ascii_uppercase(),
                false => letter,
            }
        })
        .collect();

    format!("0x{letters}")
}

/// What an address goes by inside the EVM.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Alias {
    /// Its 20 bytes, in lowercase hex.
    pub evm: String,
    pub checksummed: String,
    /// The chainv4 accounts these 20 bytes are: the secp256k1 account and the EVM contract they
    /// name. Empty for an alias that is a hash of an account's key, which reads back to nothing.
    pub accounts: Vec<String>,
}

/// An address and its EVM alias, either way: a chainv4 address to the 20 bytes the EVM knows it by,
/// or 20 bytes, 0x…, to the accounts they are.
pub fn alias(text: &str) -> Result<Alias, String> {
    let text = text.trim();
    let same = |bytes: [u8; 20]| Alias {
        evm: to_hex(&bytes),
        checksummed: checksummed(&bytes),
        accounts: vec![Address::Secp256k1(bytes).to_string(), Address::Evm(bytes).to_string()],
    };

    if !text.contains(':') && text.starts_with("0x") {
        let bytes: [u8; 20] = from_hex(text).and_then(|bytes| bytes.try_into().ok()).ok_or("an EVM address is 0x and 40 hex digits")?;

        return Ok(same(bytes));
    }

    let address: Address = text.parse().map_err(|_| "not an address: kind:0x…, such as secp256k1:0x…, bc1q…, or an EVM 0x…".to_string())?;
    let bytes = evm_address(&address);

    match address {
        Address::Secp256k1(_) | Address::Evm(_) => Ok(same(bytes)),
        _ => Ok(Alias { evm: to_hex(&bytes), checksummed: checksummed(&bytes), accounts: Vec::new() }),
    }
}

/// A scheme's name, and the address a key controls under it, or why it is no key of it.
pub type SchemeAddress = (&'static str, Result<String, String>);

/// The address a public key controls under each scheme it fits: an Edwards key under ed25519 and
/// edblake, a compressed secp256k1 key under secp256k1, Ethereum's, and bitcoin, bc1q…. A key with
/// its scheme in front, as ed25519:0x…, is that scheme's alone.
pub fn key_addresses(text: &str) -> Result<Vec<SchemeAddress>, String> {
    let text = text.trim();
    let keys: Vec<PublicKey> = match text.contains(':') {
        true => vec![text.parse::<PublicKey>().map_err(|_| "not a key: scheme:0x…, such as ed25519:0x… or secp256k1:0x and 33 bytes".to_string())?],
        false => match from_hex(text).ok_or("a key is hex: 0x…")?.as_slice() {
            bytes if bytes.len() == 32 => {
                let key: [u8; 32] = bytes.try_into().expect("32 bytes");

                vec![PublicKey::Ed25519(key), PublicKey::EdBlake(key)]
            }
            bytes if bytes.len() == 33 => {
                let key: [u8; 33] = bytes.try_into().expect("33 bytes");

                vec![PublicKey::Secp256k1(key), PublicKey::Bitcoin(key)]
            }
            bytes => return Err(format!("a key of {} bytes: Edwards keys are 32, ed25519's and edblake's; secp256k1's and bitcoin's 33, compressed", bytes.len())),
        },
    };

    Ok(keys
        .into_iter()
        .map(|key| {
            let address = match key.verify() {
                Ok(()) => Ok(key.address().to_string()),
                Err(_) => Err("not a key of this scheme: not a point on its curve".to_string()),
            };

            (key.key_type().name(), address)
        })
        .collect())
}

/// A segwit address read: its network's part, its kind, and its witness program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Witness {
    pub hrp: String,
    /// P2WPKH for 20 bytes, a key's hash; P2WSH for 32, a script's.
    pub kind: &'static str,
    pub program: String,
}

/// Bech32 to its witness program: version 0 alone, bc1q… and its test networks'.
pub fn from_bech32(text: &str) -> Result<Witness, String> {
    let (hrp, program) = bech32::decode_segwit(text.trim()).ok_or("not a version 0 segwit address, bc1q… or tb1q…: a bad checksum, mixed case, or another version")?;
    let kind = match program.len() {
        20 => "P2WPKH",
        _ => "P2WSH",
    };

    Ok(Witness { hrp, kind, program: to_hex(&program) })
}

/// A witness program to bech32, as version 0 under a network's part.
pub fn to_bech32(hrp: &str, hex: &str) -> Result<String, String> {
    let program = from_hex(hex.trim()).ok_or("a witness program is hex: 0x…")?;

    match program.len() {
        20 | 32 => Ok(bech32::encode_segwit(hrp, &program)),
        other => Err(format!("a version 0 program is 20 bytes, a key's hash, or 32, a script's; not {other}")),
    }
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx, state: &mut ConvertState) {
    let look = ctx.look;
    let width = super::width(ctx);
    let (decimals, symbol) = ctx.coin();
    let symbol = if symbol.is_empty() { "the coin".to_string() } else { symbol };

    views::section(ui, look, "Units", |ui| {
        views::note(ui, look, format!("At the chain's {decimals} decimals, from its status: one {symbol} is 1 and {decimals} zeros of the smallest unit."));
        ui.add_space(4.0);
        views::facts(ui, look, "units", |facts| {
            facts.row("smallest unit", |ui| {
                ui.add(egui::TextEdit::singleline(&mut state.smallest).hint_text("1500000000000000000").desired_width(width));
            });

            if !state.smallest.trim().is_empty() {
                let result = to_whole(&state.smallest, decimals).map(|whole| format!("{whole} {symbol}"));
                facts.row("is", |ui| super::outcome(ui, ctx, &result));
            }

            facts.row(&symbol, |ui| {
                ui.add(egui::TextEdit::singleline(&mut state.whole).hint_text("1.5").desired_width(width));
            });

            if !state.whole.trim().is_empty() {
                let result = to_smallest(&state.whole, decimals);
                facts.row("is", |ui| super::outcome(ui, ctx, &result));
            }
        });
    });

    views::section(ui, look, "Address and EVM alias", |ui| {
        views::note(ui, look, "A secp256k1 account is its EVM address: the same 20 bytes, either way. Any other account's alias is a hash of its key, which reads back to nothing.");
        ui.add_space(4.0);
        ui.add(egui::TextEdit::singleline(&mut state.address).hint_text("secp256k1:0x…, ed25519:0x…, bc1q…, or an EVM 0x…").desired_width(width));

        if !state.address.trim().is_empty() {
            match alias(&state.address) {
                Ok(found) => views::facts(ui, look, "alias", |facts| {
                    facts.row("EVM address", |ui| views::copyable(ui, &found.evm));
                    facts.row("checksummed", |ui| views::copyable(ui, &found.checksummed));

                    match found.accounts.is_empty() {
                        true => facts.text("back", "a hash: nothing reads it back to the account"),
                        false => facts.row("chainv4 account", |ui| {
                            ui.vertical(|ui| {
                                for account in &found.accounts {
                                    views::address(ui, ctx, account);
                                }
                            });
                        }),
                    }
                }),
                Err(why) => super::outcome(ui, ctx, &Err(why)),
            }
        }
    });

    views::section(ui, look, "Public key to address", |ui| {
        ui.add(egui::TextEdit::singleline(&mut state.key).hint_text("ed25519:0x…, secp256k1:0x…, or a key's hex alone").desired_width(width));

        if !state.key.trim().is_empty() {
            match key_addresses(&state.key) {
                Ok(found) => views::facts(ui, look, "key addresses", |facts| {
                    for (scheme, address) in &found {
                        facts.row(scheme, |ui| match address {
                            Ok(address) => views::address(ui, ctx, address),
                            Err(why) => {
                                ui.label(RichText::new(why).color(look.palette.muted));
                            }
                        });
                    }
                }),
                Err(why) => super::outcome(ui, ctx, &Err(why)),
            }
        }
    });

    views::section(ui, look, "Bech32 and hex", |ui| {
        views::facts(ui, look, "bech32", |facts| {
            facts.row("bech32", |ui| {
                ui.add(egui::TextEdit::singleline(&mut state.bech32).hint_text("bc1q… or tb1q…").desired_width(width));
            });

            if !state.bech32.trim().is_empty() {
                match from_bech32(&state.bech32) {
                    Ok(witness) => {
                        facts.text("network", format!("{} · {}", witness.hrp, witness.kind));
                        facts.row("program", |ui| views::copyable(ui, &witness.program));
                    }
                    Err(why) => facts.row("is", |ui| super::outcome(ui, ctx, &Err(why))),
                }
            }

            facts.row("program", |ui| {
                ui.add(egui::TextEdit::singleline(&mut state.program).hint_text("0x and 40 or 64 hex digits").desired_width(width));
            });
            facts.row("under", |ui| {
                for hrp in [Hrp::Bitcoin, Hrp::Test, Hrp::Regtest] {
                    ui.selectable_value(&mut state.hrp, hrp, hrp.text());
                }
            });

            if !state.program.trim().is_empty() {
                let result = to_bech32(state.hrp.text(), &state.program);
                facts.row("is", |ui| super::outcome(ui, ctx, &result));
            }
        });
    });
}
