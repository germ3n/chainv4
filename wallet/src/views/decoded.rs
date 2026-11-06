//! EVM calls and logs as what they say, for Activity and for the screens someone signs on. What
//! reads them is chainv4::abi::known, which reads each thing once however many frames show it, and
//! has cut what it read short and escaped it.
//!
//! What is read is untrusted: a site, or whoever wrote the contract, chose it. So it adds to a
//! screen's facts and never stands in for them: the To row, the value and the fact that there is
//! data come first and stay as they are, and what was read goes below them in a frame of its own,
//! the height it may take held down, in lines that look like no row of the card. A read that fails
//! leaves the facts alone.

use {
    crate::app::{SavedContract, Settings},
    crate::format,
    crate::theme::{self, Palette},
    chainv4::abi::known::{NOT_PROOF, Piece, ReadArg, ReadCall, ReadLog, TokenMove},
    chainv4::structs::hex::to_hex,
    egui::{RichText, Ui},
};

/// The most a frame of what was read takes on a screen: the fields below it must stay in reach.
const MAX_HEIGHT: f32 = 180.0;
/// How much raw data shows, past which it is there to copy.
const MAX_RAW: usize = 4_096;

/// The ABI saved for a contract, by its address as the chain writes it.
pub fn kept<'a>(contracts: &'a [SavedContract], address: &str) -> Option<&'a str> {
    contracts.iter().find(|kept| kept.address.eq_ignore_ascii_case(address)).map(|kept| kept.abi.as_str()).filter(|abi| !abi.trim().is_empty())
}

/// An EVM address's own, for the ABI saved for it.
pub fn kept_for<'a>(settings: &'a Settings, address: &[u8; 20]) -> Option<&'a str> {
    kept(&settings.contracts, &format!("evm:{}", to_hex(address)))
}

/// A token's symbol and decimals, for its amounts: only if both are known.
pub type TokenUnits<'a> = Option<(&'a str, u8)>;

/// A whole amount of the token called: in its decimals where they are known, and otherwise as the
/// plain whole number it is.
fn amount(ui: &mut Ui, palette: &Palette, token: TokenUnits, raw: &str) {
    match token {
        Some((symbol, decimals)) => {
            ui.label(RichText::new(format::amount(raw, decimals, symbol)).color(palette.ink)).on_hover_text(format!("{raw} whole units"));
        }
        None => {
            ui.label(theme::mono(raw).color(palette.ink)).on_hover_text("A whole number: no decimals are known for this contract");
        }
    }
}

fn arguments(ui: &mut Ui, palette: &Palette, token: TokenUnits, args: &[ReadArg]) {
    for arg in args {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;

            if !arg.name.is_empty() {
                ui.label(theme::mono(format!("{} =", arg.name)).color(palette.muted)).on_hover_text(&arg.ty);
            }

            for piece in arg.pieces() {
                match piece {
                    Piece::Text(text) => {
                        ui.add(egui::Label::new(theme::mono(text).color(palette.ink)).wrap());
                    }
                    Piece::Address(bytes) => super::address(ui, palette, &to_hex(&bytes)),
                    Piece::Amount(raw) => amount(ui, palette, token, &raw.to_string()),
                }
            }
        });

        // A call its bytes hold: to another contract, so its amounts stay plain.
        if let Some(inner) = &arg.inner {
            ui.indent(("held call", &arg.name), |ui| lines(ui, palette, None, inner));
        }
    }
}

/// A call's lines: its function, its arguments, and which ABI read it.
fn lines(ui: &mut Ui, palette: &Palette, token: TokenUnits, read: &ReadCall) {
    ui.add(egui::Label::new(theme::mono(&read.name).color(palette.ink)).wrap()).on_hover_text(&read.signature);
    arguments(ui, palette, token, &read.args);
    ui.add(egui::Label::new(RichText::new(read.by.label()).small().color(palette.muted)).wrap());

    if !read.exact {
        ui.add(egui::Label::new(RichText::new("Its data holds more than this reads.").small().color(palette.warn)).wrap());
    }
}

/// What was read, framed, and however much it is, taking no more of the screen than it may.
fn frame(ui: &mut Ui, palette: &Palette, salt: &str, add: impl FnOnce(&mut Ui)) {
    egui::Frame::new().fill(palette.row).corner_radius(8.0).inner_margin(10).show(ui, |ui| {
        ui.set_width(ui.available_width());
        egui::ScrollArea::vertical().id_salt(("decoded", salt)).max_height(MAX_HEIGHT).show(ui, add);
    });
}

/// A call read, in a frame, for a screen someone signs on: which ABI read it, and that a match
/// is all a built-in one is.
pub fn signing(ui: &mut Ui, palette: &Palette, token: TokenUnits, read: &ReadCall) {
    frame(ui, palette, "signing", |ui| {
        lines(ui, palette, token, read);

        if read.by.only_fits() {
            ui.add(egui::Label::new(RichText::new(NOT_PROOF).small().color(palette.warn)).wrap());
        }
    });
}

/// A call read, in a frame, for what already happened.
pub fn call(ui: &mut Ui, palette: &Palette, token: TokenUnits, read: &ReadCall) {
    frame(ui, palette, "call", |ui| lines(ui, palette, token, read));
}

/// A token moving, as a log claims: who, to whom, and how much.
fn token_move(ui: &mut Ui, palette: &Palette, token: TokenUnits, moved: &TokenMove) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;

        match &moved.from {
            Some(from) => super::address(ui, palette, &to_hex(from)),
            None => {
                ui.label(RichText::new("minted").color(palette.muted));
            }
        }

        ui.label(RichText::new("to").color(palette.muted));

        match &moved.to {
            Some(to) => super::address(ui, palette, &to_hex(to)),
            None => {
                ui.label(RichText::new("burned").color(palette.muted));
            }
        }

        match &moved.id {
            Some(id) if moved.amount == chainv4::structs::u256::u256::ONE => {
                ui.add(egui::Label::new(theme::mono(format!("token {id}")).color(palette.ink)).wrap());
            }
            Some(id) => {
                ui.add(egui::Label::new(theme::mono(format!("{} of token {id}", moved.amount)).color(palette.ink)).wrap());
            }
            None => amount(ui, palette, token, &moved.amount.to_string()),
        }
    });
}

/// A log read, in a frame: the tokens it says moved, or its event's values, and which ABI read it.
pub fn log(ui: &mut Ui, palette: &Palette, token: TokenUnits, read: &ReadLog) {
    frame(ui, palette, "log", |ui| {
        ui.add(egui::Label::new(theme::mono(&read.name).color(palette.ink)).wrap()).on_hover_text(&read.signature);

        match read.moves.is_empty() {
            true => arguments(ui, palette, token, &read.args),
            false => {
                for moved in &read.moves {
                    token_move(ui, palette, token, moved);
                }
            }
        }

        ui.add(egui::Label::new(RichText::new(read.by.label()).small().color(palette.muted)).wrap());
    });
}

/// Raw bytes in hex, a click away, and copied whole however little of them shows.
pub fn raw(ui: &mut Ui, palette: &Palette, salt: &str, hex: &str) {
    egui::CollapsingHeader::new(RichText::new("Raw data").small().color(palette.muted)).id_salt(("raw", salt, hex.len())).show(ui, |ui| {
        super::copy(ui, palette, hex);

        let shown: String = hex.chars().take(MAX_RAW).collect();
        let cut = match hex.len() > MAX_RAW {
            true => format!("{shown}…"),
            false => shown,
        };

        egui::ScrollArea::vertical().id_salt(("raw text", salt)).max_height(MAX_HEIGHT).show(ui, |ui| super::text_block(ui, palette, &cut));
    });
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        chainv4::abi::known::known_abis,
        chainv4::abi::{AbiValue, encode_values, parse_signature},
        chainv4::structs::u256::u256,
    };

    /// Draws a screen's rows and what was read of a call, as a signing screen does: the height the
    /// read took, whether the row after it is still placed under it, and all the text drawn.
    fn drawn(read: &ReadCall) -> (f32, f32, f32, String) {
        let palette = crate::theme::DARK;
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(380.0, 900.0));
        let (mut height, mut read_bottom, mut fee_top) = (0.0, 0.0, 0.0);
        let mut text = String::new();

        for _ in 0..3 {
            let mut output = ctx.run_ui(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ui| {
                crate::views::row(ui, &palette, "To", |ui| crate::views::address(ui, &palette, "0xto"));

                let shown = ui.scope(|ui| signing(ui, &palette, None, read)).response.rect;
                (height, read_bottom) = (shown.height(), shown.bottom());
                fee_top = ui.label("Fee, at most").rect.top();
            });
            output.textures_delta.clear();
            text.clear();

            for clipped in &output.shapes {
                if let egui::epaint::Shape::Text(shape) = &clipped.shape {
                    text.push_str(shape.galley.text());
                    text.push('\n');
                }
            }
        }

        (height, read_bottom, fee_top, text)
    }

    fn input(signature: &str, values: &[AbiValue]) -> Vec<u8> {
        let (selector, types) = parse_signature(signature).unwrap();

        [selector.unwrap().to_vec(), encode_values(&types, values).unwrap()].concat()
    }

    #[test]
    fn a_signing_screen_says_a_built_in_abi_only_fits() {
        let known = known_abis();
        let read = chainv4::abi::known::read_call(None, &known, &input("transfer(address,uint256)", &[AbiValue::Address([0xab; 20]), AbiValue::Uint(u256::new(5))])).unwrap();
        let (_, _, _, text) = drawn(&read);

        for said in ["transfer", "read as ERC-20", "A match, not proof", "5"] {
            assert!(text.contains(said), "{said} in {text}");
        }
    }

    #[test]
    fn however_long_what_was_read_is_the_fields_below_stay_in_reach() {
        let known = known_abis();
        // A long string, under a NamePayer's pay.
        let long = chainv4::abi::known::read_call(None, &known, &input("pay(string)", &[AbiValue::String("W".repeat(90_000))])).unwrap();
        // A huge array, under a router's path.
        let path = vec![AbiValue::Address([1; 20]); 2_000];
        let many = chainv4::abi::known::read_call(None, &known, &input("swapExactDEVForTokens(uint256,address[],uint256[],address,uint256)", &[AbiValue::Uint(u256::ONE), AbiValue::Array(path), AbiValue::Array(vec![AbiValue::Uint(u256::new(30))]), AbiValue::Address([2; 20]), AbiValue::Uint(u256::ONE)])).unwrap();

        for read in [long, many] {
            let (height, read_bottom, fee_top, _) = drawn(&read);

            assert!(height <= MAX_HEIGHT + 24.0, "held down: {height}");
            assert!(fee_top >= read_bottom, "the row after it is below it: {fee_top} {read_bottom}");
        }
    }

    #[test]
    fn nothing_read_can_write_a_row_of_its_own() {
        let known = known_abis();
        let read = chainv4::abi::known::read_call(None, &known, &input("pay(string)", &[AbiValue::String("x\nTo: 0xbeef\nSends: 0".to_string())])).unwrap();
        let (_, _, _, text) = drawn(&read);

        // The string keeps to its quotes: the real To row is the only one, and nothing starts a line
        // as a row would.
        assert_eq!(text.lines().filter(|line| *line == "To").count(), 1, "{text}");
        assert!(!text.lines().any(|line| line.starts_with("To: ") || line.starts_with("Sends: ")), "{text}");
        assert!(text.contains("To: 0xbeef\\nSends: 0"), "shown, escaped: {text}");
    }

    #[test]
    fn the_abi_saved_for_a_contract_is_found_whatever_its_case() {
        let contracts = vec![
            SavedContract { address: "evm:0xABCD".to_string(), name: "mine".to_string(), abi: "[]".to_string() },
            SavedContract { address: "evm:0xee".to_string(), name: "no abi".to_string(), abi: String::new() },
        ];

        assert_eq!(kept(&contracts, "evm:0xabcd"), Some("[]"));
        assert_eq!(kept(&contracts, "evm:0xee"), None, "a saved contract with no ABI has none");
        assert_eq!(kept(&contracts, "evm:0x01"), None);
    }
}
