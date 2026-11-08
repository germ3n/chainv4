//! A contract's versions: its deploy and each upgrade after, oldest first as the node's versions
//! index lists them, the newest shown first. A version picked shows its code as its transaction
//! carried it: a Lua contract's source, from its block, and a WebAssembly module's functions,
//! reentrant ones and layout, from its sections; and what the version after it changed, line by
//! line. Each version names its transaction and block, whose page proves them as any other.

use {
    super::Ctx,
    crate::api::Ask,
    crate::format,
    crate::model::{TxView, Version, steps_of},
    egui::{RichText, Ui},
    serde_json::Value,
};

/// The most lines a diff weighs against each other after the same start and end are set aside:
/// past it the two are shown whole, one after the other.
const DIFF_CELLS: usize = 4_000_000;

#[derive(Default)]
pub struct VersionsState {
    /// The contract these are for: another one starts afresh.
    address: String,
    /// The version picked, by its place oldest first.
    picked: Option<usize>,
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx, address: &str, state: &mut VersionsState) {
    if state.address != address {
        *state = VersionsState { address: address.to_string(), picked: None };
    }

    let look = ctx.look;
    let store = ctx.store;
    let ask = Ask::ContractVersions { address: address.to_string(), cursor: None };
    let Some(listed) = store.versions.get(address) else {
        ctx.api.ask(ask.clone());
        super::waiting(ui, ctx, &ask);

        return;
    };

    if let Some(down_to) = listed.catching_up_to {
        let from = listed.indexed_from.map_or_else(String::new, |from| format!(" It holds blocks from {} on.", format::thousands(&from.to_string())));
        super::note(ui, look, format!("The node is still filling its versions index, down to block {}, so earlier versions may not show yet.{from}", format::thousands(&down_to.to_string())));
    }

    if listed.versions.is_empty() {
        super::note(ui, look, "No deploy of it is in the node's versions index: genesis placed it, or a contract cloned it, so no transaction carried its first code.");

        return;
    }

    let versions = &listed.versions;
    let newest = versions.len() - 1;
    let label = |place: usize| match place == newest {
        true => format!("v{} · now", place + 1),
        false => format!("v{}", place + 1),
    };

    match look.narrow {
        true => {
            for (place, version) in versions.iter().enumerate().rev() {
                super::list_row(ui, look, |ui| {
                    ui.horizontal(|ui| {
                        if ui.selectable_label(state.picked == Some(place), RichText::new(label(place)).strong()).clicked() {
                            state.picked = (state.picked != Some(place)).then_some(place);
                        }

                        ui.label(RichText::new(&version.kind).color(look.palette.muted));
                    });
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("block").color(look.palette.muted));
                        super::block_link(ui, ctx, &version.block, version.height);
                        ui.label(RichText::new(format!("{} · {} bytes", version.engine, format::thousands(&version.size.to_string()))).color(look.palette.muted));
                    });
                });
            }
        }
        false => super::table(ui, look, "versions", &["version", "by", "block", "transaction", "engine", "code"], |ui| {
            for (place, version) in versions.iter().enumerate().rev() {
                if ui.selectable_label(state.picked == Some(place), RichText::new(label(place)).strong()).clicked() {
                    state.picked = (state.picked != Some(place)).then_some(place);
                }

                ui.label(match version.step {
                    Some(step) => format!("{}, step {}", version.kind, step + 1),
                    None => version.kind.clone(),
                });
                super::block_link(ui, ctx, &version.block, version.height);
                super::tx(ui, ctx, &version.tx);
                ui.monospace(&version.engine);
                ui.monospace(format!("{} bytes", format::thousands(&version.size.to_string())))
                    .on_hover_text(format!("{}, hashed by the chain's hash", version.code_hash));
                super::row_end(ui);
            }
        }),
    }

    if let Some(place) = state.picked.filter(|place| *place <= newest) {
        ui.add_space(8.0);
        picked(ui, ctx, versions, place);
    } else {
        super::note(ui, look, "Pick a version for its code, and what the one after it changed.");
    }
}

/// A version's code, and what the one after changed.
fn picked(ui: &mut Ui, ctx: &mut Ctx, versions: &[Version], place: usize) {
    let look = ctx.look;
    let version = &versions[place];
    let next = versions.get(place + 1);

    ui.label(RichText::new(format!("v{}: its code", place + 1)).strong());
    super::copyable(ui, &version.code_hash);

    match version.engine.as_str() {
        "lua" => {
            let Some(source) = source(ctx, version) else {
                return;
            };

            code_box(ui, ("source", place), &source);

            if let Some(next) = next {
                let Some(after) = source_of(ctx, next) else {
                    return;
                };

                ui.add_space(6.0);
                ui.label(RichText::new(format!("What v{} changed", place + 2)).strong());
                diff_box(ui, ctx, ("diff", place), &source, &after);
            }
        }
        "wasm" => {
            let Some(outline) = &version.outline else {
                super::note(ui, look, "The node did not read this module's sections.");

                return;
            };
            let lines = |outline: &crate::model::Outline| outline_text(outline);

            code_box(ui, ("outline", place), &lines(outline));

            if let Some(after) = next.and_then(|next| next.outline.as_ref()) {
                ui.add_space(6.0);
                ui.label(RichText::new(format!("What v{} changed", place + 2)).strong());
                diff_box(ui, ctx, ("outline diff", place), &lines(outline), &lines(after));
            }
        }
        _ => super::note(ui, look, "EVM code is shown on the contract's EVM page."),
    }
}

/// A WebAssembly module's outline as lines, one thing a line, for showing and diffing.
fn outline_text(outline: &crate::model::Outline) -> String {
    let mut lines = Vec::new();
    lines.extend(outline.functions.iter().map(|function| match outline.reentrant.contains(function) {
        true => format!("function {function} (reentrant)"),
        false => format!("function {function}"),
    }));
    lines.extend(outline.layout.iter().map(|entry| format!("layout {entry}")));

    lines.join("\n")
}

/// A Lua version's source, from its block, asking for the block when it is not here yet.
fn source(ctx: &mut Ctx, version: &Version) -> Option<String> {
    let found = source_of(ctx, version);

    if found.is_none() && ctx.store.detail_for(&version.block).is_none() {
        ctx.api.ask(Ask::Block(version.block.clone()));
    }

    found
}

/// What the block's transaction carried for this version, when the block is here, asking for it
/// otherwise.
fn source_of(ctx: &mut Ctx, version: &Version) -> Option<String> {
    let Some(detail) = ctx.store.detail_for(&version.block) else {
        ctx.api.ask(Ask::Block(version.block.clone()));

        return None;
    };

    carried(detail.txs.get(version.index as usize)?, version.step).and_then(|tx| tx.get("source")).and_then(Value::as_str).map(str::to_string)
}

/// The transaction, or the batch step, that carried a version's code.
fn carried(view: &TxView, step: Option<u32>) -> Option<&Value> {
    match step {
        Some(step) => steps_of(&view.tx)?.get(step as usize),
        None if view.kind() == "multisig" => view.tx.get("tx"),
        None => Some(&view.tx),
    }
}

fn code_box(ui: &mut Ui, id: impl std::hash::Hash + std::fmt::Debug, text: &str) {
    egui::ScrollArea::both().id_salt(id).max_height(360.0).show(ui, |ui| {
        ui.add(egui::Label::new(RichText::new(text).monospace()).extend());
    });
}

/// The lines of one text against the next, each marked as taken out, put in or kept.
fn diff_box(ui: &mut Ui, ctx: &Ctx, id: impl std::hash::Hash + std::fmt::Debug, before: &str, after: &str) {
    let palette = ctx.look.palette;
    let lines = diff(before, after);

    if lines.iter().all(|(change, _)| *change == Change::Kept) {
        ui.label(RichText::new("No line changed.").color(palette.muted));

        return;
    }

    egui::ScrollArea::both().id_salt(id).max_height(360.0).show(ui, |ui| {
        for (change, line) in lines {
            let (mark, color) = match change {
                Change::Out => ("- ", palette.bad),
                Change::In => ("+ ", palette.good),
                Change::Kept => ("  ", palette.muted),
            };

            ui.add(egui::Label::new(RichText::new(format!("{mark}{line}")).monospace().color(color)).extend());
        }
    });
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Change {
    Out,
    In,
    Kept,
}

/// The lines of before against after: the lines both start and end with kept as they are, and the
/// rest between weighed by their longest common run. Two too long to weigh are shown whole, the
/// one taken out, the other put in.
pub(crate) fn diff<'a>(before: &'a str, after: &'a str) -> Vec<(Change, &'a str)> {
    let (old, new): (Vec<&str>, Vec<&str>) = (before.lines().collect(), after.lines().collect());
    let start = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
    let end = old[start..].iter().rev().zip(new[start..].iter().rev()).take_while(|(a, b)| a == b).count();
    let (old_mid, new_mid) = (&old[start..old.len() - end], &new[start..new.len() - end]);
    let mut lines: Vec<(Change, &str)> = old[..start].iter().map(|line| (Change::Kept, *line)).collect();

    match old_mid.len().saturating_mul(new_mid.len()) <= DIFF_CELLS {
        true => {
            // The longest common run of each pair of tails, filled from the ends.
            let width = new_mid.len() + 1;
            let mut run = vec![0u32; (old_mid.len() + 1) * width];

            for i in (0..old_mid.len()).rev() {
                for j in (0..new_mid.len()).rev() {
                    run[i * width + j] = match old_mid[i] == new_mid[j] {
                        true => run[(i + 1) * width + j + 1] + 1,
                        false => run[(i + 1) * width + j].max(run[i * width + j + 1]),
                    };
                }
            }

            let (mut i, mut j) = (0, 0);

            while i < old_mid.len() || j < new_mid.len() {
                if i < old_mid.len() && j < new_mid.len() && old_mid[i] == new_mid[j] {
                    lines.push((Change::Kept, old_mid[i]));
                    (i, j) = (i + 1, j + 1);
                } else if i < old_mid.len() && (j == new_mid.len() || run[(i + 1) * width + j] >= run[i * width + j + 1]) {
                    // A line taken out before one put in, where either keeps as much.
                    lines.push((Change::Out, old_mid[i]));
                    i += 1;
                } else {
                    lines.push((Change::In, new_mid[j]));
                    j += 1;
                }
            }
        }
        false => {
            lines.extend(old_mid.iter().map(|line| (Change::Out, *line)));
            lines.extend(new_mid.iter().map(|line| (Change::In, *line)));
        }
    }

    lines.extend(old[old.len() - end..].iter().map(|line| (Change::Kept, *line)));

    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::light::Light;
    use crate::store::Store;
    use serde_json::json;

    const CONTRACT: &str = "contract:0x02";

    /// A block holding one transaction, at this height, carrying this.
    fn block(id: &str, height: u64, tx: Value) -> crate::model::BlockDetail {
        serde_json::from_value(json!({
            "id": id, "height": height, "parent": "0xbb", "timestamp": 6000, "producer": "ed25519:0x01",
            "difficulty": "0", "tx_root": "0xcc", "gas_used": 0, "base_price": "0",
            "tx_count": 1, "proof_count": 0, "main": true,
            "seal": { "type": "signature", "signature": "0x99" },
            "proofs": [],
            "txs": [{
                "id": format!("0xd{height}"), "sender": "ed25519:0x01", "nonce": height, "signer": "ed25519:0x01", "signature": "0x98",
                "fee": { "gas_limit": 0, "max_price": "0", "tip": "0" },
                "tx": tx
            }]
        }))
        .unwrap()
    }

    fn version(block: &str, height: u64, kind: &str, step: Option<u32>) -> Version {
        Version {
            tx: format!("0xd{height}"),
            block: block.to_string(),
            height,
            index: 0,
            step,
            kind: kind.to_string(),
            engine: "lua".to_string(),
            code_hash: format!("xxh3_64:0x0{height}"),
            size: 40,
            outline: None,
        }
    }

    #[test]
    fn a_contract_lists_its_versions_and_a_picked_one_shows_its_source_and_what_came_after() {
        let mut store = Store::default();
        store.versions.insert(CONTRACT.to_string(), crate::model::Versions {
            versions: vec![version("0xa1", 1, "deploy", None), version("0xa2", 2, "upgrade", Some(1))],
            ..Default::default()
        });
        store.detail(block("0xa1", 1, json!({ "type": "deploy", "engine": "lua", "code_size": 40, "source": "local M = {}\nfunction M.v() return 1 end\nreturn M" })));
        store.detail(block("0xa2", 2, json!({ "type": "batch", "steps": [
            { "type": "transfer", "to": CONTRACT, "amount": "1" },
            { "type": "upgrade", "contract": CONTRACT, "engine": "lua", "code_size": 40, "source": "local M = {}\nfunction M.v() return 2 end\nreturn M" }
        ] })));

        for width in [1_200.0, 380.0] {
            let (text, asked) = crate::views::render(&store, &mut Light::default(), width, |ui, ctx| show(ui, ctx, CONTRACT, &mut VersionsState::default()));

            assert!(text.contains("v2 · now") && text.contains("v1") && text.contains("Pick a version"), "{text}");
            assert!(asked.is_empty(), "all here: {asked:?}");
        }

        let mut picked = VersionsState { address: CONTRACT.to_string(), picked: Some(0) };
        let (text, _) = crate::views::render(&store, &mut Light::default(), 1_200.0, |ui, ctx| show(ui, ctx, CONTRACT, &mut picked));

        assert!(text.contains("function M.v() return 1 end"), "its source: {text}");
        assert!(text.contains("What v2 changed") && text.contains("- function M.v() return 1 end") && text.contains("+ function M.v() return 2 end"), "{text}");

        // A version whose block is not here asks for it; a contract not asked yet asks its versions.
        store.details.remove("0xa2");
        let mut newest = VersionsState { address: CONTRACT.to_string(), picked: Some(1) };
        let (_, asked) = crate::views::render(&store, &mut Light::default(), 1_200.0, |ui, ctx| show(ui, ctx, CONTRACT, &mut newest));
        assert!(asked.contains(&Ask::Block("0xa2".to_string())), "{asked:?}");

        let (_, asked) = crate::views::render(&Store::default(), &mut Light::default(), 1_200.0, |ui, ctx| show(ui, ctx, CONTRACT, &mut VersionsState::default()));
        assert_eq!(asked, [Ask::ContractVersions { address: CONTRACT.to_string(), cursor: None }]);
    }

    #[test]
    fn a_contract_with_no_deploy_on_record_says_why() {
        let mut store = Store::default();
        store.versions.insert(CONTRACT.to_string(), crate::model::Versions { catching_up_to: Some(0), indexed_from: Some(900), ..Default::default() });
        let (text, _) = crate::views::render(&store, &mut Light::default(), 1_200.0, |ui, ctx| show(ui, ctx, CONTRACT, &mut VersionsState::default()));

        assert!(text.contains("still filling its versions index") && text.contains("No deploy of it"), "{text}");
    }

    #[test]
    fn a_diff_keeps_what_is_the_same_and_marks_the_rest() {
        let before = "local M = {}\nfunction M.a() return 1 end\nfunction M.b() end\nreturn M";
        let after = "local M = {}\nfunction M.a() return 2 end\nfunction M.b() end\nfunction M.c() end\nreturn M";
        let changes: Vec<(Change, &str)> = diff(before, after);

        assert_eq!(
            changes,
            [
                (Change::Kept, "local M = {}"),
                (Change::Out, "function M.a() return 1 end"),
                (Change::In, "function M.a() return 2 end"),
                (Change::Kept, "function M.b() end"),
                (Change::In, "function M.c() end"),
                (Change::Kept, "return M"),
            ]
        );
        assert!(diff(before, before).iter().all(|(change, _)| *change == Change::Kept));
        assert_eq!(diff("", "a"), [(Change::In, "a")]);
        assert_eq!(diff("a", ""), [(Change::Out, "a")]);
    }

    #[test]
    fn an_outline_is_a_line_a_thing() {
        let outline = crate::model::Outline {
            functions: vec!["get".into(), "set".into()],
            reentrant: vec!["get".into()],
            layout: vec!["map counter number".into()],
        };

        assert_eq!(outline_text(&outline), "function get (reentrant)\nfunction set\nlayout map counter number");
    }
}
