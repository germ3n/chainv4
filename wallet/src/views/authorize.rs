//! Signed authorizations of tokens (docs/token-standard.md, "Signed authorizations"): an allowance
//! or a transfer this account signs off the chain, for someone else to send and pay for, handed on
//! as text and a QR code; and sending one someone else signed, once it reads and holds. The text
//! signed is written here from the terms alone, and signed only when the token writes the same:
//! a token that writes anything else is not one whose signatures this page makes.

use {
    super::send::{Prepared, SendState},
    super::{Ctx, Page},
    crate::format,
    crate::node::Ask,
    crate::theme,
    chainv4::registry::tokens::authorization::{SignedAuthorization, TokenAuthorization},
    chainv4::structs::address::Address,
    chainv4::structs::tx::TxCallArgumentType,
    chainv4::structs::u256::u256,
    egui::{RichText, Ui},
    serde_json::{Value, json},
};

/// How long an authorization holds unless the hours say otherwise.
const HOURS: &str = "24";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Mode {
    #[default]
    Sign,
    Send,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Kind {
    #[default]
    Allowance,
    Transfer,
}

#[derive(Default)]
pub struct AuthorizeState {
    mode: Mode,
    kind: Kind,
    from: Option<Address>,
    token: String,
    to: String,
    amount: String,
    hours: String,
    /// The terms prepared, waiting for the token's text and, for an allowance, its nonce.
    prepared: Option<TokenAuthorization>,
    /// What the token wrote for them, once it answered: its text, or why not.
    written: Option<Result<String, String>>,
    /// The owner's next permit nonce, as the token answered it.
    nonce: Option<Result<u256, String>>,
    signed: Option<SignedAuthorization>,
    pasted: String,
    trouble: Option<String>,
}

fn call_ask(token: &Address, function: &str, args: &[TxCallArgumentType]) -> Ask {
    let args: Vec<Value> = args.iter().map(chainv4::structs::json::argument).collect();

    Ask::Call(json!({ "contract": token.to_string(), "function": function, "args": args, "purpose": "authorize" }).to_string())
}

/// A call's one value as text, once it returned.
fn returned(result: &Result<Value, crate::node::Failure>) -> Result<String, String> {
    match result {
        Ok(value) if value["status"] == "returned" => value["values"][0]["value"].as_str().map(str::to_string).ok_or_else(|| "it returned nothing".to_string()),
        Ok(value) => Err(value["error"].as_str().unwrap_or("the token would not say").to_string()),
        Err(failure) => Err(failure.message.clone()),
    }
}

impl AuthorizeState {
    /// Takes the token's answers; false for anyone else's.
    pub fn answered(&mut self, ask: &Ask, result: &Result<Value, crate::node::Failure>) -> bool {
        let Ask::Call(body) = ask else {
            return false;
        };
        let Ok(body) = serde_json::from_str::<Value>(body) else {
            return false;
        };

        if body["purpose"] != "authorize" {
            return false;
        }

        match body["function"].as_str() {
            Some("nonces") => self.nonce = Some(returned(result).and_then(|digits| u256::from_str_radix(&digits, 10).map_err(|_| "a nonce that is not a number".to_string()))),
            Some("permit_message" | "transfer_message") => self.written = Some(returned(result)),
            _ => {}
        }

        true
    }

    /// The terms, with the nonce in place once the token said it: what is signed.
    fn terms(&self) -> Option<TokenAuthorization> {
        match self.prepared.clone()? {
            TokenAuthorization::Permit { token, owner, spender, amount, deadline, .. } => {
                let nonce = *self.nonce.as_ref()?.as_ref().ok()?;

                Some(TokenAuthorization::Permit { token, owner, spender, amount, nonce, deadline })
            }
            transfer => Some(transfer),
        }
    }
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx, state: &mut AuthorizeState, send: &mut SendState) {
    let palette = *ctx.palette;
    theme::title(ui, &palette, "Authorize", "Sign an allowance or a transfer of a token for someone else to send and pay for, or send one signed for you.");

    theme::card(ui, &palette, |ui| {
        theme::segmented_fit(ui, &palette, &mut state.mode, &[(Mode::Sign, "Sign one"), (Mode::Send, "Send one")]);
    });
    theme::section_gap(ui);

    let Some(genesis) = ctx.chain.genesis else {
        theme::card(ui, &palette, |ui| super::waiting(ui, &palette, "Following the chain first: an authorization names it."));

        return;
    };
    let chain_id = chainv4::crypto::ethereum::chain_id(&genesis);

    match state.mode {
        Mode::Sign => sign(ui, ctx, state, chain_id),
        Mode::Send => {
            if let Some(prepared) = take(ui, ctx, state, chain_id) {
                send.prepare(prepared);
                *ctx.page = Page::Send;
            }
        }
    }
}

/// The addresses of the account whose keys this wallet holds: what signs text here.
fn signers(ctx: &Ctx) -> Vec<Address> {
    ctx.holder().map(|holder| holder.addresses().into_iter().filter(|address| holder.key(address).is_some()).collect()).unwrap_or_default()
}

fn sign(ui: &mut Ui, ctx: &mut Ctx, state: &mut AuthorizeState, chain_id: u64) {
    let palette = *ctx.palette;
    let signers = signers(ctx);

    if signers.is_empty() {
        theme::card(ui, &palette, |ui| {
            ui.add(egui::Label::new(RichText::new("This account's keys are not in this wallet: on the offline signer or in MetaMask, which sign transactions, not a token's text.").color(palette.muted)).wrap());
        });

        return;
    }

    if state.hours.is_empty() {
        state.hours = HOURS.to_string();
    }

    let from = *state.from.get_or_insert(signers[0]);
    let from = if signers.contains(&from) { from } else { signers[0] };
    state.from = Some(from);
    let mut tokens: Vec<(String, String)> = ctx.chain.tokens.iter().map(|(address, proven)| (address.clone(), proven.value.symbol.clone().unwrap_or_else(|| format::short(address)))).collect();
    tokens.sort_by(|a, b| a.1.cmp(&b.1));

    theme::card(ui, &palette, |ui| {
        theme::segmented_fit(ui, &palette, &mut state.kind, &[(Kind::Allowance, "An allowance"), (Kind::Transfer, "A transfer")]);
        ui.add_space(6.0);
        super::row(ui, &palette, "From", |ui| {
            egui::ComboBox::from_id_salt("authorize from").selected_text(format::short(&from.to_string())).show_ui(ui, |ui| {
                for address in &signers {
                    if ui.selectable_label(*address == from, format!("{} {}", super::scheme(address), format::short(&address.to_string()))).clicked() {
                        state.from = Some(*address);
                    }
                }
            });
        });
        super::row(ui, &palette, "Token", |ui| {
            let shown = tokens.iter().find(|(address, _)| *address == state.token).map_or_else(|| "Pick a token".to_string(), |(_, symbol)| symbol.clone());
            egui::ComboBox::from_id_salt("authorize token").selected_text(shown).show_ui(ui, |ui| {
                for (address, symbol) in &tokens {
                    if ui.selectable_label(*address == state.token, symbol).clicked() {
                        state.token = address.clone();
                    }
                }
            });
        });
        let party = if state.kind == Kind::Allowance { "Spender" } else { "To" };
        super::row(ui, &palette, party, |ui| {
            ui.add(egui::TextEdit::singleline(&mut state.to).hint_text("an address or a name").desired_width(f32::INFINITY));
        });
        let (symbol, decimals) = ctx.token(&state.token);
        super::row(ui, &palette, "Amount", |ui| {
            ui.add(egui::TextEdit::singleline(&mut state.amount).hint_text(format!("in {symbol}")).desired_width(160.0));
        });
        super::row(ui, &palette, "Holds for", |ui| {
            ui.add(egui::TextEdit::singleline(&mut state.hours).desired_width(60.0));
            ui.label(RichText::new("hours").color(palette.muted));
        });

        let to = recipient(ctx, &state.to);
        let amount = format::parse_units(&state.amount, decimals).and_then(|units| u256::from_str_radix(&units, 10).ok()).filter(|amount| *amount > u256::ZERO);
        let hours = state.hours.trim().parse::<u64>().ok().filter(|hours| (1..=24 * 365).contains(hours));
        let token = state.token.parse::<Address>().ok();
        let problem = match (token, to, amount, hours) {
            (None, ..) => Some("Pick the token."),
            (_, None, ..) if !state.to.trim().is_empty() => Some("Not an address, nor a name proven to point at one."),
            (_, None, ..) => Some("Say who it is for."),
            (_, _, None, _) => Some("An amount above zero, in the token's units."),
            (.., None) => Some("Hours from 1 to a year's."),
            _ => None,
        };

        if let Some(problem) = problem.filter(|_| !state.to.is_empty() || !state.amount.is_empty()) {
            ui.colored_label(palette.warn, problem);
        }

        if theme::primary(ui, &palette, "Prepare", problem.is_none()).clicked()
            && let (Some(token), Some(to), Some(amount), Some(hours)) = (token, to, amount, hours)
        {
            let until = ctx.now.saturating_add(hours * 3_600_000);
            let prepared = match state.kind {
                Kind::Allowance => TokenAuthorization::Permit { token, owner: from, spender: to, amount, nonce: u256::ZERO, deadline: until },
                Kind::Transfer => {
                    let mut nonce = [0; 32];
                    let _ = getrandom::fill(&mut nonce);

                    TokenAuthorization::Transfer { token, from, to, amount, valid_after: 0, valid_before: until, nonce }
                }
            };
            let (function, args) = prepared.message_call();
            ctx.node.ask(call_ask(&token, function, &args));

            if matches!(prepared, TokenAuthorization::Permit { .. }) {
                ctx.node.ask(call_ask(&token, "nonces", &[TxCallArgumentType::Address(from)]));
            }

            state.nonce = None;
            state.written = None;
            state.signed = None;
            state.trouble = None;
            state.prepared = Some(prepared);
        }
    });

    if state.prepared.is_none() {
        return;
    }

    theme::section_gap(ui);
    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "What it signs");
        let terms = state.terms();

        match (&state.written, &state.nonce, &terms) {
            (Some(Err(why)), ..) | (_, Some(Err(why)), _) => {
                ui.colored_label(palette.bad, format!("The token did not say: {why}. A token without permit_message or transfer_message takes no signed authorization."));
            }
            (Some(Ok(written)), _, Some(terms)) => {
                let ours = terms.message(chain_id);

                ui.label(theme::mono(ours.clone()));

                if *written != ours {
                    ui.colored_label(palette.bad, "The token writes other text for these terms than the standard does: nothing is signed for it.");

                    return;
                }

                match &state.signed {
                    None => {
                        if theme::primary(ui, &palette, "Sign it", true).clicked() {
                            let key = ctx.holder().and_then(|holder| holder.key(&terms.owner()));

                            match key.map(|key| SignedAuthorization::sign(terms.clone(), key, chain_id)) {
                                Some(Ok(signed)) => state.signed = Some(signed),
                                Some(Err(why)) => state.trouble = Some(why),
                                None => state.trouble = Some("This wallet does not hold that address's key.".to_string()),
                            }
                        }
                    }
                    Some(signed) => {
                        let text = signed.to_text();
                        ui.add_space(6.0);
                        theme::caps(ui, &palette, "Hand this on");
                        ui.add(egui::Label::new(RichText::new("Whoever sends it pays for it. It holds once, until it lapses.").small().color(palette.muted)).wrap());
                        super::receive::qr(ui, &text, 260.0_f32.min(ui.available_width()));
                        ui.label(theme::mono(format::short(&text)));
                        super::copy(ui, &palette, &text);
                    }
                }
            }
            _ => super::waiting(ui, &palette, "Asking the token for its text."),
        }

        if let Some(trouble) = &state.trouble {
            ui.colored_label(palette.bad, trouble);
        }
    });
}

/// Who to authorize: an address, or a name the light client proved points at one.
fn recipient(ctx: &Ctx, text: &str) -> Option<Address> {
    let text = text.trim();

    text.parse().ok().or_else(|| ctx.chain.resolve(text).and_then(|resolved| resolved.target()))
}

/// When a time in milliseconds is, as a person reads it.
fn when(ms: u64) -> String {
    format::utc(ms)
}

/// Sending one someone signed: pasted, read, checked as the token checks it, and shown before it
/// goes to Send.
fn take(ui: &mut Ui, ctx: &mut Ctx, state: &mut AuthorizeState, chain_id: u64) -> Option<Prepared> {
    let palette = *ctx.palette;
    let from = ctx.holder().and_then(|holder| holder.addresses().into_iter().next())?;
    let mut prepared = None;

    theme::card(ui, &palette, |ui| {
        ui.add(egui::TextEdit::multiline(&mut state.pasted).hint_text("Paste a signed authorization").desired_rows(4).desired_width(f32::INFINITY));

        if state.pasted.trim().is_empty() {
            return;
        }

        let signed = match SignedAuthorization::from_text(&state.pasted) {
            Ok(signed) => signed,
            Err(why) => {
                ui.colored_label(palette.bad, why);

                return;
            }
        };
        let authorization = &signed.authorization;
        let token = authorization.token().to_string();
        let (symbol, decimals) = ctx.token(&token);
        let held = signed.check(chain_id);

        ui.add_space(6.0);
        super::row(ui, &palette, "Token", |ui| {
            ui.label(&symbol);
            super::address(ui, &palette, &token);
        });

        let lapses = match authorization {
            TokenAuthorization::Permit { owner, spender, amount, deadline, .. } => {
                super::row(ui, &palette, "Owner", |ui| super::address(ui, &palette, &owner.to_string()));
                super::row(ui, &palette, "Lets", |ui| super::address(ui, &palette, &spender.to_string()));
                super::row(ui, &palette, "Move up to", |ui| {
                    ui.label(format::amount(&amount.to_string(), decimals, &symbol));
                });
                super::row(ui, &palette, "Until", |ui| {
                    ui.label(when(*deadline));
                });

                *deadline
            }
            TokenAuthorization::Transfer { from, to, amount, valid_after, valid_before, .. } => {
                super::row(ui, &palette, "From", |ui| super::address(ui, &palette, &from.to_string()));
                super::row(ui, &palette, "To", |ui| super::address(ui, &palette, &to.to_string()));
                super::row(ui, &palette, "Amount", |ui| {
                    ui.label(format::amount(&amount.to_string(), decimals, &symbol));
                });
                super::row(ui, &palette, "Holds", |ui| {
                    ui.label(match *valid_after {
                        0 => format!("until {}", when(*valid_before)),
                        after => format!("from {} until {}", when(after), when(*valid_before)),
                    });
                });

                *valid_before
            }
        };

        match &held {
            Ok(()) => ui.label(RichText::new("Signed by the owner's key, for this chain and this token.").small().color(palette.good)),
            Err(why) => ui.colored_label(palette.bad, format!("It does not hold: {why}.")),
        };

        let lapsed = lapses <= ctx.now;

        if lapsed {
            ui.colored_label(palette.bad, "It has lapsed.");
        }

        if theme::primary(ui, &palette, "Review and send", held.is_ok() && !lapsed).clicked() {
            prepared = Some(Prepared {
                from,
                tx: signed.call(),
                what: authorization.summary(),
                about: format!("signed by {}", format::short(&authorization.owner().to_string())),
                coin: u256::ZERO,
                functions: vec![signed.function().to_string()],
            });
        }
    });

    prepared
}

#[cfg(test)]
mod tests;
