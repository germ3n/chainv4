//! Tools, in four tabs. **Decode** reads a raw signed transaction, the wallet's swap codes and
//! multisig bundles, and EVM calldata and logs by chainv4's own contracts' ABIs or one pasted.
//! **Convert** turns amounts between the smallest unit and the coin, an address into its EVM
//! alias and back, a public key into its address under each scheme, and bech32 into hex and back.
//! **Hash & verify** hashes text or hex with every hash the build has, checks a signature, and
//! works out an EVM selector and event topic. **Proof** fetches a proof of an account, a state
//! entry or a receipt, or takes one pasted, and checks it against the light client step by step.
//! Everything but Proof's fetch works offline.

use {
    super::Ctx,
    egui::Ui,
};

pub mod convert;
pub mod decode;
pub mod hash;
pub mod proof;

#[cfg(test)]
mod tests;

const ABOUT: &str = "Decode what the chain and the wallet write, convert between their forms, hash and check signatures, and check the node's proofs against the light client.";

/// The tabs, in order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Decode,
    Convert,
    Hash,
    Proof,
}

impl Tab {
    pub const ALL: [(Tab, &'static str); 4] = [(Tab::Decode, "Decode"), (Tab::Convert, "Convert"), (Tab::Hash, "Hash & verify"), (Tab::Proof, "Proof")];
}

#[derive(Default)]
pub struct ToolsState {
    pub tab: Tab,
    pub decode: decode::DecodeState,
    pub convert: convert::ConvertState,
    pub hash: hash::HashState,
    pub proof: proof::ProofState,
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx, state: &mut ToolsState) {
    let look = ctx.look;
    super::heading(ui, look, "Tools", ABOUT, |_| {});

    ui.horizontal_wrapped(|ui| {
        for (tab, name) in Tab::ALL {
            ui.selectable_value(&mut state.tab, tab, name);
        }
    });
    ui.add_space(8.0);

    match state.tab {
        Tab::Decode => decode::show(ui, ctx, &mut state.decode),
        Tab::Convert => convert::show(ui, ctx, &mut state.convert),
        Tab::Hash => hash::show(ui, ctx, &mut state.hash),
        Tab::Proof => proof::show(ui, ctx, &mut state.proof),
    }
}

/// A field as wide as a section allows on a phone, and a comfortable width on a wide screen.
fn width(ctx: &Ctx) -> f32 {
    if ctx.look.narrow { f32::INFINITY } else { 560.0 }
}

/// A result line: what came out, copyable, or why nothing did, in red.
fn outcome(ui: &mut Ui, ctx: &Ctx, result: &Result<String, String>) {
    match result {
        Ok(text) => super::copyable(ui, text),
        Err(why) => {
            ui.add(egui::Label::new(egui::RichText::new(why).color(ctx.look.palette.bad)).wrap());
        }
    }
}
