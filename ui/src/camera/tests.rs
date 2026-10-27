use super::*;

/// A 3 by 2 frame, its pixels numbered row after row:
///
/// ```text
/// 0 1 2
/// 3 4 5
/// ```
fn small() -> Frame {
    Frame { width: 3, height: 2, luma: vec![0, 1, 2, 3, 4, 5] }
}

#[test]
fn frames_turn_upright_by_quarter_turns() {
    // A quarter clockwise: the left column, bottom first, becomes the top row.
    assert_eq!(small().turned(1), Frame { width: 2, height: 3, luma: vec![3, 0, 4, 1, 5, 2] });
    assert_eq!(small().turned(2), Frame { width: 3, height: 2, luma: vec![5, 4, 3, 2, 1, 0] });
    assert_eq!(small().turned(3), Frame { width: 2, height: 3, luma: vec![2, 5, 1, 4, 0, 3] });
    assert_eq!(small().turned(4), small());
    assert_eq!(small().turned(1).turned(3), small(), "a turn back undoes it");
    assert_eq!(small().image().size, [3, 2]);
}

#[test]
fn on_android_a_camera_is_asked_for_through_the_context_and_hands_frames_upright() {
    let ctx = egui::Context::default();
    let slot: Slot = Arc::new(Mutex::new(Shared { state: State::Starting, frame: None, stopped: false }));

    assert!(android::wanted_slot(&ctx).is_none());

    android::want(&ctx, Some(slot.clone()));
    let wanted = android::wanted_slot(&ctx).unwrap();

    assert!(Arc::ptr_eq(&wanted, &slot));

    android::asking(&slot, &ctx);
    assert_eq!(lock(&slot).state, State::Starting);

    // As Android's sensor gives it, lying a quarter turn off.
    android::frame(&slot, &ctx, vec![0, 1, 2, 3, 4, 5], 3, 2, 90);
    let shared = lock(&slot);

    assert_eq!(shared.state, State::Running);
    assert_eq!(shared.frame, Some(small().turned(1)));
    drop(shared);

    // A frame that is not what it says is dropped.
    android::frame(&slot, &ctx, vec![0, 1], 3, 2, 0);
    assert_eq!(lock(&slot).frame, Some(small().turned(1)));

    android::failed(&slot, &ctx, "the camera went away".to_string());
    assert_eq!(lock(&slot).state, State::Failed("the camera went away".to_string()));

    // Dropped, nothing more is handed over.
    lock(&slot).stopped = true;
    lock(&slot).frame = None;
    assert!(!hand(&slot, &ctx, small()));
    assert!(lock(&slot).frame.is_none());

    android::want(&ctx, None);
    assert!(android::wanted_slot(&ctx).is_none());
}
