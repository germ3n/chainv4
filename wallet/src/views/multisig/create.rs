//! Making a multisig, or changing the members of one, as plain steps: who the members are, whether
//! it reaches other chains, which is optional, how many must agree, and a review. Only a key on this
//! chain is asked of a member: a Bitcoin account key and an EVM address are asked only of a multisig
//! that is to hold coins there too.

use {
    super::{Ctx, follow, member_line},
    crate::format,
    crate::theme,
    chainv4::crypto::bitcoin::multisig::parse_xpub,
    chainv4::crypto::bitcoin::tx::Network,
    chainv4::crypto::keypair::{KeyType, PublicKey},
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
    Both,
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

/// What the first step asks: one key at least, 20 at most, each one a key, each once.
pub fn member_errors(rows: &[Row]) -> Errors {
    let mut errors = Errors::new();
    let mut seen = Vec::new();

    for (index, row) in rows.iter().enumerate() {
        match row.key.trim().parse::<PublicKey>() {
            _ if row.key.trim().is_empty() => {
                errors.insert((Some(index), "key"), "Required: this member's key on this chain.".to_string());
            }
            Err(_) => {
                errors.insert((Some(index), "key"), "Not a key. It looks like ed25519:0x and 64 hex digits, or a member's whole line.".to_string());
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

        if reach != Reach::Both {
            continue;
        }

        let xpub = row.xpub.trim();

        match parse_xpub(xpub) {
            _ if xpub.is_empty() => {
                errors.insert((Some(index), "xpub"), "Required once it reaches Bitcoin, of every member.".to_string());
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
        let evm = match wizard.reach {
            Reach::Here => None,
            Reach::Evm | Reach::Both => Some(from_hex(row.evm.trim()).and_then(|bytes| bytes.try_into().ok()).ok_or("Not an EVM address.")?),
        };
        let bitcoin = match wizard.reach {
            Reach::Both => Some(parse_xpub(row.xpub.trim()).map_err(|why| why.to_string())?.0),
            _ => None,
        };

        Ok(MultisigMember { key, bitcoin, evm })
    };
    let members = wizard.rows.iter().map(member).collect::<Result<Vec<_>, _>>()?;
    let policy = MultisigPolicy { members, threshold: wizard.threshold, testnet: wizard.reach == Reach::Both && wizard.testnet };

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
        let reach = match (rows.iter().any(|row| !row.xpub.is_empty()), rows.iter().any(|row| !row.evm.is_empty())) {
            (true, _) => Reach::Both,
            (false, true) => Reach::Evm,
            _ => Reach::Here,
        };

        Wizard { rows, reach, testnet: value["testnet"].as_bool().unwrap_or(false), threshold: value["threshold"].as_u64().unwrap_or(1) as u32, changing: Some(account.to_string()), ..Wizard::default() }
    }

    /// Whether a change of members changes anything.
    fn same_as(&self, value: &Value) -> bool {
        let listed: Vec<Row> = value["members"].as_array().into_iter().flatten().map(Row::from_listed).collect();
        let trimmed = |row: &Row| Row { key: row.key.trim().to_string(), evm: if self.reach == Reach::Here { String::new() } else { row.evm.trim().to_string() }, xpub: if self.reach == Reach::Both { row.xpub.trim().to_string() } else { String::new() } };

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
    let mine = account_keys(ctx);

    hint(ui, &palette, "Each member is one key on this chain: all a member needs. Ask each for theirs: for most accounts it is the ed25519:0x… address on their Receive page, and an offline signer shows it under Pair.");
    ui.add_space(4.0);

    let mut gone = None;

    for (index, row) in wizard.rows.iter_mut().enumerate() {
        ui.push_id(index, |ui| {
            let is_mine = mine.iter().any(|key| *key == row.key.trim());
            let label = format!("Member {}{} · required", index + 1, if is_mine { ", this account" } else { "" });
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

            if !row.key.is_empty() {
                bad(ui, &palette, errors.get(&(Some(index), "key")));
            }
        });
    }

    if let Some(index) = gone {
        wizard.rows.remove(index);
    }

    ui.horizontal_wrapped(|ui| {
        if !mine.iter().any(|key| wizard.rows.iter().any(|row| row.key.trim() == key.as_str()))
            && theme::primary(ui, &palette, "Add this account", true).clicked()
            && let Some(line) = ctx.holder().and_then(|holder| member_line(holder, crate::keys::seed(ctx.book).as_ref(), wizard.testnet))
        {
            wizard.rows.push(Row::from_line(&line));
        }

        if theme::quiet(ui, &palette, "Add a member").clicked() {
            wizard.rows.push(Row::default());
        }
    });

    // None yet is what Next waits for, not a mistake.
    if !wizard.rows.is_empty() {
        bad(ui, &palette, errors.get(&(None, "members")));
    }
}

/// The account's keys on this chain, as members name them.
fn account_keys(ctx: &Ctx) -> Vec<String> {
    let Some(holder) = ctx.holder() else {
        return Vec::new();
    };

    holder.addresses().iter().filter_map(|address| holder.public(address)).map(|key| key.to_string()).collect()
}

/// The second step: whether it reaches other chains, and if so each member's address there.
fn reach(ui: &mut Ui, ctx: &mut Ctx, wizard: &mut Wizard, errors: &Errors) {
    let palette = *ctx.palette;

    hint(ui, &palette, "Most multisigs need nothing here. A Bitcoin address is not needed to make one.");
    ui.add_space(2.0);
    theme::segmented_rows(ui, &palette, &mut wizard.reach, &[(Reach::Here, "Only this chain"), (Reach::Evm, "Also EVM chains"), (Reach::Both, "Also Bitcoin and EVM")]);
    ui.add_space(2.0);

    match wizard.reach {
        Reach::Here => hint(ui, &palette, "It holds coins and tokens here, and sends anything an account sends. Its members can add other chains later, by changing its members."),
        Reach::Evm => hint(ui, &palette, "Fill these in to hold coins on Ethereum-like chains too: the same members control one contract, at one address on every such chain. Every member gives one, or none does."),
        Reach::Both => hint(ui, &palette, "Fill these in to hold bitcoin too, in a multisig other Bitcoin wallets can co-sign, as well as on EVM chains. Every member gives both, or none does."),
    }

    if wizard.reach == Reach::Both {
        let was = wizard.testnet;
        ui.add_space(2.0);
        theme::segmented(ui, &palette, &mut wizard.testnet, &[(false, "Bitcoin"), (true, "Bitcoin's test networks")]);

        // This account's own key follows the network; others' are their own to give.
        if was != wizard.testnet
            && let Some(line) = ctx.holder().and_then(|holder| member_line(holder, crate::keys::seed(ctx.book).as_ref(), wizard.testnet))
        {
            let mine = Row::from_line(&line);

            for row in wizard.rows.iter_mut().filter(|row| row.key.trim() == mine.key) {
                row.xpub = mine.xpub.clone();
            }
        }
    }

    if wizard.reach == Reach::Here {
        return;
    }

    let mine = account_keys(ctx);

    for (index, row) in wizard.rows.iter_mut().enumerate() {
        ui.add_space(6.0);
        ui.push_id(index, |ui| {
            let name = match mine.iter().any(|key| *key == row.key.trim()) {
                true => format!("Member {}, this account", index + 1),
                false => format!("Member {} · {}", index + 1, format::short(row.key.trim())),
            };
            ui.label(theme::strong(name, 13.0).color(palette.soft));
            ui.label(RichText::new("EVM address · required for this choice").small().color(palette.soft));
            ui.add(egui::TextEdit::singleline(&mut row.evm).hint_text("0x…").font(egui::TextStyle::Monospace).desired_width(f32::INFINITY));
            bad(ui, &palette, errors.get(&(Some(index), "evm")));

            if wizard.reach == Reach::Both {
                ui.label(RichText::new("Bitcoin account key · required for this choice").small().color(palette.soft));
                hint(ui, &palette, "Not a Bitcoin address: the member's multisig account key, with its origin. Bitcoin addresses are made from all the members' keys together.");
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
    let policy = policy(wizard);

    super::super::row(ui, &palette, "Who must agree", |ui| {
        ui.label(RichText::new(format!("{} members", of(wizard.threshold, wizard.rows.len()))).color(palette.ink));
    });

    for (index, row) in wizard.rows.iter().enumerate() {
        let you = if mine.iter().any(|key| *key == row.key.trim()) { " · this account" } else { "" };
        ui.add(egui::Label::new(RichText::new(format!("{}. {}{you}", index + 1, format::short(row.key.trim()))).monospace().color(palette.soft)).wrap());
    }

    super::super::row(ui, &palette, "Where", |ui| {
        ui.label(match (wizard.reach, wizard.testnet) {
            (Reach::Here, _) => "This chain only",
            (Reach::Evm, _) => "This chain and EVM chains",
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
}
