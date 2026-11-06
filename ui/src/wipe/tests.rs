use {
    super::*,
    std::sync::atomic::{AtomicUsize, Ordering},
};

/// The system's allocator, counting the blocks it has back and those not all zeros.
struct Checking {
    freed: AtomicUsize,
    unwiped: AtomicUsize,
}

unsafe impl GlobalAlloc for Checking {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let block = unsafe { std::slice::from_raw_parts(ptr, layout.size()) };
        self.freed.fetch_add(1, Ordering::SeqCst);
        if block.iter().any(|byte| *byte != 0) {
            self.unwiped.fetch_add(1, Ordering::SeqCst);
        }

        unsafe { System.dealloc(ptr, layout) }
    }
}

fn checking() -> Wiping<Checking> {
    Wiping(Checking { freed: AtomicUsize::new(0), unwiped: AtomicUsize::new(0) })
}

#[test]
fn a_freed_block_is_wiped_before_it_goes_back() {
    let wiping = checking();
    let layout = Layout::from_size_align(64, 8).unwrap();

    unsafe {
        let block = wiping.alloc(layout);
        std::ptr::write_bytes(block, 0xab, 64);
        wiping.dealloc(block, layout);
    }

    assert_eq!(wiping.0.freed.load(Ordering::SeqCst), 1);
    assert_eq!(wiping.0.unwiped.load(Ordering::SeqCst), 0);
}

#[test]
fn a_moved_block_keeps_what_it_held_and_its_old_place_is_wiped() {
    let wiping = checking();
    let layout = Layout::from_size_align(16, 4).unwrap();

    unsafe {
        let block = wiping.alloc(layout);
        for at in 0..16 {
            *block.add(at) = at as u8 + 1;
        }

        // Grown, then shrunk: each a new block, the old one wiped as it goes back.
        let grown = wiping.realloc(block, layout, 4096);
        assert_eq!(std::slice::from_raw_parts(grown, 16), (1..=16).collect::<Vec<u8>>());
        let grown_layout = Layout::from_size_align(4096, 4).unwrap();
        let shrunk = wiping.realloc(grown, grown_layout, 8);
        assert_eq!(std::slice::from_raw_parts(shrunk, 8), (1..=8).collect::<Vec<u8>>());
        wiping.dealloc(shrunk, Layout::from_size_align(8, 4).unwrap());
    }

    assert_eq!(wiping.0.freed.load(Ordering::SeqCst), 3);
    assert_eq!(wiping.0.unwiped.load(Ordering::SeqCst), 0);
}

#[test]
fn wipe_zeroes_what_it_is_given() {
    let mut words = *b"abandon ability able";
    unsafe { wipe(words.as_mut_ptr(), words.len()) };

    assert_eq!(words, [0; 20]);
}

#[test]
fn let_go_drops_the_fields_states_and_draws_until_egui_frees_the_words_it_laid_out() {
    let ctx = egui::Context::default();
    let mut typed = String::from("abandon ability able");
    let field = egui::Id::unique("the words");
    let frame = |ctx: &egui::Context, page: &mut dyn FnMut(&mut egui::Ui)| {
        let mut drawn = ctx.run_ui(egui::RawInput::default(), |ui| page(ui));
        drawn.textures_delta.clear();
    };

    // The words shown, and typed into a field.
    frame(&ctx, &mut |ui| {
        ui.label("zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo wrong");
        ui.add(egui::TextEdit::singleline(&mut typed).id(field));
    });
    assert!(egui::text_edit::TextEditState::load(&ctx, field).is_some());
    assert!(ctx.fonts(|fonts| fonts.num_galleys_in_cache()) > 0);

    // Locked: the field's state goes at once, and the frames asked for free the layouts.
    let_go(&ctx);
    assert!(egui::text_edit::TextEditState::load(&ctx, field).is_none());

    let mut frames = 0;
    while ctx.has_requested_repaint() {
        frames += 1;
        assert!(frames <= 10, "let_go keeps asking for frames");
        frame(&ctx, &mut |_| letting_go(&ctx));
    }

    assert!(frames >= 2, "a layout is freed only once a frame has gone by without it: {frames} frames");
    assert_eq!(ctx.fonts(|fonts| fonts.num_galleys_in_cache()), 0, "what was laid out is freed");
}
