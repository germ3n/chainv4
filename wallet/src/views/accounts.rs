//! The accounts this wallet holds keys for: making a new one, bringing one in from a key file or
//! its text, and naming, using, exporting or removing each. Every change seals the vault again.

use {
    super::Ctx,
    crate::app::Holder,
    crate::format,
    crate::theme,
    chainv4::crypto::keypair::Keypair,
    egui::{RichText, Ui},
    std::sync::{Arc, Mutex},
};

/// A key file picked, by name, once it is read.
type Picked = Arc<Mutex<Option<(String, Vec<u8>)>>>;

#[derive(Default)]
pub struct AccountsState {
    name: String,
    /// A key's text, pasted, and a key file picked, once it is read.
    pasted: String,
    picked: Picked,
    error: Option<String>,
    renaming: Option<(usize, String)>,
    /// The account whose key shows, and the one asked to be removed.
    revealing: Option<usize>,
    removing: Option<usize>,
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx, state: &mut AccountsState) {
    let palette = *ctx.palette;
    take_picked(ctx, state);

    theme::title(ui, &palette, "Accounts", "The keys this wallet holds, sealed with your password on this device.");

    if ctx.holders.is_empty() {
        theme::card(ui, &palette, |ui| {
            theme::caps(ui, &palette, "Start here");
            ui.label(RichText::new("Make a new account, or bring in a key you have. Either way the key stays on this device, sealed.").color(palette.soft));
        });
        ui.add_space(14.0);
    }

    for index in 0..ctx.holders.len() {
        holder(ui, ctx, state, index);
        ui.add_space(10.0);
    }

    ui.add_space(4.0);
    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Add an account");
        ui.add_space(4.0);
        ui.add(egui::TextEdit::singleline(&mut state.name).hint_text("name, such as Spending").desired_width(f32::INFINITY));
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if theme::primary(ui, &palette, "New account", true).clicked() {
                let made = Keypair::generate(Keypair::development().public().key_type()).map_err(|err| format!("no randomness for a key: {err}"));
                add(ctx, state, made);
            }

            if theme::quiet(ui, &palette, "Open key file…").clicked() {
                pick(ctx.wake, state.picked.clone());
            }
        });

        ui.add_space(10.0);
        ui.label(RichText::new("Or paste a key, as chainv4 keygen writes it:").small().color(palette.muted));
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut state.pasted).password(true).hint_text("ed25519:0x…").desired_width(ui.available_width() - 90.0));

            if theme::quiet(ui, &palette, "Import").clicked() {
                let read = Keypair::from_key_file(&state.pasted);
                add(ctx, state, read);
            }
        });

        ui.add_space(10.0);
        egui::CollapsingHeader::new(RichText::new("Development key").color(palette.soft)).id_salt("development").show(ui, |ui| {
            ui.label(
                RichText::new("Development chains give their coin to a well-known key whose seed is in chainv4's source. Anyone can spend from it: use it to try things on a dev chain, never for anything of value.")
                    .small()
                    .color(palette.warn),
            );

            if theme::quiet(ui, &palette, "Add the development key").clicked() {
                if state.name.trim().is_empty() {
                    state.name = "Development".to_string();
                }

                add(ctx, state, Ok(Keypair::development()));
            }
        });

        if let Some(error) = &state.error {
            ui.add_space(6.0);
            ui.colored_label(palette.bad, error);
        }
    });
}

fn holder(ui: &mut Ui, ctx: &mut Ctx, state: &mut AccountsState, index: usize) {
    let palette = *ctx.palette;
    let address = ctx.holders[index].address.to_string();
    let current = index == ctx.current;

    theme::card(ui, &palette, |ui| {
        ui.horizontal(|ui| {
            match &mut state.renaming {
                Some((renamed, name)) if *renamed == index => {
                    let field = ui.add(egui::TextEdit::singleline(name).desired_width(220.0));

                    if field.lost_focus() || theme::quiet(ui, &palette, "Done").clicked() {
                        let name = name.trim().to_string();

                        if !name.is_empty() {
                            ctx.holders[index].name = name;
                            *ctx.changed = true;
                        }

                        state.renaming = None;
                    }
                }
                _ => {
                    ui.label(theme::strong(&ctx.holders[index].name, 16.0).color(palette.ink));
                }
            }

            if current {
                theme::pill(ui, "In use", palette.accent);
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| match ctx.chain.account(&address) {
                Some(proven) => {
                    ui.label(format::amount(&proven.value.balance.to_string(), ctx.coin.decimals, &ctx.coin.symbol));
                }
                None => {
                    ui.label(RichText::new("not proven yet").small().color(palette.muted));
                }
            });
        });
        super::address(ui, &palette, &address);
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if !current && theme::quiet(ui, &palette, "Use").clicked() {
                ctx.settings.current = address.clone();
            }

            if theme::quiet(ui, &palette, "Rename").clicked() {
                state.renaming = Some((index, ctx.holders[index].name.clone()));
            }

            let revealed = state.revealing == Some(index);

            if theme::quiet(ui, &palette, if revealed { "Hide key" } else { "Export key" }).clicked() {
                state.revealing = (!revealed).then_some(index);
            }

            if theme::quiet(ui, &palette, "Remove").clicked() {
                state.removing = Some(index);
            }
        });

        if state.revealing == Some(index) {
            let key = ctx.holders[index].key.to_key_file();
            ui.add_space(8.0);
            ui.label(RichText::new("Anyone who sees this key can spend from the account. Keep it somewhere only you can read.").small().color(palette.warn));
            ui.horizontal(|ui| {
                ui.label(theme::mono(&key).color(palette.soft));
                super::copy(ui, &palette, &key);
            });
        }

        if state.removing == Some(index) {
            ui.add_space(8.0);
            ui.label(RichText::new("Its key leaves this wallet. Without a copy of it, what the account holds is out of reach for good.").color(palette.bad));
            ui.horizontal(|ui| {
                if theme::primary(ui, &palette, "Remove it", true).clicked() {
                    ctx.holders.remove(index);
                    state.removing = None;
                    state.revealing = None;
                    *ctx.changed = true;

                    if current {
                        ctx.settings.current = ctx.holders.first().map(|holder| holder.address.to_string()).unwrap_or_default();
                    }
                }

                if theme::quiet(ui, &palette, "Keep it").clicked() {
                    state.removing = None;
                }
            });
        }
    });
}

/// Adds an account with the name typed, unless the wallet has it already.
fn add(ctx: &mut Ctx, state: &mut AccountsState, key: Result<Keypair, String>) {
    let key = match key {
        Ok(key) => key,
        Err(why) => {
            state.error = Some(why);

            return;
        }
    };
    let address = key.public().address();

    if ctx.holders.iter().any(|holder| holder.address == address) {
        state.error = Some(format!("{} is in this wallet already", format::short(&address.to_string())));

        return;
    }

    let name = match state.name.trim() {
        "" => format!("Account {}", ctx.holders.len() + 1),
        name => name.to_string(),
    };
    ctx.holders.push(Holder { name, key, address });
    ctx.settings.current = address.to_string();
    *ctx.changed = true;
    state.name.clear();
    state.pasted.clear();
    state.error = None;
}

/// Asks for a key file, which arrives later: in the browser, picking one is asynchronous.
fn pick(wake: &egui::Context, picked: Picked) {
    let wake = wake.clone();
    let finish = move |name: String, bytes: Vec<u8>| {
        *picked.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some((name, bytes));
        wake.request_repaint();
    };

    #[cfg(not(target_arch = "wasm32"))]
    if let Some(path) = rfd::FileDialog::new().pick_file()
        && let Ok(bytes) = std::fs::read(&path)
    {
        finish(path.file_name().map_or_else(String::new, |name| name.to_string_lossy().into_owned()), bytes);
    }

    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(async move {
        if let Some(file) = rfd::AsyncFileDialog::new().pick_file().await {
            let bytes = file.read().await;
            finish(file.file_name(), bytes);
        }
    });
}

fn take_picked(ctx: &mut Ctx, state: &mut AccountsState) {
    let picked = state.picked.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();

    if let Some((name, bytes)) = picked {
        let read = String::from_utf8(bytes)
            .map_err(|_| "it is not text".to_string())
            .and_then(|text| Keypair::from_key_file(&text))
            .map_err(|why| format!("{name}: {why}"));

        if state.name.trim().is_empty() {
            state.name = name.trim_end_matches(".key").to_string();
        }

        add(ctx, state, read);
    }
}
