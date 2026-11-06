//! What a person types that must not linger: a password, seed words, a private key. Typed keeps
//! it in room made up front, so typing never moves it and leaves an unwiped copy behind, and
//! wipes it as it drops; secret draws its field without the undo history egui would keep of it.
//! What egui lays out to draw a shown secret is wiped as it is freed, by the apps' allocator
//! (src/wipe.rs); what the system swaps out is beyond reach.

use {egui::Ui, zeroize::{Zeroize, Zeroizing}};

/// What is typed into a password or words field: wiped from memory as it drops, with room made
/// up front, so typing never moves it and leaves an unwiped copy behind.
pub struct Typed(Zeroizing<String>);

/// The room a typed secret has: 24 words, or a long password, many times over.
pub const ROOM: usize = 1024;

impl Typed {
    /// A typed secret holding this text, which is wiped where it was.
    pub fn new(mut text: String) -> Typed {
        let mut typed = Typed::default();
        typed.0.push_str(&text);
        text.zeroize();

        typed
    }
}

impl Default for Typed {
    fn default() -> Typed {
        Typed(Zeroizing::new(String::with_capacity(ROOM)))
    }
}

/// Never the secret itself, in a log or a failed test.
impl std::fmt::Debug for Typed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Typed({} characters)", self.0.chars().count())
    }
}

impl Clone for Typed {
    fn clone(&self) -> Typed {
        Typed::new(self.0.as_str().to_string())
    }
}

impl PartialEq for Typed {
    fn eq(&self, other: &Typed) -> bool {
        *self.0 == *other.0
    }
}

impl std::ops::Deref for Typed {
    type Target = String;

    fn deref(&self) -> &String {
        &self.0
    }
}

impl std::ops::DerefMut for Typed {
    fn deref_mut(&mut self) -> &mut String {
        &mut self.0
    }
}

/// A field for a secret, a password or words: as ui.add, then egui's undo history of it dropped,
/// which would keep a copy of each change for as long as the field lives.
pub fn secret(ui: &mut Ui, edit: egui::TextEdit) -> egui::Response {
    let response = ui.add(edit);

    if let Some(mut state) = egui::text_edit::TextEditState::load(ui.ctx(), response.id) {
        state.clear_undoer();
        state.store(ui.ctx(), response.id);
    }

    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_field_keeps_no_undo_history() {
        let ctx = egui::Context::default();
        let mut typed = Typed::new("abandon abandon".to_string());
        let id = egui::Id::unique("the words");
        let draw = |ctx: &egui::Context, typed: &mut Typed| {
            let mut drawn = ctx.run_ui(egui::RawInput::default(), |ui| {
                secret(ui, egui::TextEdit::singleline(&mut **typed).id(id));
            });
            drawn.textures_delta.clear();
        };
        draw(&ctx, &mut typed);

        // As typing leaves it: a copy of the text before, in the undo history.
        let mut state = egui::text_edit::TextEditState::load(&ctx, id).unwrap();
        let mut undoer = state.undoer();
        let before = (Default::default(), "abandon".to_string());
        undoer.add_undo(&before);
        state.set_undoer(undoer);
        state.store(&ctx, id);
        let now = (Default::default(), "abandon abandon".to_string());
        assert!(egui::text_edit::TextEditState::load(&ctx, id).unwrap().undoer().has_undo(&now), "what egui keeps");

        // The next frame drops it.
        draw(&ctx, &mut typed);
        assert!(!egui::text_edit::TextEditState::load(&ctx, id).unwrap().undoer().has_undo(&now));
    }
}
