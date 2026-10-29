//! Proof: an account's, a state entry's or a transaction's receipt's proof, fetched from the node
//! at the light client's tip, or a proof's JSON pasted, checked against the light client step by
//! step: the header, its finality, the root, the path. While the light client follows nothing, the
//! proof is read and said to be unchecked, never passed off as proven.

use {
    crate::api::Ask,
    crate::format,
    crate::proof::{self, Checked, Mark, Proof, StateEntry, Verdict},
    crate::views::{self, Ctx, Look},
    chainv4::db::{Column, STATE_COLUMNS, address_key},
    chainv4::registry::supply::SUPPLY_KEY,
    chainv4::structs::account::Account,
    chainv4::structs::address::Address,
    chainv4::structs::hex::{from_hex, to_hex},
    chainv4_ui::widgets,
    egui::{RichText, Ui},
    serde_json::Value,
};

/// What to prove.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Pick {
    /// An account's balance and nonce.
    #[default]
    Account,
    /// Any chain-state entry, by its column and key.
    Entry,
    /// A transaction's receipt, and the transaction, in the block they landed in.
    Tx,
    /// A proof's JSON, as the node wrote it.
    Paste,
}

pub struct ProofState {
    pub pick: Pick,
    /// The account, the key, or the transaction id typed.
    pub typed: String,
    /// The column picked, by name.
    pub column: String,
    pub pasted: String,
    /// What was last fetched: its column, or receipts, and its key or transaction id.
    pub asked: Option<(String, String)>,
    pub trouble: Option<String>,
}

impl Default for ProofState {
    fn default() -> ProofState {
        ProofState {
            pick: Pick::default(),
            typed: String::new(),
            column: Column::Accounts.name().to_string(),
            pasted: String::new(),
            asked: None,
            trouble: None,
        }
    }
}

/// What to ask the node for a pick and what was typed, at a block the light client holds, or at
/// the node's tip while it holds none.
pub fn ask_for(pick: Pick, typed: &str, column: &str, block: Option<String>) -> Result<Ask, String> {
    let typed = typed.trim();

    match pick {
        Pick::Account => {
            let address: Address = typed.parse().map_err(|_| "not an address: kind:0x…, such as ed25519:0x…, or bc1q…".to_string())?;

            Ok(Ask::Proof { column: Column::Accounts.name().to_string(), key: to_hex(&address_key(&address)), block })
        }
        Pick::Entry => {
            let column = proof::column(column)?;
            let key = from_hex(typed).filter(|key| !key.is_empty()).ok_or("a key is hex: 0x…")?;

            Ok(Ask::Proof { column: column.name().to_string(), key: to_hex(&key), block })
        }
        Pick::Tx => match from_hex(typed) {
            Some(id) if !id.is_empty() => Ok(Ask::ReceiptProof(to_hex(&id))),
            _ => Err("a transaction id is hex: 0x…".to_string()),
        },
        Pick::Paste => Err("a pasted proof is checked as it is: nothing to fetch".to_string()),
    }
}

/// What a state entry's value says, where the explorer can read it: an account's balance and
/// nonce, or the coin's supply.
pub fn describe(entry: &StateEntry, decimals: u8, symbol: &str) -> Option<String> {
    let value = entry.value.as_deref();

    match entry.column {
        Column::Accounts => {
            let account: Account = match value {
                Some(bytes) => wincode::deserialize(bytes).ok()?,
                None => Account::default(),
            };

            Some(format!("balance {}, nonce {}", format::amount(&account.balance.to_string(), decimals, symbol), account.nonce))
        }
        Column::Validators if entry.key == SUPPLY_KEY => {
            let supply = views::rich::supply_of(value).ok()?;

            Some(format!(
                "the coin's supply: minted {}, burned {}",
                format::amount(&supply.minted.to_string(), decimals, symbol),
                format::amount(&supply.burned.to_string(), decimals, symbol)
            ))
        }
        _ => None,
    }
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx, state: &mut ProofState) {
    let look = ctx.look;
    let width = super::width(ctx);

    views::section(ui, look, "What to prove", |ui| {
        ui.horizontal_wrapped(|ui| {
            for (pick, name) in [(Pick::Account, "An account"), (Pick::Entry, "A state entry"), (Pick::Tx, "A transaction"), (Pick::Paste, "Paste a proof")] {
                if ui.selectable_value(&mut state.pick, pick, name).clicked() {
                    state.trouble = None;
                }
            }
        });
        ui.add_space(6.0);

        if state.pick == Pick::Paste {
            ui.add(egui::TextEdit::multiline(&mut state.pasted).hint_text("A proof's JSON, as GET /proofs/… answers it").font(egui::TextStyle::Monospace).desired_rows(4).desired_width(width));

            return;
        }

        if state.pick == Pick::Entry {
            egui::ComboBox::from_id_salt("proof column").selected_text(state.column.as_str()).show_ui(ui, |ui| {
                for column in STATE_COLUMNS {
                    ui.selectable_value(&mut state.column, column.name().to_string(), column.name());
                }
            });
        }

        let hint = match state.pick {
            Pick::Account => "An address: ed25519:0x…, secp256k1:0x…, bc1q…",
            Pick::Entry => "The entry's key, in hex",
            _ => "A transaction id, 0x…",
        };
        let palette = look.palette;
        let typed = !state.typed.trim().is_empty();
        let pressed = widgets::beside(
            ui,
            |ui| widgets::primary(ui, &palette, "Fetch and check", typed).clicked(),
            |ui, width| {
                ui.add(egui::TextEdit::singleline(&mut state.typed).hint_text(hint).font(egui::TextStyle::Monospace).desired_width(width));
            },
        );

        if pressed {
            match ask_for(state.pick, &state.typed, &state.column, ctx.light.tip_block()) {
                Ok(ask) => {
                    state.asked = entry_of(&ask);
                    state.trouble = None;
                    ctx.light.fetch(&ask);
                    ctx.api.ask(ask);
                }
                Err(why) => state.trouble = Some(why),
            }
        }

        if let Some(trouble) = &state.trouble {
            ui.colored_label(palette.bad, trouble.as_str());
        }

        views::note(
            ui,
            look,
            match ctx.light.client() {
                Some(_) => "Asked at the newest block the light client checked, so it holds the header to check against.",
                None => "The light client is not following this node: a proof fetched now can be read, not checked.",
            },
        );
    });

    let answer: Option<Result<Value, String>> = match state.pick {
        Pick::Paste if state.pasted.trim().is_empty() => return,
        Pick::Paste => Some(serde_json::from_str::<Value>(&state.pasted).map_err(|err| format!("not JSON: {err}"))),
        _ => {
            let Some((column, key)) = &state.asked else {
                return;
            };

            match ctx.light.fetched(column, key) {
                Some(answer) => Some(answer.clone()),
                None => {
                    views::section(ui, look, "Proof", |ui| {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(RichText::new("Fetching…").color(look.palette.muted));
                        });
                    });

                    return;
                }
            }
        }
    };

    views::section(ui, look, "Proof", |ui| match answer {
        Some(Ok(value)) => match proof::read(&value) {
            Ok(read) => {
                let checked = proof::check(&read, ctx.light.client(), ctx.light.trouble.as_deref());
                report(ui, look, &checked);
                ui.add_space(6.0);
                entry(ui, ctx, &read);
                views::json_tree(ui, ctx, "as the node wrote it", &value);
            }
            Err(why) => super::outcome(ui, ctx, &Err(why)),
        },
        Some(Err(why)) => super::outcome(ui, ctx, &Err(format!("The node did not prove it: {why}"))),
        None => {}
    });
}

/// What was asked, as the light client keeps its answer: a column and key, or receipts and an id.
fn entry_of(ask: &Ask) -> Option<(String, String)> {
    match ask {
        Ask::Proof { column, key, .. } => Some((column.clone(), key.clone())),
        Ask::ReceiptProof(tx) => Some(("receipts".to_string(), tx.clone())),
        _ => None,
    }
}

/// A verdict as a pill and words.
pub fn verdict(ui: &mut Ui, look: Look, verdict: &Verdict) {
    ui.horizontal_wrapped(|ui| {
        let (word, color, text) = match verdict {
            Verdict::Proven { final_: true, height } => ("proven, final", look.roles.final_, format!("It checks against block {}, which is final.", format::thousands(&height.to_string()))),
            Verdict::Proven { final_: false, height } => ("proven", look.palette.good, format!("It checks against block {}, not final yet.", format::thousands(&height.to_string()))),
            Verdict::Wrong(why) => ("does not check", look.palette.bad, format!("{why}.")),
            Verdict::Waiting(height) => ("waiting", look.palette.warn, format!("The light client has yet to reach block {}; checked once it does.", format::thousands(&height.to_string()))),
            Verdict::Unchecked(why) => ("not checked", look.palette.muted, format!("{why}.")),
        };
        widgets::pill(ui, word, color);
        ui.add(egui::Label::new(RichText::new(text).color(look.palette.soft)).wrap());
    });
}

/// A proof checked: the verdict, then each step with how it went.
pub fn report(ui: &mut Ui, look: Look, checked: &Checked) {
    verdict(ui, look, &checked.verdict);
    ui.add_space(4.0);

    for step in &checked.steps {
        let color = match step.mark {
            Mark::Good => look.palette.good,
            Mark::Bad => look.palette.bad,
            Mark::Unknown => look.palette.muted,
        };

        ui.horizontal_wrapped(|ui| {
            views::dot(ui, color);
            ui.label(RichText::new(step.name).strong().color(look.palette.ink));
            ui.add(egui::Label::new(RichText::new(&step.text).color(look.palette.soft)).wrap());
        });
    }
}

/// What the proof is of: the entry and what it holds, or the receipt's transaction.
fn entry(ui: &mut Ui, ctx: &mut Ctx, read: &Proof) {
    let look = ctx.look;
    let (decimals, symbol) = ctx.coin();

    views::facts(ui, look, "proven entry", |facts| match read {
        Proof::State(entry) => {
            facts.text("column", entry.column.name());
            facts.row("key", |ui| views::copyable(ui, &to_hex(&entry.key)));

            match &entry.value {
                Some(value) => facts.row("holds", |ui| views::copyable(ui, &to_hex(value))),
                None => facts.text("holds", "nothing: the proof shows the entry is empty"),
            }

            if let Some(said) = describe(entry, decimals, &symbol) {
                facts.text("which is", said);
            }
        }
        Proof::Receipt(entry) => {
            facts.row("transaction", |ui| views::tx(ui, ctx, &entry.tx));
            facts.row("block", |ui| views::block_link(ui, ctx, &entry.block, entry.height));
            facts.text("place", format!("transaction {} of {} in it", entry.index, entry.count));
        }
    });
}
