//! Hash & verify: text or hex hashed with every hash this build has, chainv4's own and Bitcoin's
//! double SHA-256 and HASH160; a signature checked against a message and a public key, under any
//! scheme the build has; and an EVM function's selector and an event's topic from its signature.

use {
    crate::views::{self, Ctx},
    chainv4::abi::{AbiType, parse_signature},
    chainv4::crypto::bitcoin::{hash160, sha256d},
    chainv4::crypto::hasher::Hasher,
    chainv4::crypto::keypair::{PublicKey, Signature},
    chainv4::crypto::verifiable::Verifiable,
    chainv4::structs::hash::HashType,
    chainv4::structs::hex::{from_hex, to_hex},
    chainv4_ui::widgets,
    egui::{RichText, Ui},
};

#[derive(Default)]
pub struct HashState {
    pub input: String,
    /// Whether the input is hex rather than text.
    pub hex: bool,
    pub key: String,
    pub message: String,
    pub message_hex: bool,
    pub signature: String,
    /// An EVM function's or event's signature, such as transfer(address,uint256).
    pub evm: String,
}

/// The bytes typed: the text's UTF-8, or the hex's bytes.
pub fn bytes_of(text: &str, hex: bool) -> Result<Vec<u8>, String> {
    match hex {
        true => from_hex(text.trim()).ok_or_else(|| "not hex: 0x and pairs of hex digits".to_string()),
        false => Ok(text.as_bytes().to_vec()),
    }
}

/// Every digest of these bytes: by each hash the build has, by name, then Bitcoin's double SHA-256
/// and HASH160.
pub fn digests(bytes: &[u8]) -> Vec<(&'static str, String)> {
    HashType::ALL
        .iter()
        .map(|algorithm| (algorithm.name(), to_hex(&Hasher::digest(*algorithm, bytes).to_bytes())))
        .chain([("sha256d", to_hex(&sha256d(bytes))), ("hash160", to_hex(&hash160(bytes)))])
        .collect()
}

/// What a scheme signs of a message, for the page to say.
pub fn signs(key: &PublicKey) -> &'static str {
    match key {
        PublicKey::Ed25519(_) | PublicKey::EdBlake(_) => "the message itself",
        PublicKey::Secp256k1(_) => "the message's keccak256",
        PublicKey::Bitcoin(_) => "the message's double SHA-256",
    }
}

/// Checks a signature, 64 bytes of hex, over a message, by a public key, scheme:0x….
pub fn check_signature(key: &str, message: &[u8], signature: &str) -> Result<(), String> {
    let key: PublicKey = key.trim().parse().map_err(|_| "not a key: scheme:0x…, such as ed25519:0x… or secp256k1:0x and 33 bytes".to_string())?;
    key.verify().map_err(|_| "not a key of its scheme: not a point on its curve".to_string())?;
    let bytes: [u8; 64] = from_hex(signature.trim())
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or("a signature is 64 bytes of hex: r, then s")?;
    let signature = match key {
        PublicKey::Ed25519(_) => Signature::Ed25519(bytes),
        PublicKey::EdBlake(_) => Signature::EdBlake(bytes),
        PublicKey::Secp256k1(_) => Signature::Secp256k1(bytes),
        PublicKey::Bitcoin(_) => Signature::Bitcoin(bytes),
    };

    key.verify_signature(message, &signature).map_err(|_| "it does not check: this key did not sign this message".to_string())
}

/// An EVM function's or event's signature, as hashed, and what it hashes to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selector {
    /// The signature as Solidity hashes it: name(types), no spaces or names.
    pub canonical: String,
    /// A function's selector: the first four bytes of its keccak256.
    pub selector: String,
    /// An event's topic: all 32.
    pub topic: String,
}

/// The selector and topic of a signature, such as transfer(address,uint256).
pub fn selector(text: &str) -> Result<Selector, String> {
    let (Some(selector), types) = parse_signature(text)? else {
        return Err("a signature has a name before its types: transfer(address,uint256)".to_string());
    };
    let name = text.trim().split('(').next().unwrap_or_default().trim();
    let canonical = format!("{name}{}", AbiType::Tuple(types));

    Ok(Selector {
        topic: to_hex(&Hasher::digest(HashType::Keccak256, canonical.as_bytes()).to_bytes()),
        selector: to_hex(&selector),
        canonical,
    })
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx, state: &mut HashState) {
    let look = ctx.look;
    let width = super::width(ctx);

    views::section(ui, look, "Hashes", |ui| {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut state.hex, false, "Text");
            ui.selectable_value(&mut state.hex, true, "Hex");
        });
        ui.add(egui::TextEdit::multiline(&mut state.input).hint_text(if state.hex { "0x…" } else { "Text to hash" }).desired_rows(2).desired_width(width));

        match bytes_of(&state.input, state.hex) {
            Ok(bytes) => {
                views::note(ui, look, format!("{} bytes", bytes.len()));
                views::facts(ui, look, "digests", |facts| {
                    for (name, digest) in digests(&bytes) {
                        facts.row(name, |ui| views::copyable(ui, &digest));
                    }
                });
            }
            Err(why) => super::outcome(ui, ctx, &Err(why)),
        }
    });

    views::section(ui, look, "Check a signature", |ui| {
        views::facts(ui, look, "signature check", |facts| {
            facts.row("public key", |ui| {
                ui.add(egui::TextEdit::singleline(&mut state.key).hint_text("ed25519:0x…, edblake:0x…, secp256k1:0x…, bitcoin:0x…").desired_width(width));
            });
            facts.row("message", |ui| {
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut state.message_hex, false, "Text");
                        ui.selectable_value(&mut state.message_hex, true, "Hex");
                    });
                    ui.add(egui::TextEdit::multiline(&mut state.message).desired_rows(2).desired_width(width));
                });
            });
            facts.row("signature", |ui| {
                ui.add(egui::TextEdit::singleline(&mut state.signature).hint_text("0x and 128 hex digits").desired_width(width));
            });

            if !state.key.trim().is_empty() && !state.signature.trim().is_empty() {
                let checked = bytes_of(&state.message, state.message_hex).and_then(|message| check_signature(&state.key, &message, &state.signature));
                let signed = state.key.trim().parse::<PublicKey>().ok().map(|key| signs(&key));

                facts.row("result", |ui| match checked {
                    Ok(()) => {
                        widgets::pill(ui, "checks", look.palette.good);

                        if let Some(signed) = signed {
                            ui.label(RichText::new(format!("this key signed {signed}")).color(look.palette.soft));
                        }
                    }
                    Err(why) => {
                        widgets::pill(ui, "does not check", look.palette.bad);
                        ui.label(RichText::new(why).color(look.palette.soft));
                    }
                });
            }
        });
    });

    views::section(ui, look, "EVM selector and topic", |ui| {
        ui.add(egui::TextEdit::singleline(&mut state.evm).hint_text("transfer(address,uint256) or Transfer(address,address,uint256)").desired_width(width));

        if !state.evm.trim().is_empty() {
            match selector(&state.evm) {
                Ok(found) => views::facts(ui, look, "selector", |facts| {
                    facts.row("hashed as", |ui| views::copyable(ui, &found.canonical));
                    facts.row("selector", |ui| views::copyable(ui, &found.selector));
                    facts.row("topic", |ui| views::copyable(ui, &found.topic));
                }),
                Err(why) => super::outcome(ui, ctx, &Err(why)),
            }
        }
    });
}
