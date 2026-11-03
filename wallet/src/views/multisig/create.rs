//! Making a multisig, or changing the members of one, as plain steps: who the members are, whether
//! it reaches other chains, which is optional, how many must agree, and a review. Only a key on this
//! chain is asked of a member, of any scheme the chain has: Ed25519, EdBlake, or an Ethereum or
//! Bitcoin account's key. A Bitcoin account key and an EVM address are asked only of a multisig that
//! is to hold coins there too, on EVM chains, on Bitcoin, or on both; this wallet fills in its own
//! accounts' from their words.

use {
    super::{Ctx, follow, member_line_of, other_chains},
    crate::format,
    crate::theme,
    chainv4::crypto::bitcoin::multisig::parse_xpub,
    chainv4::crypto::bitcoin::tx::Network,
    chainv4::crypto::keypair::{KeyType, PublicKey},
    chainv4::crypto::verifiable::Verifiable,
    chainv4::registry::multisig::{MAX_MEMBERS, MultisigMember, MultisigPolicy},
    chainv4::structs::address::{Address, multisig_address},
    chainv4::structs::hex::from_hex,
    chainv4::structs::tx::Tx,
    chainv4::structs::u256::u256,
    crate::views::send::Prepared,
    egui::{RichText, Ui},
    serde_json::Value,
    std::collections::BTreeMap,
};

/// Where a multisig reaches, beyond this chain, where it always is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Reach {
    #[default]
    Here,
    Evm,
    Bitcoin,
    Both,
}

impl Reach {
    /// Whether it asks each member's EVM address.
    pub fn evm(self) -> bool {
        matches!(self, Reach::Evm | Reach::Both)
    }

    /// Whether it asks each member's Bitcoin account key.
    pub fn bitcoin(self) -> bool {
        matches!(self, Reach::Bitcoin | Reach::Both)
    }

    /// Where a multisig whose members name these reaches.
    fn of(bitcoin: bool, evm: bool) -> Reach {
        match (bitcoin, evm) {
            (true, true) => Reach::Both,
            (true, false) => Reach::Bitcoin,
            (false, true) => Reach::Evm,
            (false, false) => Reach::Here,
        }
    }
}

/// The steps, in order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Step {
    #[default]
    Members,
    Reach,
    Threshold,
    Review,
}

impl Step {
    fn number(self) -> usize {
        match self {
            Step::Members => 1,
            Step::Reach => 2,
            Step::Threshold => 3,
            Step::Review => 4,
        }
    }

    fn next(self) -> Step {
        match self {
            Step::Members => Step::Reach,
            Step::Reach => Step::Threshold,
            Step::Threshold | Step::Review => Step::Review,
        }
    }

    fn before(self) -> Option<Step> {
        match self {
            Step::Members => None,
            Step::Reach => Some(Step::Members),
            Step::Threshold => Some(Step::Reach),
            Step::Review => Some(Step::Threshold),
        }
    }
}

/// One member as typed. What a member gives for another chain is kept while its reach is off, so
/// turning it on again finds it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Row {
    pub key: String,
    pub evm: String,
    pub xpub: String,
}

impl Row {
    /// A member from what Pair and chainv4 multisig write: KEY, KEY,,EVM or KEY,XPUB,EVM. A key alone
    /// is a key alone.
    pub fn from_line(line: &str) -> Row {
        let mut parts = line.split(',').map(str::trim);
        let key = parts.next().unwrap_or_default().to_string();
        let xpub = parts.next().unwrap_or_default().to_string();
        let evm = parts.next().unwrap_or_default().to_string();

        Row { key, evm, xpub }
    }

    /// A row from what the node lists of a member.
    fn from_listed(member: &Value) -> Row {
        let text = |name: &str| member[name].as_str().unwrap_or_default().to_string();

        Row { key: text("key"), evm: text("evm"), xpub: text("bitcoin") }
    }
}

/// The steps' state: what is typed, on which step, for a new multisig or for changing one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Wizard {
    pub step: Step,
    pub rows: Vec<Row>,
    pub reach: Reach,
    pub testnet: bool,
    pub threshold: u32,
    /// The multisig whose members change, if it is not a new one.
    pub changing: Option<String>,
}

/// What is wrong with what a step asks, by the row and its field, or None for the step as a whole.
pub type Errors = BTreeMap<(Option<usize>, &'static str), String>;

/// What the first step asks: one key at least, 20 at most, each one a key of any scheme, on its
/// curve, each once.
pub fn member_errors(rows: &[Row]) -> Errors {
    let mut errors = Errors::new();
    let mut seen = Vec::new();

    for (index, row) in rows.iter().enumerate() {
        match row.key.trim().parse::<PublicKey>() {
            _ if row.key.trim().is_empty() => {
                errors.insert((Some(index), "key"), "Required: this member's key on this chain.".to_string());
            }
            // An Ethereum or Bitcoin account's address is a hash of its key, which cannot be read
            // back from it.
            Err(_) if row.key.trim().parse::<Address>().is_ok() => {
                errors.insert((Some(index), "key"), ADDRESS_NOT_KEY.to_string());
            }
            Err(_) => {
                errors.insert((Some(index), "key"), "Not a key. It looks like ed25519:0x and 64 hex digits, edblake:0x and 64, secp256k1:0x or bitcoin:0x and 66, or a member's whole line.".to_string());
            }
            Ok(key) if key.verify().is_err() => {
                errors.insert((Some(index), "key"), "Not a key on its curve: check it was copied whole.".to_string());
            }
            Ok(key) if seen.contains(&key) => {
                errors.insert((Some(index), "key"), "This key is already a member: each member is one key.".to_string());
            }
            Ok(key) => seen.push(key),
        }
    }

    if rows.is_empty() {
        errors.insert((None, "members"), "A multisig has one member at least.".to_string());
    }

    if rows.len() > MAX_MEMBERS {
        errors.insert((None, "members"), format!("A multisig has at most {MAX_MEMBERS} members."));
    }

    errors
}

/// What an address given for a member's key is told.
const ADDRESS_NOT_KEY: &str = "That is an address, not a key. For an Ethereum or Bitcoin account the member gives its public key, secp256k1:0x… or bitcoin:0x… and 66 hex digits, which its wallet's Pair page shows under As a multisig's member: an address is a hash of its key, and the key cannot be read back from it.";

/// What the second step asks, when it is asked at all: every member's address on the chains it
/// reaches, each once, and account keys of the network picked.
pub fn reach_errors(rows: &[Row], reach: Reach, testnet: bool) -> Errors {
    let mut errors = Errors::new();
    let (mut evm_seen, mut xpub_seen): (Vec<String>, Vec<String>) = Default::default();

    if reach == Reach::Here {
        return errors;
    }

    for (index, row) in rows.iter().enumerate() {
        let evm = row.evm.trim();

        match from_hex(evm).and_then(|bytes| <[u8; 20]>::try_from(bytes).ok()) {
            _ if !reach.evm() => {}
            _ if evm.is_empty() => {
                errors.insert((Some(index), "evm"), "Required once it reaches EVM chains, of every member.".to_string());
            }
            None => {
                errors.insert((Some(index), "evm"), "Not an EVM address: 0x and 40 hex digits.".to_string());
            }
            Some(address) if address == [0; 20] => {
                errors.insert((Some(index), "evm"), "Not the zero address.".to_string());
            }
            Some(_) if evm_seen.iter().any(|known| known.eq_ignore_ascii_case(evm)) => {
                errors.insert((Some(index), "evm"), "Another member has named this address.".to_string());
            }
            Some(_) => evm_seen.push(evm.to_string()),
        }

        if !reach.bitcoin() {
            continue;
        }

        let xpub = row.xpub.trim();

        match parse_xpub(xpub) {
            _ if xpub.is_empty() => {
                errors.insert((Some(index), "xpub"), "Required once it reaches Bitcoin, of every member.".to_string());
            }
            Err(_) if xpub.parse::<Address>().is_ok() || xpub.starts_with("bc1") || xpub.starts_with("tb1") || xpub.starts_with('1') || xpub.starts_with('3') => {
                errors.insert((Some(index), "xpub"), "That is a Bitcoin address. A multisig needs the member's account key, [fingerprint/48h/0h/0h/2h]xpub…, from which its addresses are made with every other member's.".to_string());
            }
            Err(why) => {
                errors.insert((Some(index), "xpub"), format!("Not a Bitcoin account key with its origin, [73c5da0a/48h/0h/0h/2h]xpub…: {why}"));
            }
            Ok((_, network)) if (network == Network::Test) != testnet => {
                let (key, wanted) = if testnet { ("xpub", "Bitcoin's test networks") } else { ("tpub", "Bitcoin") };
                errors.insert((Some(index), "xpub"), format!("This {key} is for the other network: the multisig is on {wanted}."));
            }
            Ok(_) if xpub_seen.iter().any(|known| known == xpub) => {
                errors.insert((Some(index), "xpub"), "Another member has named this key.".to_string());
            }
            Ok(_) => xpub_seen.push(xpub.to_string()),
        }
    }

    errors
}

/// What the third step asks: at least one member, at most all.
pub fn threshold_error(threshold: u32, members: usize) -> Option<String> {
    (threshold == 0 || threshold as usize > members).then(|| format!("Between 1 and {members}."))
}

/// How many must agree, as "2 of 3".
pub fn of(threshold: u32, members: usize) -> String {
    format!("{threshold} of {members}")
}

/// What a threshold means, in a sentence.
pub fn threshold_words(threshold: u32, members: usize) -> String {
    match (threshold, members) {
        (1, 1) => "One member alone: it acts as that key does, and can take more members later.".to_string(),
        (1, members) => format!("Any one of the {members} can send anything alone. Handy, but one stolen key is enough to empty it."),
        (threshold, members) if threshold as usize == members => format!("All {members} must agree. Safest against a stolen key, but one lost key locks it for good."),
        (threshold, members) => format!("Any {threshold} of the {members} must agree before it sends anything. {} can lose a key and the rest can still act.", members - threshold as usize),
    }
}

/// A threshold to start from: most of them, so one key alone does nothing where it can be helped.
fn suggested(members: usize) -> u32 {
    (members / 2 + 1) as u32
}

/// The policy all the steps make, or the first thing that is wrong with it, as the chain checks it.
pub fn policy(wizard: &Wizard) -> Result<MultisigPolicy, String> {
    let first = |errors: Errors| errors.into_values().next();

    if let Some(why) = first(member_errors(&wizard.rows)).or_else(|| first(reach_errors(&wizard.rows, wizard.reach, wizard.testnet))).or_else(|| threshold_error(wizard.threshold, wizard.rows.len())) {
        return Err(why);
    }

    let member = |row: &Row| -> Result<MultisigMember, String> {
        let key = row.key.trim().parse::<PublicKey>().map_err(|_| "Not a key.".to_string())?;
        let evm = match wizard.reach.evm() {
            false => None,
            true => Some(from_hex(row.evm.trim()).and_then(|bytes| bytes.try_into().ok()).ok_or("Not an EVM address.")?),
        };
        let bitcoin = match wizard.reach.bitcoin() {
            true => Some(parse_xpub(row.xpub.trim()).map_err(|why| why.to_string())?.0),
            false => None,
        };

        Ok(MultisigMember { key, bitcoin, evm })
    };
    let members = wizard.rows.iter().map(member).collect::<Result<Vec<_>, _>>()?;
    let policy = MultisigPolicy { members, threshold: wizard.threshold, testnet: wizard.reach.bitcoin() && wizard.testnet };

    match policy.problem() {
        Some(problem) => Err(format!("Not a multisig: {problem}.")),
        None => Ok(policy),
    }
}

/// Whether a step is done: what it asks reads.
fn step_errors(wizard: &Wizard) -> Errors {
    match wizard.step {
        Step::Members => member_errors(&wizard.rows),
        Step::Reach => reach_errors(&wizard.rows, wizard.reach, wizard.testnet),
        Step::Threshold => threshold_error(wizard.threshold, wizard.rows.len()).into_iter().map(|why| ((None, "threshold"), why)).collect(),
        Step::Review => Errors::new(),
    }
}

impl Wizard {
    /// Changing a multisig's members: the steps, filled in with what the node lists.
    pub fn changing(account: &str, value: &Value) -> Wizard {
        let rows: Vec<Row> = value["members"].as_array().into_iter().flatten().map(Row::from_listed).collect();
        let reach = Reach::of(rows.iter().any(|row| !row.xpub.is_empty()), rows.iter().any(|row| !row.evm.is_empty()));

        Wizard { rows, reach, testnet: value["testnet"].as_bool().unwrap_or(false), threshold: value["threshold"].as_u64().unwrap_or(1) as u32, changing: Some(account.to_string()), ..Wizard::default() }
    }

    /// Whether a change of members changes anything.
    fn same_as(&self, value: &Value) -> bool {
        let listed: Vec<Row> = value["members"].as_array().into_iter().flatten().map(Row::from_listed).collect();
        let trimmed = |row: &Row| Row { key: row.key.trim().to_string(), evm: if self.reach.evm() { row.evm.trim().to_string() } else { String::new() }, xpub: if self.reach.bitcoin() { row.xpub.trim().to_string() } else { String::new() } };

        self.rows.iter().map(trimmed).eq(listed.iter().map(trimmed)) && u64::from(self.threshold) == value["threshold"].as_u64().unwrap_or(0)
    }
}

/// What the steps come to when someone goes through with them.
pub enum Done {
    /// Nothing yet.
    Going,
    /// Put away.
    Cancelled,
    /// A new multisig's transaction, for Send to sign.
    Make(Prepared),
    /// A change of the members, as the multisig's own transaction, for its members to back.
    Change(Address, Tx),
}

/// Shows the step it is on, and what comes of it.
pub fn show(ui: &mut Ui, ctx: &mut Ctx, wizard: &mut Wizard, listed: Option<&Value>) -> Done {
    let palette = *ctx.palette;
    let changing = wizard.changing.is_some();
    let mut done = Done::Going;

    theme::card(ui, &palette, |ui| {
        let title = match wizard.step {
            Step::Members => "Who are the members?",
            Step::Reach => "Should it reach other chains? (Optional)",
            Step::Threshold => "How many must agree?",
            Step::Review => "Review",
        };
        theme::caps(ui, &palette, &format!("{} · step {} of 4", if changing { "Change its members" } else { "Make a multisig" }, wizard.step.number()));
        ui.label(theme::strong(title, 16.0).color(palette.ink));
        ui.add_space(4.0);

        let errors = step_errors(wizard);

        match wizard.step {
            Step::Members => members(ui, ctx, wizard, &errors),
            Step::Reach => reach(ui, ctx, wizard, &errors),
            Step::Threshold => threshold(ui, &palette, wizard),
            Step::Review => done = review(ui, ctx, wizard, listed),
        }

        ui.add_space(10.0);
        ui.horizontal_wrapped(|ui| {
            if wizard.step != Step::Review && theme::primary(ui, &palette, "Next", errors.is_empty()).clicked() {
                if wizard.step == Step::Members && wizard.threshold == 0 {
                    wizard.threshold = suggested(wizard.rows.len());
                }

                wizard.threshold = wizard.threshold.min(wizard.rows.len() as u32);
                wizard.step = wizard.step.next();
            }

            if let Some(before) = wizard.step.before()
                && theme::quiet(ui, &palette, "Back").clicked()
            {
                wizard.step = before;
            }

            if theme::quiet(ui, &palette, "Cancel").clicked() {
                done = Done::Cancelled;
            }
        });
    });

    done
}

fn hint(ui: &mut Ui, palette: &theme::Palette, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).small().color(palette.muted)).wrap());
}

fn bad(ui: &mut Ui, palette: &theme::Palette, why: Option<&String>) {
    if let Some(why) = why {
        ui.add(egui::Label::new(RichText::new(why).small().color(palette.bad)).wrap());
    }
}

/// The first step: a key for each member.
fn members(ui: &mut Ui, ctx: &mut Ctx, wizard: &mut Wizard, errors: &Errors) {
    let palette = *ctx.palette;
    let mine = wallet_keys(ctx);

    hint(ui, &palette, "Each member is one key on this chain, of any kind it has: Ed25519, EdBlake, Ethereum's or Bitcoin's. Ask each for theirs: for most accounts it is the ed25519:0x… address on their Receive page. An Ethereum or Bitcoin account is named by its public key, not its address: Pair shows it, under As a multisig's member.");
    ui.add_space(4.0);

    let mut gone = None;

    for (index, row) in wizard.rows.iter_mut().enumerate() {
        ui.push_id(index, |ui| {
            let ours = mine.iter().find(|own| own.key.to_string() == row.key.trim()).map(|own| format!(", {}", own.name)).unwrap_or_default();
            let label = format!("Member {}{ours} · required", index + 1);
            ui.label(RichText::new(label).small().color(palette.soft));

            let remove = theme::beside(
                ui,
                |ui| theme::quiet(ui, &palette, "Remove").clicked(),
                |ui, width| {
                    ui.add(egui::TextEdit::singleline(&mut row.key).hint_text("ed25519:0x…").font(egui::TextStyle::Monospace).desired_width(width));
                },
            );

            if remove {
                gone = Some(index);
            }

            // A member's whole line pasted in: its parts go to their fields.
            if row.key.contains(',') {
                *row = Row::from_line(&row.key);
            }

            // One of this wallet's own addresses typed or pasted: its key is known here.
            if let Some(own) = mine.iter().find(|own| own.key.address().to_string() == row.key.trim() && own.key.to_string() != row.key.trim()) {
                row.key = own.key.to_string();
            }

            if !row.key.is_empty() {
                bad(ui, &palette, errors.get(&(Some(index), "key")));
            }
        });
    }

    if let Some(index) = gone {
        wizard.rows.remove(index);
    }

    // Any of this wallet's keys not yet a member, one of each scheme of each account.
    let open: Vec<&Own> = mine.iter().filter(|own| !wizard.rows.iter().any(|row| row.key.trim() == own.key.to_string())).collect();
    let mut added = None;

    if !open.is_empty() {
        egui::ComboBox::from_id_salt("add own key").selected_text("Add one of this wallet's keys…").width(ui.available_width()).show_ui(ui, |ui| {
            for own in &open {
                if ui.selectable_label(false, &own.name).clicked() {
                    added = Some(own.line(ctx, wizard.testnet));
                }
            }
        });
    }

    if let Some(line) = added {
        wizard.rows.push(Row::from_line(&line));
    }

    if theme::quiet(ui, &palette, "Add a member").clicked() {
        wizard.rows.push(Row::default());
    }

    // None yet is what Next waits for, not a mistake.
    if !wizard.rows.is_empty() {
        bad(ui, &palette, errors.get(&(None, "members")));
    }
}

/// One of this wallet's keys, any account's, any scheme's: what a member's row can be filled from.
pub struct Own {
    /// Its account, by its place among the holders.
    pub holder: usize,
    pub key: PublicKey,
    /// The account's name and the key's scheme.
    pub name: String,
}

impl Own {
    /// The member's line it makes: the key, and its account's EVM address and words' xpub.
    fn line(&self, ctx: &Ctx, testnet: bool) -> String {
        let holder = &ctx.holders[self.holder];

        member_line_of(holder, &self.key, crate::keys::seed_of(ctx.book, holder), testnet)
    }
}

/// Every key this wallet holds or watches, account by account, scheme by scheme.
fn wallet_keys(ctx: &Ctx) -> Vec<Own> {
    ctx.holders
        .iter()
        .enumerate()
        .flat_map(|(index, holder)| holder.publics.iter().map(move |key| Own { holder: index, key: *key, name: format!("{} · {}", holder.name, crate::keys::label(key.key_type())) }))
        .collect()
}

/// The account in use's keys on this chain, as members name them.
fn account_keys(ctx: &Ctx) -> Vec<String> {
    ctx.holder().map(|holder| holder.publics.iter().map(PublicKey::to_string).collect()).unwrap_or_default()
}

/// The second step: whether it reaches other chains, and if so each member's keys there.
fn reach(ui: &mut Ui, ctx: &mut Ctx, wizard: &mut Wizard, errors: &Errors) {
    let palette = *ctx.palette;

    hint(ui, &palette, "Most multisigs need nothing here. A Bitcoin address is not needed to make one.");
    ui.add_space(2.0);
    theme::segmented_rows(ui, &palette, &mut wizard.reach, &[(Reach::Here, "Only this chain"), (Reach::Evm, "Also EVM chains"), (Reach::Bitcoin, "Also Bitcoin"), (Reach::Both, "Also Bitcoin and EVM chains")]);
    ui.add_space(2.0);

    match wizard.reach {
        Reach::Here => hint(ui, &palette, "It holds coins and tokens here, and sends anything an account sends. Its members can add other chains later, by changing its members."),
        Reach::Evm => hint(ui, &palette, "Each member gives its EVM address, to hold coins on Ethereum-like chains too: the same members control one contract, at one address on every such chain. Every member gives one, or none does."),
        Reach::Bitcoin => hint(ui, &palette, "Each member gives its Bitcoin account key, to hold bitcoin too, in a multisig other Bitcoin wallets can co-sign. Every member gives one, or none does."),
        Reach::Both => hint(ui, &palette, "Each member gives its Bitcoin account key and its EVM address, to hold bitcoin in a multisig other Bitcoin wallets co-sign, and coins on EVM chains. Every member gives both, or none does."),
    }

    if wizard.reach.bitcoin() {
        let was = wizard.testnet;
        ui.add_space(2.0);
        theme::segmented(ui, &palette, &mut wizard.testnet, &[(false, "Bitcoin"), (true, "Bitcoin's test networks")]);

        // This wallet's own keys follow the network; others' are their own to give.
        if was != wizard.testnet {
            for own in wallet_keys(ctx) {
                let (_, xpub) = other_chains(&ctx.holders[own.holder], crate::keys::seed_of(ctx.book, &ctx.holders[own.holder]), wizard.testnet);

                for row in wizard.rows.iter_mut().filter(|row| row.key.trim() == own.key.to_string()) {
                    row.xpub = xpub.clone().unwrap_or_default();
                }
            }
        }
    }

    if wizard.reach == Reach::Here {
        return;
    }

    if wizard.reach.bitcoin() {
        hint(ui, &palette, "A Bitcoin account key is not a Bitcoin address: it is the member's multisig account key, xpub… (tpub… on the test networks) with its origin, such as [73c5da0a/48h/0h/0h/2h]xpub…, from which the multisig's addresses are made with every other member's. A member finds theirs on this wallet's Pair page, under As a multisig's member, or in Sparrow, Nunchuk or a hardware wallet as its multisig key for native segwit, BIP48. Each member's comes from its own words: two accounts of the same words have the same one.");
    }

    if wizard.reach.evm() {
        hint(ui, &palette, "An EVM address is the member's Ethereum account, 0x… and 40 hex digits: its key signs the contract's calls. In this wallet it is an account's Ethereum address on Receive.");
    }

    let mine = wallet_keys(ctx);

    for (index, row) in wizard.rows.iter_mut().enumerate() {
        ui.add_space(6.0);
        ui.push_id(index, |ui| {
            let ours = mine.iter().find(|own| own.key.to_string() == row.key.trim());
            let name = match ours {
                Some(own) => format!("Member {}, {}", index + 1, own.name),
                None => format!("Member {} · {}", index + 1, format::short(row.key.trim())),
            };
            ui.label(theme::strong(name, 13.0).color(palette.soft));

            // This wallet's own: what its words make, in one tap.
            if let Some(own) = ours {
                let holder = &ctx.holders[own.holder];
                let (evm, xpub) = other_chains(holder, crate::keys::seed_of(ctx.book, holder), wizard.testnet);
                let filled = (!wizard.reach.evm() || evm.as_deref() == Some(row.evm.trim())) && (!wizard.reach.bitcoin() || xpub.as_deref() == Some(row.xpub.trim()));
                let has = (!wizard.reach.evm() || evm.is_some()) && (!wizard.reach.bitcoin() || xpub.is_some());

                if !filled && has && theme::quiet(ui, &palette, &format!("Fill in from {}", holder.name)).clicked() {
                    if wizard.reach.evm() {
                        row.evm = evm.unwrap_or_default();
                    }

                    if wizard.reach.bitcoin() {
                        row.xpub = xpub.unwrap_or_default();
                    }
                }

                if !has {
                    hint(ui, &palette, "This account comes from no words this wallet holds, so its keys elsewhere are not known here: fill them in by hand.");
                }
            }

            if wizard.reach.evm() {
                ui.label(RichText::new("EVM address · required for this choice").small().color(palette.soft));
                ui.add(egui::TextEdit::singleline(&mut row.evm).hint_text("0x…").font(egui::TextStyle::Monospace).desired_width(f32::INFINITY));
                bad(ui, &palette, errors.get(&(Some(index), "evm")));
            }

            if wizard.reach.bitcoin() {
                ui.label(RichText::new("Bitcoin account key, not an address · required for this choice").small().color(palette.soft));
                ui.add(egui::TextEdit::singleline(&mut row.xpub).hint_text("[73c5da0a/48h/0h/0h/2h]xpub…").font(egui::TextStyle::Monospace).desired_width(f32::INFINITY));
                bad(ui, &palette, errors.get(&(Some(index), "xpub")));
            }
        });
    }
}

/// The third step: so many of so many.
fn threshold(ui: &mut Ui, palette: &theme::Palette, wizard: &mut Wizard) {
    let count = wizard.rows.len();
    wizard.threshold = wizard.threshold.clamp(1, count.max(1) as u32);

    ui.horizontal(|ui| {
        if ui.add_enabled(wizard.threshold > 1, egui::Button::new(RichText::new("−").size(18.0))).clicked() {
            wizard.threshold -= 1;
        }

        ui.label(theme::strong(of(wizard.threshold, count), 22.0).color(palette.ink));

        if ui.add_enabled((wizard.threshold as usize) < count, egui::Button::new(RichText::new("+").size(18.0))).clicked() {
            wizard.threshold += 1;
        }
    });
    ui.add(egui::Label::new(RichText::new(threshold_words(wizard.threshold, count)).color(palette.soft)).wrap());
}

/// The last step: all of it in words, and going through with it.
fn review(ui: &mut Ui, ctx: &mut Ctx, wizard: &mut Wizard, listed: Option<&Value>) -> Done {
    let palette = *ctx.palette;
    let mine = account_keys(ctx);
    let own = wallet_keys(ctx);
    let policy = policy(wizard);

    super::super::row(ui, &palette, "Who must agree", |ui| {
        ui.label(RichText::new(format!("{} members", of(wizard.threshold, wizard.rows.len()))).color(palette.ink));
    });

    for (index, row) in wizard.rows.iter().enumerate() {
        let you = own.iter().find(|own| own.key.to_string() == row.key.trim()).map(|own| format!(" · {}", own.name)).unwrap_or_default();
        ui.add(egui::Label::new(RichText::new(format!("{}. {}{you}", index + 1, format::short(row.key.trim()))).monospace().color(palette.soft)).wrap());
    }

    super::super::row(ui, &palette, "Where", |ui| {
        ui.label(match (wizard.reach, wizard.testnet) {
            (Reach::Here, _) => "This chain only",
            (Reach::Evm, _) => "This chain and EVM chains",
            (Reach::Bitcoin, false) => "This chain and Bitcoin",
            (Reach::Bitcoin, true) => "This chain and Bitcoin's test networks",
            (Reach::Both, false) => "This chain, EVM chains and Bitcoin",
            (Reach::Both, true) => "This chain, EVM chains and Bitcoin's test networks",
        });
    });

    if let Err(why) = &policy {
        ui.add(egui::Label::new(RichText::new(why).color(palette.bad)).wrap());

        return Done::Going;
    }

    let Ok(policy) = policy else {
        return Done::Going;
    };

    if let (Some(account), Some(value)) = (wizard.changing.clone(), listed) {
        let same = wizard.same_as(value);

        ui.add_space(4.0);

        match same {
            true => hint(ui, &palette, "That is what it is already: change something, or cancel."),
            false => hint(ui, &palette, "Its current members decide this, as any of its transactions: enough of them must back it. Its address stays. Anything waiting for approval is dropped once it runs. On EVM chains its contract keeps its owners until they sign the change, which this page offers."),
        }

        ui.add_space(6.0);

        if !same && theme::primary(ui, &palette, "Propose this change", true).clicked() {
            let Ok(account) = account.parse::<Address>() else {
                return Done::Cancelled;
            };

            return Done::Change(account, Tx::ChangeMultisig { members: policy.members, threshold: wizard.threshold, testnet: policy.testnet });
        }

        return Done::Going;
    }

    // Who sends it pays, and has no say after unless a member.
    let from = ctx.holder().and_then(|holder| holder.addresses().into_iter().find(|address| crate::keys::scheme_of(address) != Some(KeyType::Bitcoin)));
    let nonce = from.and_then(|from| ctx.chain.account(&from.to_string()).map(|proven| proven.value.nonce));
    let genesis = ctx.chain.genesis;

    let (Some(from), Some(nonce), Some(genesis)) = (from, nonce, genesis) else {
        hint(ui, &palette, "Waiting for the light client to prove this account, which sends the transaction.");

        return Done::Going;
    };
    let address = multisig_address(&from, nonce, genesis.algorithm());

    super::super::row(ui, &palette, "It will be at", |ui| super::super::address(ui, &palette, &address.to_string()));
    ui.add_space(4.0);
    hint(ui, &palette, &format!(
        "This account sends the transaction and pays its fee{}. A multisig pays its own fees afterwards, so send it some {} before it has to act.",
        if mine.iter().any(|key| wizard.rows.iter().any(|row| row.key.trim() == key.as_str())) { "" } else { ", and has no say afterwards, not being a member" },
        ctx.coin.symbol
    ));
    ui.add_space(6.0);

    if theme::primary(ui, &palette, "Continue to Send", true).clicked() {
        follow(ctx, &address.to_string());

        return Done::Make(Prepared {
            from,
            tx: Tx::CreateMultisig { members: policy.members, threshold: wizard.threshold, testnet: policy.testnet },
            what: format!("Make a multisig, {} members", of(wizard.threshold, wizard.rows.len())),
            about: format::short(&address.to_string()),
            coin: u256::ZERO,
        });
    }

    Done::Going
}

#[cfg(test)]
mod tests {
    use {
        super::super::network,
        super::*,
        chainv4::crypto::bitcoin::multisig::xpub_text,
        chainv4::crypto::keypair::Keypair,
        chainv4::crypto::mnemonic::{Phrase, Seed},
    };

    fn key(byte: u8) -> String {
        Keypair::from_seed([byte; 32]).public().to_string()
    }

    fn seed(words: &str) -> Seed {
        Phrase::parse(words).unwrap().seed("")
    }

    fn xpub(testnet: bool) -> String {
        let network = network(testnet);

        xpub_text(&seed("abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about").multisig_account(network.coin()), network)
    }

    fn other_xpub(testnet: bool) -> String {
        let network = network(testnet);

        xpub_text(&seed("legal winner thank year wave sausage worth useful legal winner thank yellow").multisig_account(network.coin()), network)
    }

    fn rows(keys: &[u8]) -> Vec<Row> {
        keys.iter().map(|byte| Row { key: key(*byte), ..Row::default() }).collect()
    }

    #[test]
    fn a_key_each_is_all_a_multisig_needs() {
        let wizard = Wizard { rows: rows(&[1, 2, 3]), threshold: 2, ..Wizard::default() };
        let policy = policy(&wizard).expect("no Bitcoin address, no EVM address");

        assert_eq!((policy.threshold, policy.members.len(), policy.testnet), (2, 3, false));
        assert!(policy.members.iter().all(|member| member.bitcoin.is_none() && member.evm.is_none()));
    }

    #[test]
    fn what_a_step_asks_says_what_is_wrong_with_each_member() {
        assert!(member_errors(&rows(&[1, 2])).is_empty());
        assert_eq!(member_errors(&[])[&(None, "members")], "A multisig has one member at least.");
        assert!(member_errors(&[Row::default()])[&(Some(0), "key")].starts_with("Required"));
        assert!(member_errors(&[Row { key: "nobody".to_string(), ..Row::default() }])[&(Some(0), "key")].starts_with("Not a key"));
        assert!(member_errors(&rows(&[1, 1]))[&(Some(1), "key")].contains("already a member"));
        assert!(member_errors(&vec![Row { key: key(1), ..Row::default() }; 21]).contains_key(&(None, "members")));

        let too_many: Vec<Row> = (1..=21).flat_map(|byte| rows(&[byte])).collect();
        assert!(member_errors(&too_many)[&(None, "members")].contains("at most 20"));
    }

    #[test]
    fn other_chains_ask_every_member_only_once_they_are_picked() {
        let evm = |byte: u8| format!("0x{}", format!("{byte:02x}").repeat(20));
        let bare = rows(&[1, 2]);

        assert!(reach_errors(&bare, Reach::Here, false).is_empty(), "nothing asked of a multisig on this chain alone");

        let errors = reach_errors(&bare, Reach::Evm, false);
        assert!(errors[&(Some(0), "evm")].starts_with("Required") && errors[&(Some(1), "evm")].starts_with("Required"));
        assert!(!errors.contains_key(&(Some(0), "xpub")), "no Bitcoin key for EVM chains alone");

        let mut filled = bare.clone();
        filled[0].evm = evm(0x11);
        filled[1].evm = evm(0x22);
        assert!(reach_errors(&filled, Reach::Evm, false).is_empty());
        assert!(reach_errors(&filled, Reach::Both, false).contains_key(&(Some(1), "xpub")), "Bitcoin asks for more");

        filled[0].xpub = xpub(false);
        filled[1].xpub = other_xpub(false);
        assert_eq!(reach_errors(&filled, Reach::Both, false), Errors::new());
        assert!(reach_errors(&filled, Reach::Both, true)[&(Some(0), "xpub")].contains("other network"), "a Bitcoin key on the test networks");

        filled[1].xpub = filled[0].xpub.clone();
        filled[1].evm = filled[0].evm.to_uppercase().replace("0X", "0x");
        let errors = reach_errors(&filled, Reach::Both, false);
        assert!(errors[&(Some(1), "evm")].contains("Another member") && errors[&(Some(1), "xpub")].contains("Another member"));

        filled[0].evm = "0x12".to_string();
        filled[0].xpub = "xpub123".to_string();
        let errors = reach_errors(&filled, Reach::Both, false);
        assert!(errors[&(Some(0), "evm")].starts_with("Not an EVM address") && errors[&(Some(0), "xpub")].starts_with("Not a Bitcoin account key"));
    }

    #[test]
    fn a_reach_makes_its_policy_and_the_rest_is_left_out() {
        let evm = |byte: u8| format!("0x{}", format!("{byte:02x}").repeat(20));
        let mut wizard = Wizard { rows: rows(&[1, 2]), threshold: 2, ..Wizard::default() };

        // What a member gave for another chain stays out while it is not picked.
        wizard.rows[0].evm = evm(0x11);
        wizard.rows[0].xpub = xpub(false);
        assert!(policy(&wizard).unwrap().members.iter().all(|member| member.evm.is_none() && member.bitcoin.is_none()));

        wizard.reach = Reach::Evm;
        wizard.rows[1].evm = evm(0x22);
        let made = policy(&wizard).unwrap();
        assert_eq!(made.members[1].evm, Some([0x22; 20]));
        assert!(made.members.iter().all(|member| member.bitcoin.is_none()));

        wizard.reach = Reach::Both;
        wizard.rows[1].xpub = other_xpub(false);
        let made = policy(&wizard).unwrap();
        assert!(made.members.iter().all(|member| member.bitcoin.is_some() && member.evm.is_some()));
        assert!(!made.testnet);

        wizard.threshold = 3;
        assert_eq!(policy(&wizard).unwrap_err(), "Between 1 and 2.");
    }

    #[test]
    fn a_threshold_says_what_it_means() {
        assert_eq!(of(2, 3), "2 of 3");
        assert_eq!(threshold_error(0, 3).as_deref(), Some("Between 1 and 3."));
        assert_eq!(threshold_error(4, 3).as_deref(), Some("Between 1 and 3."));
        assert_eq!(threshold_error(3, 3), None);
        assert!(threshold_words(1, 3).contains("Any one of the 3"));
        assert!(threshold_words(3, 3).contains("one lost key locks it"));
        assert_eq!(threshold_words(2, 3), "Any 2 of the 3 must agree before it sends anything. 1 can lose a key and the rest can still act.");
        assert_eq!((suggested(1), suggested(2), suggested(3), suggested(5)), (1, 2, 2, 3));
    }

    #[test]
    fn a_member_s_line_goes_to_its_fields() {
        assert_eq!(Row::from_line(&key(1)), Row { key: key(1), ..Row::default() });
        assert_eq!(Row::from_line(&format!("{},,0x11", key(1))), Row { key: key(1), evm: "0x11".to_string(), xpub: String::new() });
        assert_eq!(Row::from_line(&format!(" {} , {} , 0x11 ", key(1), xpub(false))), Row { key: key(1), evm: "0x11".to_string(), xpub: xpub(false) });
    }

    #[test]
    fn changing_a_multisig_starts_from_what_it_is() {
        let listed = serde_json::json!({
            "threshold": 2,
            "testnet": true,
            "members": [
                { "key": key(1), "evm": format!("0x{}", "11".repeat(20)), "bitcoin": xpub(true) },
                { "key": key(2), "evm": format!("0x{}", "22".repeat(20)), "bitcoin": other_xpub(true) },
            ],
        });
        let mut wizard = Wizard::changing("multisig:0x01", &listed);

        assert_eq!((wizard.reach, wizard.testnet, wizard.threshold, wizard.rows.len()), (Reach::Both, true, 2, 2));
        assert_eq!(wizard.changing.as_deref(), Some("multisig:0x01"));
        assert!(wizard.same_as(&listed), "nothing changed yet");
        assert!(policy(&wizard).is_ok());

        wizard.threshold = 1;
        assert!(!wizard.same_as(&listed));

        wizard.threshold = 2;
        wizard.rows[0].key = key(3);
        assert!(!wizard.same_as(&listed));

        let here = Wizard::changing("multisig:0x02", &serde_json::json!({ "threshold": 1, "members": [{ "key": key(1) }] }));
        assert_eq!(here.reach, Reach::Here);
    }

    #[test]
    fn the_steps_go_in_order_and_back() {
        assert_eq!(Step::Members.next(), Step::Reach);
        assert_eq!(Step::Reach.next(), Step::Threshold);
        assert_eq!(Step::Threshold.next(), Step::Review);
        assert_eq!(Step::Review.before(), Some(Step::Threshold));
        assert_eq!(Step::Members.before(), None);

        let mut wizard = Wizard { rows: rows(&[1, 2]), ..Wizard::default() };
        assert!(step_errors(&wizard).is_empty());
        wizard.step = Step::Threshold;
        assert!(!step_errors(&wizard).is_empty(), "no threshold yet");
        wizard.threshold = 2;
        assert!(step_errors(&wizard).is_empty());
    }

    #[test]
    fn a_member_of_every_scheme_the_chain_has_is_a_member() {
        use chainv4::crypto::keypair::PrivateKey;

        let keys: Vec<String> = [PrivateKey::Ed25519([1; 32]), PrivateKey::EdBlake([2; 32]), PrivateKey::Secp256k1([3; 32]), PrivateKey::Bitcoin([4; 32])].into_iter().map(|private| Keypair::from_private(private).public().to_string()).collect();
        let rows: Vec<Row> = keys.iter().map(|key| Row { key: key.clone(), ..Row::default() }).collect();
        let wizard = Wizard { rows: rows.clone(), threshold: 3, ..Wizard::default() };
        let made = policy(&wizard).expect("one of each scheme");
        let schemes: Vec<KeyType> = made.members.iter().map(|member| member.key.key_type()).collect();

        assert_eq!(schemes, [KeyType::Ed25519, KeyType::EdBlake, KeyType::Secp256k1, KeyType::Bitcoin]);

        // An Ethereum or Bitcoin account's address is told apart from its key, and so is a key off
        // its curve.
        let address = Keypair::from_private(PrivateKey::Secp256k1([3; 32])).public().address().to_string();
        let errors = member_errors(&[Row { key: address, ..Row::default() }]);
        assert!(errors[&(Some(0), "key")].starts_with("That is an address, not a key"), "{errors:?}");

        let off_curve = format!("secp256k1:0x05{}", "11".repeat(32));
        assert!(member_errors(&[Row { key: off_curve, ..Row::default() }])[&(Some(0), "key")].contains("Not a key on its curve"));
    }

    #[test]
    fn it_reaches_evm_chains_alone_bitcoin_alone_or_both_asking_only_what_each_needs() {
        let evm = |byte: u8| format!("0x{}", format!("{byte:02x}").repeat(20));
        let mut rows = rows(&[1, 2]);

        // Bitcoin alone asks each member's account key, and no EVM address.
        let errors = reach_errors(&rows, Reach::Bitcoin, false);
        assert!(errors.contains_key(&(Some(0), "xpub")) && !errors.contains_key(&(Some(0), "evm")), "{errors:?}");

        rows[0].xpub = xpub(false);
        rows[1].xpub = other_xpub(false);
        assert!(reach_errors(&rows, Reach::Bitcoin, false).is_empty());
        assert!(reach_errors(&rows, Reach::Both, false).contains_key(&(Some(0), "evm")), "both asks the EVM address too");

        // A Bitcoin address where its account key goes is told apart.
        let mut addressed = rows.clone();
        addressed[1].xpub = "bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq".to_string();
        assert!(reach_errors(&addressed, Reach::Bitcoin, false)[&(Some(1), "xpub")].starts_with("That is a Bitcoin address"));

        let wizard = Wizard { rows: rows.clone(), threshold: 2, reach: Reach::Bitcoin, testnet: false, ..Wizard::default() };
        let made = policy(&wizard).expect("on Bitcoin alone");
        assert!(made.members.iter().all(|member| member.bitcoin.is_some() && member.evm.is_none()));

        rows[0].evm = evm(0x11);
        rows[1].evm = evm(0x22);
        let both = policy(&Wizard { rows: rows.clone(), threshold: 2, reach: Reach::Both, ..Wizard::default() }).unwrap();
        let evm_only = policy(&Wizard { rows, threshold: 2, reach: Reach::Evm, ..Wizard::default() }).unwrap();
        assert!(both.members.iter().all(|member| member.bitcoin.is_some() && member.evm.is_some()));
        assert!(evm_only.members.iter().all(|member| member.bitcoin.is_none() && member.evm.is_some()));

        // Changing one on Bitcoin alone starts there.
        let listed = serde_json::json!({ "threshold": 1, "members": [{ "key": key(1), "bitcoin": xpub(false) }] });
        assert_eq!(Wizard::changing("multisig:0x03", &listed).reach, Reach::Bitcoin);
    }

    #[test]
    fn every_step_and_every_reach_fits_phones_and_a_desktop_and_fills_in_this_wallets_own() {
        use crate::views::drawn;

        for size in drawn::SCREENS {
            for reach in [Reach::Here, Reach::Evm, Reach::Bitcoin, Reach::Both] {
                for step in [Step::Members, Step::Reach, Step::Threshold, Step::Review] {
                    let mut book = drawn::two_phrases();
                    let mut holders = crate::keys::open(&book).unwrap();
                    let own = holders[1].publics[2].to_string();
                    let mut wizard = Wizard { step, reach, rows: vec![Row { key: own, ..Row::default() }, Row { key: key(9), ..Row::default() }], threshold: 2, ..Wizard::default() };
                    // Tall enough that nothing is left undrawn for being off the screen.
                    let (width, _, shown) = drawn::page_shown(&mut book, &mut holders, (size.0, 20_000.0), |ui, ctx| {
                        show(ui, ctx, &mut wizard, None);
                    });
                    let texts: Vec<&str> = shown.iter().map(|(text, _)| text.as_str()).collect();

                    assert!(width <= size.0 + 1.0, "{reach:?} {step:?} on {size:?}: {width}");

                    if step == Step::Reach && reach != Reach::Here {
                        assert!(texts.iter().any(|text| text.starts_with("Fill in from")), "{reach:?}: {texts:?}");
                    }

                    if step == Step::Reach && reach.bitcoin() {
                        assert!(texts.iter().any(|text| text.contains("is not a Bitcoin address")), "{texts:?}");
                    }
                }
            }
        }
    }
}
