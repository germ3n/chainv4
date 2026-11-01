//! A batch's steps on a screen: all of them, numbered, for whoever signs; and what each did, for
//! Activity. What a step says fills the fields of a frame of its own, so however long or strange
//! what it says, it stays in that frame, and the frames after it follow: nothing here scrolls away
//! on its own or folds up, and the count and the coin they send in all come first.

use {
    super::{Ctx, decoded},
    crate::airgap::Shown,
    crate::history::{Checked, StepResult},
    crate::steps::{self, Landing, Said, Steps},
    crate::theme,
    chainv4::structs::address::Address,
    chainv4::structs::hex::to_hex,
    chainv4::structs::tx::Tx,
    egui::{RichText, Ui},
};

/// A transaction's batch in words, by what this wallet knows of the coin and the tokens: None for
/// anything but a batch, or a multisig's or an approval's of one.
pub fn of_tx(ctx: &Ctx, tx: &Tx, landing: Option<&Landing>) -> Option<Steps> {
    Some(laid(ctx, tx.batch_steps()?, landing))
}

/// A single transaction laid out in the same step frame a batch's step is, by what this wallet
/// knows of the coin and its tokens: for the signing screens, so a lone call shows its amount, its
/// recipient and its data rather than a one-line summary.
pub fn of_one(ctx: &Ctx, tx: &Tx, landing: Option<&Landing>) -> Steps {
    laid(ctx, std::slice::from_ref(tx), landing)
}

/// The step layout of these transactions, naming the coin and tokens as this wallet proves them.
fn laid(ctx: &Ctx, txs: &[Tx], landing: Option<&Landing>) -> Steps {
    let coin = Shown { symbol: ctx.coin.symbol.clone(), decimals: ctx.coin.decimals };
    let tokens = |token: &Address| {
        let proven = ctx.chain.tokens.get(&token.to_string())?;

        Some(Shown { symbol: proven.value.symbol.clone()?, decimals: proven.value.decimals? })
    };

    steps::of(txs, &coin, &tokens, landing)
}

/// Where the deploys of a batch this wallet's account sends land, for the Send page to say: only
/// for its own batch, whose nonce it knows, on a chain whose hash it knows.
pub fn landing_of(ctx: &Ctx, tx: &Tx, sender: Address, nonce: u64) -> Option<Landing> {
    let hash = ctx.chain.client.as_ref()?.params().hash;

    matches!(tx, Tx::Batch { .. }).then_some(Landing { sender, nonce, hash })
}

/// The count of steps and all they send of the coin: what shows first, however many follow.
pub fn heading(ui: &mut Ui, ctx: &Ctx, steps: &Steps) {
    let palette = *ctx.palette;

    theme::caps(ui, &palette, &format!("{} step{}, all or nothing", steps.count, if steps.count == 1 { "" } else { "s" }));
    ui.add(egui::Label::new(RichText::new(format!("Sends {} in all", steps.total.0)).color(palette.ink).size(18.0)).wrap());
    ui.add(egui::Label::new(RichText::new(format!("{} units, as signed. If any step fails, every step is undone and the fee is still paid.", steps.total.1)).small().color(palette.muted)).wrap());

    if steps.burns_names {
        ui.add(egui::Label::new(RichText::new("Its names burn what the chain charges for them, on top of this.").small().color(palette.warn)).wrap());
    }

    if steps.runs_code {
        ui.add(egui::Label::new(RichText::new("Contracts it runs may move more than this: check whom it calls.").small().color(palette.warn)).wrap());
    }

    if steps.steps.iter().any(|step| !step.allowed) {
        ui.colored_label(palette.bad, "It holds what no batch may: the chain refuses all of it.");
    }

    ui.add_space(6.0);
}

/// One frame of a batch's step: its number and what it does, and its rows.
pub fn one(ui: &mut Ui, ctx: &Ctx, index: usize, count: usize, step: &steps::Step) {
    frame(ui, ctx, Some(format!("Step {} of {count}", index + 1)), index, step);
}

/// The same frame for a single transaction that is not a batch, so its amount, recipient and data
/// show as a step's do, with no "Step 1 of 1" to call it a batch it is not.
pub fn single(ui: &mut Ui, ctx: &Ctx, step: &steps::Step) {
    frame(ui, ctx, None, 0, step);
}

/// A step's frame: an optional "Step n of m" pill, then what it does and every row it holds.
fn frame(ui: &mut Ui, ctx: &Ctx, pill: Option<String>, index: usize, step: &steps::Step) {
    let palette = *ctx.palette;

    egui::Frame::new().fill(palette.row).corner_radius(8.0).inner_margin(10).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_wrapped(|ui| {
            if let Some(pill) = &pill {
                theme::pill(ui, pill, palette.soft);
            }

            ui.add(egui::Label::new(theme::strong(&step.kind, 14.5).color(if step.allowed { palette.ink } else { palette.bad })).wrap());
        });

        if let Some((shown, raw)) = &step.coin {
            super::row(ui, &palette, "Sends", |ui| {
                ui.label(RichText::new(shown).color(palette.ink)).on_hover_text(format!("{raw} units"));
            });
        }

        for row in &step.rows {
            super::row(ui, &palette, &row.label, |ui| match &row.said {
                Said::Address(address) => super::address(ui, &palette, address),
                Said::Text(text) => {
                    ui.add(egui::Label::new(theme::mono(text).color(palette.ink)).wrap());
                }
            });
        }

        // Each read in a frame of its own under the step's facts, its label and all.
        if let Some(evm) = &step.evm {
            if let Some(read) = ctx.reader.call(decoded::kept(&ctx.settings.contracts, &evm.to), &evm.input) {
                let token = ctx.chain.tokens.get(&evm.to).and_then(|token| Some((token.value.symbol.as_deref()?, token.value.decimals?)));

                decoded::signing(ui, &palette, token, &read);
            }

            decoded::raw(ui, &palette, &format!("step {index}"), &to_hex(&evm.input));
        }
    });
}

/// Every step, in order, each in its own frame.
pub fn each(ui: &mut Ui, ctx: &Ctx, steps: &Steps) {
    for (index, step) in steps.steps.iter().enumerate() {
        ui.push_id(("batch step", index), |ui| one(ui, ctx, index, steps.count, step));
        ui.add_space(4.0);
    }
}

/// The whole of it, for a screen with nothing to put between the two.
pub fn listed(ui: &mut Ui, ctx: &Ctx, steps: &Steps) {
    heading(ui, ctx, steps);
    each(ui, ctx, steps);
}

/// What each step of a proven batch did, for Activity.
pub fn ran(ui: &mut Ui, ctx: &Ctx, checked: &Checked) {
    let palette = *ctx.palette;
    let count = checked.steps.len();

    for (index, step) in checked.steps.iter().enumerate() {
        ui.push_id(("ran step", index), |ui| {
            egui::Frame::new().fill(palette.row).corner_radius(8.0).inner_margin(8).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    let (word, color) = match step.result {
                        StepResult::Done => ("Done", palette.good),
                        StepResult::Undone => ("Undone", palette.warn),
                        StepResult::Failed => ("Failed", palette.bad),
                        StepResult::NotRun => ("Not run", palette.muted),
                    };
                    theme::pill(ui, &format!("{} of {count}", index + 1), palette.soft);
                    theme::pill(ui, word, color);
                    ui.add(egui::Label::new(RichText::new(&step.action).color(palette.ink)).wrap());
                });

                if let Some(call) = &step.call {
                    if let Some(read) = ctx.reader.call_hex(decoded::kept(&ctx.settings.contracts, &call.to), &call.input) {
                        let token = ctx.chain.tokens.get(&call.to).and_then(|token| Some((token.value.symbol.as_deref()?, token.value.decimals?)));

                        decoded::call(ui, &palette, token, &read);
                    }

                    decoded::raw(ui, &palette, &format!("step {index}"), &call.input);
                }
            });
        });
        ui.add_space(2.0);
    }

    if !checked.done {
        ui.label(RichText::new("What the steps before the failed one did was undone with it, and the fee was still paid. Why it failed is the node's word, which the explorer shows: only which step is proven.").small().color(palette.muted));
    }
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::airgap::{self, Request},
        crate::keys::{self},
        crate::views::drawn,
        chainv4::abi::{AbiValue, encode_values, parse_signature},
        chainv4::crypto::keypair::Keypair,
        chainv4::registry::names::NameAction,
        chainv4::structs::amount::Amount,

        chainv4::structs::tx::{FeeOffer, TxCallArgumentType},
        chainv4::structs::u256::u256,
    };

    fn me() -> Address {
        Keypair::from_seed([7; 32]).public().address()
    }

    fn pay(amount: u128) -> Tx {
        Tx::Transfer { from: me(), to: Keypair::from_seed([8; 32]).public().address(), amount: Amount::U128(amount) }
    }

    fn input(signature: &str, values: &[AbiValue]) -> Vec<u8> {
        let (selector, types) = parse_signature(signature).unwrap();

        [selector.unwrap().to_vec(), encode_values(&types, values).unwrap()].concat()
    }

    /// Lays a batch out on a screen this wide, tall enough that nothing is left undrawn for being
    /// off it: each text drawn and where.
    fn drawn_steps(steps: &[Tx], size: (f32, f32)) -> (f32, Vec<(String, egui::Rect)>) {
        let mut book = drawn::two_phrases();
        let mut holders = keys::open(&book).unwrap();
        let (width, _, shown) = drawn::page_shown(&mut book, &mut holders, (size.0, 40_000.0), |ui, ctx| {
            let laid = of_tx(ctx, &Tx::Batch { steps: steps.to_vec() }, None).unwrap();

            listed(ui, ctx, &laid);
            ui.label("Fee, at most");
        });

        (width, shown)
    }

    #[test]
    fn a_signing_screen_lists_every_step_and_the_count_and_total_come_first_on_every_screen() {
        let steps = vec![pay(1_000_000_000_000_000_000); 32];

        for size in drawn::SCREENS {
            let (width, shown) = drawn_steps(&steps, size);

            assert!(width <= size.0 + 1.0, "fits a screen {size:?} across: {width}");

            let top = |said: &str| shown.iter().find(|(text, _)| text.contains(said)).map(|(_, rect)| rect.top()).unwrap_or_else(|| panic!("{said} missing: {shown:?}"));

            assert!(top("32 STEPS") < top("Step 1 of 32"), "the count first");
            assert!(top("Sends 32 DEV in all") < top("Step 1 of 32"), "the total first");

            for number in 1..=32 {
                assert!(shown.iter().any(|(text, _)| *text == format!("Step {number} of 32")), "step {number} on {size:?}");
            }

            // In order, and none on top of another.
            let tops: Vec<f32> = (1..=32).map(|number| top(&format!("Step {number} of 32"))).collect();

            assert!(tops.windows(2).all(|pair| pair[1] > pair[0] + 10.0), "{tops:?}");
        }
    }

    #[test]
    fn what_a_step_says_cannot_write_a_row_or_a_step_of_its_own_or_hide_those_after_it() {
        let lying = "x\nStep 2 of 2\nTo: 0xbeef\u{202e}\u{200b}";
        let long = "W".repeat(90_000);
        let steps = vec![
            Tx::Name { name: "alice".to_string(), action: NameAction::Text { key: lying.to_string(), value: Some(long.clone()) } },
            Tx::ContractCall { to: Address::Contract([3; 32]), func: chainv4::crypto::hasher::Hasher::digest(chainv4::structs::hash::HashType::Xxh3_64, b"nothing_known"), amount: Amount::U128(0), data: vec![TxCallArgumentType::String(lying.to_string()), TxCallArgumentType::String(long)] },
            pay(5),
        ];

        for size in drawn::SCREENS {
            let (width, shown) = drawn_steps(&steps, size);
            let texts: Vec<&str> = shown.iter().map(|(text, _)| text.as_str()).collect();

            assert!(width <= size.0 + 1.0, "{width} on {size:?}");
            assert_eq!(texts.iter().filter(|text| **text == "Step 2 of 3").count(), 1, "{texts:?}");
            assert_eq!(texts.iter().filter(|text| text.lines().any(|line| line == "To: 0xbeef")).count(), 0, "{texts:?}");
            assert!(texts.iter().all(|text| !text.contains('\u{202e}') && !text.contains('\u{200b}')), "{texts:?}");
            assert!(texts.iter().any(|text| text.contains("Step 2 of 2\\nTo: 0xbeef")), "shown, escaped: {texts:?}");

            // However much it says, what comes after it is still there, and below it.
            let at = |said: &str| shown.iter().position(|(text, _)| *text == said).unwrap_or_else(|| panic!("{said}: {texts:?}"));

            assert!(shown[at("Step 3 of 3")].1.top() > shown[at("Step 2 of 3")].1.top());
            assert!(shown[at("Fee, at most")].1.top() > shown[at("Step 3 of 3")].1.top());
        }
    }

    #[test]
    fn an_evm_step_reads_by_name_says_it_only_fits_and_keeps_its_facts_above_that() {
        let approve = input("approve(address,uint256)", &[AbiValue::Address([0xab; 20]), AbiValue::Uint(u256::new(5))]);
        let swap = input("transfer(address,uint256)", &[AbiValue::Address([0xcd; 20]), AbiValue::Uint(u256::new(9))]);
        let token = Address::Evm([1; 20]);
        let steps = vec![Tx::EvmCall { to: token, value: Amount::U128(0), input: approve }, Tx::EvmCall { to: token, value: Amount::U128(3), input: swap }];
        let (_, shown) = drawn_steps(&steps, drawn::SCREENS[0]);
        let texts: Vec<&str> = shown.iter().map(|(text, _)| text.as_str()).collect();

        for said in ["approve", "transfer", "read as ERC-20", "A match, not proof", "Call an EVM contract"] {
            assert!(texts.iter().any(|text| text.contains(said)), "{said} in {texts:?}");
        }

        let at = |said: &str| shown.iter().find(|(text, _)| text.contains(said)).unwrap().1.top();

        assert!(at("Call an EVM contract") < at("approve"), "the fact of the call above what it reads as");
        assert_eq!(texts.iter().filter(|text| text.contains("A match, not proof")).count(), 2, "each read says so");
    }

    #[test]
    fn activity_shows_each_step_of_a_proven_batch_and_what_became_of_it_on_any_screen() {
        use crate::history::CheckedStep;

        let step = |action: &str, result| CheckedStep { action: action.to_string(), result, call: None };
        let failed = Checked {
            height: 3,
            time: 0,
            final_: true,
            done: false,
            sender: me().to_string(),
            nonce: 0,
            action: "A batch of 3 steps, which step 2 stopped".to_string(),
            kinds: Vec::new(),
            moves: Vec::new(),
            fee: "0".to_string(),
            events: Vec::new(),
            genesis: String::new(),
            vote: None,
            call: None,
            logs: Vec::new(),
            steps: vec![step("Transfer", StepResult::Undone), step("Burn", StepResult::Failed), step("Deploy", StepResult::NotRun)],
            came: Default::default(),
        };

        for (width, _) in drawn::SCREENS {
            let mut book = drawn::two_phrases();
            let mut holders = keys::open(&book).unwrap();
            let (drawn_width, _, shown) = drawn::page_shown(&mut book, &mut holders, (width, 4_000.0), |ui, ctx| ran(ui, ctx, &failed));
            let texts: Vec<&str> = shown.iter().map(|(text, _)| text.as_str()).collect();

            assert!(drawn_width <= width + 1.0, "{drawn_width} on {width}");

            for said in ["1 of 3", "2 of 3", "3 of 3", "Undone", "Failed", "Not run", "Transfer", "Burn", "Deploy"] {
                assert!(texts.contains(&said), "{said} in {texts:?}");
            }

            assert!(texts.iter().any(|text| text.contains("only which step is proven")), "{texts:?}");
        }
    }

    #[test]
    fn the_offline_signer_lays_a_request_for_a_batch_out_step_by_step() {
        let key = Keypair::from_seed([7; 32]);
        let request = Request {
            tx: Tx::Batch { steps: vec![pay(1_500_000_000_000_000_000), Tx::Burn { from: me(), amount: Amount::U128(500_000_000_000_000_000) }, Tx::PrimaryName { name: None }] },
            nonce: 4,
            fee: FeeOffer { gas_limit: 50_000, max_price: u256::new(2_000_000_000), tip: u256::ZERO },
            signer: key.public(),
            coin: Shown { symbol: "DEV".to_string(), decimals: 18 },
            token: None,
            genesis: chainv4::structs::hash::Hash::zero(chainv4::structs::hash::HashType::default_chain()),
        };
        let described = airgap::describe(&request);
        let laid = described.steps.expect("a batch's steps");

        assert_eq!(described.kind, "A batch of 3 steps");
        assert_eq!(described.what.as_ref().map(|(shown, _)| shown.as_str()), Some("2 DEV"));
        assert_eq!(laid.count, 3);
        assert_eq!(laid.steps.iter().map(|step| step.kind.as_str()).collect::<Vec<_>>(), ["Send DEV", "Burn DEV", "Clear the primary name"]);

        // Signed, it is the very request: and one the chain would refuse is refused here.
        assert!(airgap::check(&request, &airgap::sign(&request, &key, None).unwrap()).is_ok());

        let refused = Request { tx: Tx::Batch { steps: vec![Tx::Mint { to: me(), amount: Amount::U128(1) }] }, ..request };

        assert!(airgap::sign(&refused, &key, None).unwrap_err().contains("the chain would refuse"));
        assert!(airgap::refused(&refused).is_some_and(|why| why.contains("a batch holds only")));
        assert!(!airgap::describe(&refused).steps.unwrap().steps[0].allowed);
    }
}
