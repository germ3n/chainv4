//! The propose form: a field for each setting of a kind of change, as schema.rs lists them, what
//! stands beside each, what is wrong with it under it, and what changed. What is typed reads into
//! the change's JSON: only the fields that differ from what stands are given, laid over it with
//! params::json::over as the CLI's vote-change does, and the whole goes through change_from_json,
//! so the form checks what the node checks. What only a node can reach of its checks, such as
//! whether a consensus fits the finality, is asked of the settings the node lists.

use {
    super::schema::{Cell, Column, Control, Line, Spec, defaults, schema, variants},
    super::{bytes, millis},
    crate::format,
    crate::theme,
    chainv4::crypto::keypair::PublicKey,
    chainv4::governance::Change,
    chainv4::params::fees::TX_KINDS,
    chainv4::params::json::{change_from_json, over},
    chainv4::registry::collections::CollectionFunctions,
    chainv4::registry::signers::{Signer, SignerSet},
    chainv4::structs::address::Address,
    chainv4::structs::hex::from_hex,
    chainv4::structs::version::Version,
    egui::{RichText, Ui},
    serde_json::{Map, Value, json},
    std::collections::{BTreeMap, HashSet},
};

/// Where an error goes that belongs to no one field: the foot of the form.
pub const GENERAL: &str = "*";

/// The widest a field grows, on a wide screen: a number needs no more.
const FIELD_MOST: f32 = 520.0;

/// What the form takes from the chain and the wallet, beside what stands.
#[derive(Clone, Copy, Debug)]
pub struct Env<'a> {
    pub decimals: u8,
    pub symbol: &'a str,
    pub epoch: u64,
    /// The light client's tip, which an upgrade must start ahead of.
    pub tip: Option<u64>,
    /// The finality decided last and the consensus in force, by kind.
    pub finality: Option<&'a str>,
    pub consensus: Option<&'a str>,
    /// Whether a switch of finality's kind is on its way, which holds back all that touches it.
    pub switching: bool,
}

/// What is typed in the form, by each field's path.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Draft {
    texts: BTreeMap<String, String>,
    switches: BTreeMap<String, bool>,
    lists: BTreeMap<String, Vec<String>>,
    tables: BTreeMap<String, Vec<Vec<String>>>,
}

/// One setting that moves: what it is called, what it says now and what it would say.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diff {
    pub label: String,
    pub old: String,
    pub new: String,
}

/// What the form reads as it stands.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Built {
    /// What is wrong, by the field's path, a list's line's as path#line and a table's cell's as
    /// path#row.column; GENERAL for what belongs to no field.
    pub errors: BTreeMap<String, String>,
    /// The fields that differ from what stands, as they are laid over it.
    pub given: Value,
    /// The change, once all of it reads and nothing is wrong.
    pub change: Option<Change>,
    /// What it moves, field by field.
    pub changes: Vec<Diff>,
}

impl Draft {
    /// The form filled with what stands, and defaults for what does not.
    pub fn of(kind: &str, standing: Option<&Value>, env: &Env) -> Draft {
        let mut draft = Draft::default();
        let start = defaults(kind);

        for spec in schema(kind) {
            draft.fill(&spec, at(&start, spec.path).unwrap_or(&Value::Null), env);

            if let Some(value) = standing.and_then(|standing| at(standing, spec.path)) {
                draft.fill(&spec, value, env);
            }
        }

        // A switch goes to the other kind, no sooner than the chain allows.
        if kind == "switch_finality" {
            let other = match draft.says("finality.type") {
                "authority" => "ffg",
                _ => "authority",
            };
            draft.texts.insert("finality.type".to_string(), other.to_string());
            draft.texts.insert("epoch".to_string(), earliest_switch(standing, env).to_string());
        }

        draft
    }

    /// Takes a field's value as its JSON says it.
    fn fill(&mut self, spec: &Spec, value: &Value, env: &Env) {
        let text = |value: &Value| match value {
            Value::Null => String::new(),
            Value::String(text) => text.clone(),
            other => other.to_string(),
        };
        let path = spec.path.to_string();

        match spec.control {
            Control::Amount => {
                self.texts.insert(path, value.as_str().map_or_else(String::new, |raw| plain_amount(raw, env.decimals)));
            }
            Control::Bytes => {
                self.texts.insert(path, value.as_u64().map_or_else(String::new, bytes));
            }
            Control::MaybeWhole { .. } | Control::MaybeBytes { .. } => {
                self.switches.insert(path.clone(), !value.is_null());

                if !value.is_null() {
                    let shown = match spec.control {
                        Control::MaybeBytes { .. } => value.as_u64().map_or_else(String::new, bytes),
                        _ => text(value),
                    };
                    self.texts.insert(path, shown);
                }
            }
            Control::Switch { .. } => {
                self.switches.insert(path, !value.is_null());
            }
            Control::Flag { .. } => {
                self.switches.insert(path, value.as_bool() == Some(true));
            }
            Control::MaybeChoice { .. } => {
                self.texts.insert(path, value.as_str().unwrap_or_default().to_string());
            }
            Control::List(line) => {
                let lines = value.as_array().into_iter().flatten().map(|item| match line {
                    Line::Price => item.as_str().map_or_else(String::new, |raw| plain_amount(raw, env.decimals)),
                    _ => text(item),
                });
                self.lists.insert(path, lines.collect());
            }
            Control::Table(columns) => {
                let rows = value.as_array().into_iter().flatten().map(|row| {
                    columns
                        .iter()
                        .map(|column| match column.cell {
                            Cell::Amount => row[column.name].as_str().map_or_else(String::new, |raw| plain_amount(raw, env.decimals)),
                            _ => text(&row[column.name]),
                        })
                        .collect()
                });
                self.tables.insert(path, rows.collect());
            }
            _ => {
                self.texts.insert(path, text(value));
            }
        }
    }

    /// What a choice or a switch says, for what shows by it.
    fn says(&self, path: &str) -> &str {
        match self.switches.get(path) {
            Some(true) => "on",
            Some(false) => "off",
            None => self.texts.get(path).map_or("", String::as_str),
        }
    }

    fn shows(&self, spec: &Spec) -> bool {
        spec.when.iter().all(|(path, value)| self.says(path) == *value)
    }

    fn text(&self, path: &str) -> &str {
        self.texts.get(path).map_or("", String::as_str)
    }

    /// Whether a field holds what stands, by what it reads as.
    fn unchanged(&self, spec: &Spec, standing: Option<&Value>, env: &Env) -> bool {
        let mut errors = BTreeMap::new();

        match read(spec, self, env, &mut errors) {
            Some(value) => errors.is_empty() && same(standing.and_then(|standing| at(standing, spec.path)), &value),
            None => matches!(spec.control, Control::Switch { .. }) && standing.and_then(|standing| at(standing, spec.path)).is_some_and(|value| !value.is_null()),
        }
    }
}

/// The earliest epoch a switch may name: activation_delay + 1 after the one it is approved in.
pub fn earliest_switch(standing: Option<&Value>, env: &Env) -> u64 {
    let delay = standing.and_then(|standing| standing["finality"]["activation_delay"].as_u64()).unwrap_or(1);

    env.epoch + delay + 1
}

/// The amount in the coin as a field holds it, with no commas to edit around.
fn plain_amount(raw: &str, decimals: u8) -> String {
    format::units(raw, decimals).replace(',', "")
}

/// What a kind's JSON has at a path, its keys joined by dots; the value itself for no path.
pub fn at<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    match path {
        "" => Some(value),
        path => path.split('.').try_fold(value, |value, key| value.get(key)),
    }
}

/// Puts a value at a path, making the objects on the way.
fn put(root: &mut Value, path: &str, value: Value) {
    if path.is_empty() {
        *root = value;
        return;
    }

    let mut place = root;

    for key in path.split('.') {
        if !place.is_object() {
            *place = Value::Object(Map::new());
        }

        place = place.as_object_mut().expect("just made one").entry(key).or_insert(Value::Null);
    }

    *place = value;
}

/// Takes what is at a path away.
fn take(root: &mut Value, path: &str) {
    if path.is_empty() {
        *root = Value::Object(Map::new());
        return;
    }

    let (parent, key) = path.rsplit_once('.').unwrap_or(("", path));

    if let Some(parent) = at_mut(root, parent).and_then(Value::as_object_mut) {
        parent.remove(key);
    }
}

fn at_mut<'a>(value: &'a mut Value, path: &str) -> Option<&'a mut Value> {
    match path {
        "" => Some(value),
        path => path.split('.').try_fold(value, |value, key| value.get_mut(key)),
    }
}

/// Whether what stands and what is typed are the same: nothing, null and an empty list all say none,
/// as genesis files leave what is unset out.
fn same(old: Option<&Value>, new: &Value) -> bool {
    let none = |value: &Value| value.is_null() || value.as_array().is_some_and(Vec::is_empty);

    match old {
        None => none(new),
        Some(old) => old == new || (none(old) && none(new)),
    }
}

/// A whole number as typed: commas and underscores go.
fn whole(text: &str) -> Option<u64> {
    text.trim().replace([',', '_'], "").parse().ok()
}

/// A number of bytes as typed: bytes, or so many KiB, MiB or GiB.
fn typed_bytes(text: &str) -> Option<u64> {
    let text = text.trim().to_lowercase().replace([',', '_'], "");
    let (number, unit) = text.split_at(text.find(|c: char| !c.is_ascii_digit()).unwrap_or(text.len()));
    let scale: u64 = match unit.trim() {
        "" | "b" | "bytes" => 1,
        "kib" | "kb" => 1 << 10,
        "mib" | "mb" => 1 << 20,
        "gib" | "gb" => 1 << 30,
        _ => return None,
    };

    number.parse::<u64>().ok()?.checked_mul(scale)
}

/// Whether a number is within a field's bounds, or why not, in its own terms.
fn bounded(spec: &Spec, number: u64) -> Result<(), String> {
    let said = |number: u64| match spec.control {
        Control::Bytes | Control::MaybeBytes { .. } => bytes(number),
        _ => format::thousands(&number.to_string()),
    };

    if let Some(least) = spec.least.filter(|least| number < *least) {
        return Err(format!("At least {}.", said(least)));
    }

    if let Some(most) = spec.most.filter(|most| number > *most) {
        return Err(format!("At most {}.", said(most)));
    }

    Ok(())
}

/// A contract's address as typed, or why not.
fn contract(text: &str) -> Result<String, String> {
    match text.parse::<Address>() {
        Ok(address @ Address::Contract(_)) => Ok(address.to_string()),
        _ => Err("A contract's address: contract:0x and 64 hex digits.".to_string()),
    }
}

/// An EVM address as typed, or why not.
fn evm(text: &str) -> Result<String, String> {
    match from_hex(text).and_then(|bytes| <[u8; 20]>::try_from(bytes).ok()) {
        Some(address) if address == [0; 20] => Err("Not the zero address.".to_string()),
        Some(_) => Ok(text.to_string()),
        None => Err("An EVM address: 0x and 40 hex digits.".to_string()),
    }
}

/// A key on this chain as typed, or why not.
fn key(text: &str) -> Result<String, String> {
    text.parse::<PublicKey>().map(|key| key.to_string()).map_err(|_| "A key, such as ed25519:0x and 64 hex digits.".to_string())
}

/// An amount in the coin as typed, as its smallest unit, or why not.
fn amount(text: &str, env: &Env) -> Result<String, String> {
    format::parse_units(text, env.decimals).ok_or_else(|| format!("An amount in {}, such as 1.5, with at most {} decimals.", env.symbol, env.decimals))
}

/// A line of a list as JSON, or why not.
fn line_value(line: Line, text: &str, env: &Env) -> Result<Value, String> {
    match line {
        Line::Bootnode if !text.starts_with('/') || text.len() > 256 || text.chars().any(char::is_whitespace) => Err("A libp2p address, such as /ip4/203.0.113.7/udp/4001/quic-v1.".to_string()),
        Line::TxKind if !TX_KINDS.contains(&text) => Err(format!("Not a kind of transaction. One of {}.", TX_KINDS.join(", "))),
        Line::Compressed if !from_hex(text).is_some_and(|bytes| bytes.len() == 33) => Err("A compressed key: 0x and 66 hex digits.".to_string()),
        Line::Price => amount(text, env).map(Value::String),
        Line::Account => text.parse::<Address>().map(|address| Value::String(address.to_string())).map_err(|_| "An address, such as ed25519:0x and 64 hex digits.".to_string()),
        _ => Ok(Value::String(text.to_string())),
    }
}

/// A cell of a table as JSON, or why not.
fn cell_value(column: &Column, text: &str, env: &Env) -> Result<Value, String> {
    match column.cell {
        Cell::Whole => match whole(text) {
            Some(0) | None => Err("A whole number above 0.".to_string()),
            Some(number) => Ok(json!(number)),
        },
        Cell::Key => key(text).map(Value::String),
        Cell::Account => text.parse::<Address>().map(|address| Value::String(address.to_string())).map_err(|_| "An address, such as ed25519:0x and 64 hex digits.".to_string()),
        Cell::Evm => evm(text).map(Value::String),
        Cell::Amount => amount(text, env).map(Value::String),
    }
}

/// A field's JSON as typed: None for a switch that is on, whose fields say what it holds, and for
/// what does not read, which says why in errors.
fn read(spec: &Spec, draft: &Draft, env: &Env, errors: &mut BTreeMap<String, String>) -> Option<Value> {
    let text = draft.text(spec.path).trim();
    let on = draft.switches.get(spec.path).copied().unwrap_or(false);
    let mut failed = |why: String| {
        errors.insert(spec.path.to_string(), why);

        None
    };

    match spec.control {
        Control::Text => Some(json!(text)),
        Control::Version => match text.parse::<Version>() {
            Ok(_) => Some(json!(text)),
            Err(_) => failed("A version, such as 0.2.0.".to_string()),
        },
        Control::Whole | Control::Permille | Control::Millis | Control::Small => match whole(text) {
            None => failed("A whole number.".to_string()),
            Some(number) if spec.control == Control::Small && u32::try_from(number).is_err() => failed(format!("At most {}.", format::thousands(&u32::MAX.to_string()))),
            Some(number) => match bounded(spec, number) {
                Ok(()) => Some(json!(number)),
                Err(why) => failed(why),
            },
        },
        Control::Digits => match text.replace([',', '_'], "").parse::<u128>() {
            Ok(number) => Some(json!(number.to_string())),
            Err(_) => failed("A whole number.".to_string()),
        },
        Control::Amount => match amount(text, env) {
            Ok(raw) => Some(json!(raw)),
            Err(why) => failed(why),
        },
        Control::Bytes => match typed_bytes(text) {
            None => failed("A number of bytes, or so many KiB or MiB.".to_string()),
            Some(number) => match bounded(spec, number) {
                Ok(()) => Some(json!(number)),
                Err(why) => failed(why),
            },
        },
        Control::Key => match key(text) {
            Ok(key) => Some(json!(key)),
            Err(why) => failed(why),
        },
        Control::Contract => match contract(text) {
            Ok(address) => Some(json!(address)),
            Err(why) => failed(why),
        },
        Control::MaybeContract => match text {
            "" => Some(Value::Null),
            text => match contract(text) {
                Ok(address) => Some(json!(address)),
                Err(why) => failed(format!("{why} Empty for none.")),
            },
        },
        Control::MaybeEvm => match text {
            "" => Some(Value::Null),
            text => match evm(text) {
                Ok(address) => Some(json!(address)),
                Err(why) => failed(format!("{why} Empty for none.")),
            },
        },
        Control::Choice(options) => match options.iter().any(|(value, _)| *value == text) {
            true => Some(json!(text)),
            false => failed("Pick one.".to_string()),
        },
        Control::MaybeChoice { options, .. } => match text {
            "" => Some(Value::Null),
            text if options.iter().any(|(value, _)| *value == text) => Some(json!(text)),
            _ => failed("Pick one.".to_string()),
        },
        Control::MaybeWhole { .. } if !on => Some(Value::Null),
        Control::MaybeWhole { .. } => match whole(text) {
            None => failed("A whole number.".to_string()),
            Some(number) => match bounded(spec, number) {
                Ok(()) => Some(json!(number)),
                Err(why) => failed(why),
            },
        },
        Control::MaybeBytes { .. } if !on => Some(Value::Null),
        Control::MaybeBytes { .. } => match typed_bytes(text) {
            None => failed("A number of bytes, or so many KiB or MiB.".to_string()),
            Some(number) => match bounded(spec, number) {
                Ok(()) => Some(json!(number)),
                Err(why) => failed(why),
            },
        },
        Control::Switch { .. } => match on {
            true => None,
            false => Some(Value::Null),
        },
        Control::Flag { .. } => Some(json!(on)),
        Control::List(line) => {
            let mut seen = HashSet::new();
            let mut values = Vec::new();

            // A line left empty is no line.
            for (index, typed) in draft.lists.get(spec.path).into_iter().flatten().enumerate().map(|(index, typed)| (index, typed.trim())).filter(|(_, typed)| !typed.is_empty()) {
                match line_value(line, typed, env) {
                    Ok(_) if line != Line::Price && !seen.insert(typed) => {
                        errors.insert(line_key(spec.path, index), "Named twice.".to_string());
                    }
                    Ok(value) => values.push(value),
                    Err(why) => {
                        errors.insert(line_key(spec.path, index), why);
                    }
                }
            }

            Some(Value::Array(values))
        }
        Control::Table(columns) => {
            let mut values = Vec::new();

            for (row, cells) in draft.tables.get(spec.path).into_iter().flatten().enumerate().filter(|(_, cells)| cells.iter().any(|cell| !cell.trim().is_empty())) {
                let mut object = Map::new();

                for (column, typed) in columns.iter().zip(cells) {
                    match cell_value(column, typed.trim(), env) {
                        Ok(value) => {
                            object.insert(column.name.to_string(), value);
                        }
                        Err(why) => {
                            errors.insert(cell_key(spec.path, row, column.name), why);
                        }
                    }
                }

                values.push(Value::Object(object));
            }

            Some(Value::Array(values))
        }
    }
}

fn line_key(path: &str, line: usize) -> String {
    format!("{path}#{line}")
}

fn cell_key(path: &str, row: usize, column: &str) -> String {
    format!("{path}#{row}.{column}")
}

/// The fields a kind shows, as what is typed picks among them.
pub fn shown(kind: &str, draft: &Draft) -> Vec<Spec> {
    schema(kind).into_iter().filter(|spec| draft.shows(spec)).collect()
}

/// What the form says as it stands: what is wrong, or the change, and what it moves.
pub fn build(kind: &str, draft: &Draft, standing: Option<&Value>, env: &Env) -> Built {
    let fields = shown(kind, draft);
    let mut built = Built::default();
    let mut values: Vec<(&str, Value)> = Vec::new();

    for spec in &fields {
        if let Some(value) = read(spec, draft, env, &mut built.errors) {
            values.push((spec.path, value));
        }
    }

    if !built.errors.is_empty() {
        return built;
    }

    let (whole, given) = laid(kind, &values, draft, standing);
    built.given = given;
    let whole = finished(kind, whole);

    judge(kind, &whole, env, &mut built);

    if built.change.is_some() {
        built.changes = diff(kind, standing, &whole, env);
    }

    built
}

/// The whole value, and the fields that differ from what stands: what is given, over what stands. A
/// variant of another type replaces its object whole, as over only adds to one.
fn laid(kind: &str, values: &[(&str, Value)], draft: &Draft, standing: Option<&Value>) -> (Value, Value) {
    if let Some((_, value)) = values.iter().find(|(path, _)| path.is_empty()) {
        let given = match same(standing, value) {
            true => Value::Null,
            false => value.clone(),
        };

        return (value.clone(), given);
    }

    let mut base = standing.filter(|standing| standing.is_object()).cloned().unwrap_or_else(|| Value::Object(Map::new()));
    let replaced: Vec<&str> = variants(kind)
        .iter()
        .copied()
        .filter(|variant| at(&base, &type_of(variant)).and_then(Value::as_str) != Some(draft.text(&type_of(variant))))
        .collect();

    for variant in &replaced {
        take(&mut base, variant);
    }

    let whole_new = !standing.is_some_and(Value::is_object);
    let mut given = Value::Object(Map::new());
    let under = |path: &str, variant: &str| variant.is_empty() || path == variant || path.strip_prefix(variant).is_some_and(|rest| rest.starts_with('.'));

    for (path, value) in values {
        if whole_new || replaced.iter().any(|variant| under(path, variant)) || !same(at(&base, path), value) {
            put(&mut given, path, value.clone());
        }
    }

    (over(&base, &given), given)
}

/// Where a variant's type sits.
fn type_of(variant: &str) -> String {
    match variant {
        "" => "type".to_string(),
        variant => format!("{variant}.type"),
    }
}

/// What the chain asks of a whole value that no field holds: a directory's chains in order, and no
/// validators named by a switch into FFG.
pub fn finished(kind: &str, mut whole: Value) -> Value {
    match kind {
        "directory" => {
            if let Some(gateways) = whole["evm_gateways"].as_array_mut() {
                gateways.sort_by_key(|gateway| gateway["chain_id"].as_u64());
            }
        }
        "switch_finality" if whole["finality"]["type"] == "ffg" => whole["finality"]["validators"] = json!([]),
        _ => {}
    }

    whole
}

/// Checks a whole value: what only the chain's settings can say, then what change_from_json says, as
/// the node does, each under the field it is about.
pub fn judge(kind: &str, whole: &Value, env: &Env, built: &mut Built) {
    for (path, why) in checks(kind, whole, env) {
        built.errors.entry(path).or_insert(why);
    }

    match change_from_json(&json!({ kind: whole })) {
        Ok(change) => {
            if let Change::Upgrade(upgrade) = &change {
                let tip = env.tip.unwrap_or_default();

                if upgrade.problem(tip).is_some() {
                    match upgrade.least == Version::default() {
                        true => built.errors.insert("least".to_string(), "Above 0.0.0, which stops nothing.".to_string()),
                        false => built.errors.insert("from".to_string(), format!("It must start ahead of the tip, block {}.", format::thousands(&tip.to_string()))),
                    };
                }
            }

            if built.errors.is_empty() {
                built.change = Some(change);
            }
        }
        Err(why) => {
            built.errors.entry(blame(kind, &why)).or_insert(why);
        }
    }
}

/// What the chain checks of a change against what it runs, which a node alone reads there; asked
/// here of the settings it lists, each in the node's words. A node still has the last word.
fn checks(kind: &str, whole: &Value, env: &Env) -> Vec<(String, String)> {
    let mut found = Vec::new();
    let mut add = |path: &str, why: &str| found.push((path.to_string(), why.to_string()));

    if env.switching && matches!(kind, "consensus" | "finality" | "switch_finality") {
        add(GENERAL, "A switch of finality's kind is on its way: nothing that touches the consensus or finality is approved until it has run.");
    }

    let fits = |consensus: Option<&str>, finality: Option<&str>| match consensus {
        Some("rotating") if finality != Some("authority") => Some("Signers in turn need finality of the authority kind, whose signers make the blocks."),
        Some("staked") if finality != Some("ffg") => Some("Validators by stake need FFG finality, whose validators make the blocks."),
        _ => None,
    };

    match kind {
        "consensus" => {
            if let Some(why) = fits(whole["type"].as_str(), env.finality) {
                add("type", why);
            }
        }
        "finality" => {
            let listed = |name: &str| whole[name].as_array().map_or(0, Vec::len) as u64;

            if whole["type"] == "ffg" && whole["max_validators"].as_u64().is_some_and(|most| listed("validators") > most) {
                add("max_validators", "Fewer than the validators from genesis.");
            }

            if whole["type"] == "authority" && whole["max_signers"].as_u64().is_some_and(|most| listed("signers") > most) {
                add("max_signers", "Fewer than the signers from genesis.");
            }
        }
        "switch_finality" => {
            let delay = whole["finality"]["activation_delay"].as_u64().unwrap_or(1);

            if whole["epoch"].as_u64().is_some_and(|epoch| epoch < env.epoch + delay + 1) {
                add("epoch", &format!("At least epoch {}: the epoch now, {}, and {} more.", env.epoch + delay + 1, env.epoch, delay + 1));
            }

            if env.finality == whole["finality"]["type"].as_str() || env.finality == Some("depth") {
                add(GENERAL, "A switch goes between FFG and authority finality, to the kind the chain does not run.");
            }

            if let Some(why) = fits(env.consensus, whole["finality"]["type"].as_str()) {
                add(GENERAL, why);
            }

            if whole["finality"]["type"] == "authority" {
                add_signers(whole, &mut add);
            }
        }
        _ => {}
    }

    found
}

/// What the node asks of the signers a switch names, as it asks it of any set.
fn add_signers(whole: &Value, add: &mut impl FnMut(&str, &str)) {
    let number = |name: &str| whole["finality"][name].as_u64().and_then(|number| u32::try_from(number).ok()).unwrap_or(0);
    let signers: Option<Vec<Signer>> = whole["finality"]["signers"]
        .as_array()
        .map(|signers| signers.iter().map(|signer| Some(Signer { key: signer["key"].as_str()?.parse().ok()?, account: signer["account"].as_str()?.parse().ok()? })).collect::<Option<Vec<_>>>())
        .unwrap_or_default();
    let Some(signers) = signers else {
        return;
    };

    if let Some(why) = SignerSet::new(signers, number("threshold"), number("change_threshold")).problem(number("max_signers")) {
        let path = match why {
            why if why.starts_with("change_threshold") => "finality.change_threshold",
            why if why.starts_with("threshold") => "finality.threshold",
            why if why.contains("max_signers") => "finality.max_signers",
            _ => "finality.signers",
        };
        add(path, why);
    }
}

/// Which field an error of the node's is about: by the words its checks use, or by the path it
/// starts with; GENERAL when it says neither.
pub fn blame(kind: &str, why: &str) -> String {
    const WORDS: [(&str, &str, &str); 27] = [
        ("name", "chain's name", ""),
        ("bootnodes", "bootnode", ""),
        ("names", "names.prices", "prices"),
        ("fees", "block_gas_limit", "block_gas_limit"),
        ("fees", "burn_permille", "burn_permille"),
        ("fees", "fuel_per_gas", "schedule.fuel_per_gas"),
        ("fees", "scorer must be a contract", "schedule.contract"),
        ("fees", "scorer's fuel", "schedule.fuel"),
        ("fees", "scorer's kinds", "schedule.kinds"),
        ("fees", "base fee's min", "pricing.min"),
        ("fees", "initial price", "pricing.initial"),
        ("fees", "elasticity", "pricing.elasticity"),
        ("reward", "halves every", "halving_interval"),
        ("limits", "needs at least 100,000 fuel", "tx_limits.fuel"),
        ("limits", "max_block_bytes", "max_block_bytes"),
        ("federation", "1 to 20 signers", "signers"),
        ("federation", "each signer once", "signers"),
        ("federation", "compressed secp256k1", "signers"),
        ("federation", "threshold", "threshold"),
        ("federation", "at least one block", "chain_depth"),
        ("directory", "contracts here", "gateway"),
        ("directory", "at most 32 EVM", "evm_gateways"),
        ("directory", "each EVM contract", "evm_gateways"),
        ("directory", "chain ids", "evm_gateways"),
        ("collections", "each function once", "nft"),
        ("upgrade", "upgrade.least", "least"),
        ("upgrade", "upgrade.from", "from"),
    ];

    if let Some((_, _, path)) = WORDS.iter().find(|(of, words, _)| *of == kind && why.contains(words)) {
        return path.to_string();
    }

    // A field the JSON's reading names: fees.schedule.contract: cannot read …
    let named = why.split([':', ' ']).next().unwrap_or_default();
    let path = named.strip_prefix(kind).and_then(|rest| rest.strip_prefix('.')).unwrap_or(named);

    match schema(kind).iter().any(|spec| spec.path == path && !path.is_empty()) {
        true => path.to_string(),
        false => GENERAL.to_string(),
    }
}

/// What a value of a field says, in words: an address whole.
pub fn words(spec: &Spec, value: Option<&Value>, env: &Env) -> String {
    let value = value.unwrap_or(&Value::Null);
    let thousands = |value: &Value| value.as_u64().map(|number| number.to_string()).or_else(|| value.as_str().map(str::to_string)).map_or_else(|| "none".to_string(), |digits| format::thousands(&digits));

    match spec.control {
        Control::Text | Control::Version | Control::Key | Control::Contract | Control::MaybeContract | Control::MaybeEvm => value.as_str().filter(|text| !text.is_empty()).map_or_else(|| "none".to_string(), str::to_string),
        Control::Whole | Control::Small | Control::Digits => thousands(value),
        Control::Amount => value.as_str().map_or_else(|| "none".to_string(), |raw| format::amount(raw, env.decimals, env.symbol)),
        Control::Permille => value.as_u64().map_or_else(|| "none".to_string(), |permille| format!("{}%", permille as f64 / 10.0)),
        Control::Millis => value.as_u64().map_or_else(|| "none".to_string(), millis),
        Control::Bytes => value.as_u64().map_or_else(|| "none".to_string(), bytes),
        Control::Choice(options) => value.as_str().map_or_else(|| "none".to_string(), |text| options.iter().find(|(known, _)| *known == text).map_or(text, |(_, label)| label).to_string()),
        Control::MaybeChoice { off, options } => value.as_str().map_or_else(|| off.to_string(), |text| options.iter().find(|(known, _)| *known == text).map_or(text, |(_, label)| label).to_string()),
        Control::MaybeWhole { off, .. } => value.as_u64().map_or_else(|| off.to_string(), |number| format::thousands(&number.to_string())),
        Control::MaybeBytes { off, .. } => value.as_u64().map_or_else(|| off.to_string(), bytes),
        Control::Switch { off, on } => if value.is_null() { off } else { on }.to_string(),
        Control::Flag { off, on } => if value.as_bool() == Some(true) { on } else { off }.to_string(),
        Control::List(_) | Control::Table(_) => match value.as_array().map_or(0, Vec::len) {
            0 => "none".to_string(),
            1 => "1 listed".to_string(),
            count => format!("{count} listed"),
        },
    }
}

/// Whether a field shows for a value, as Draft::shows does for what is typed.
fn shows_for(spec: &Spec, value: &Value) -> bool {
    spec.when.iter().all(|(path, wanted)| match at(value, path) {
        None | Some(Value::Null) => *wanted == "off",
        Some(Value::String(text)) => text == wanted,
        Some(_) => *wanted == "on",
    })
}

/// A table's row, in words.
fn row_words(columns: &[Column], row: &Value, env: &Env) -> String {
    columns
        .iter()
        .map(|column| match column.cell {
            Cell::Amount => row[column.name].as_str().map_or_else(String::new, |raw| format::amount(raw, env.decimals, env.symbol)),
            Cell::Whole => row[column.name].to_string(),
            _ => row[column.name].as_str().unwrap_or_default().to_string(),
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

/// What a price's place in the list is for.
fn price_label(index: usize, last: usize) -> String {
    match index == last {
        true => format!("{} or more characters", index + 3),
        false => format!("{} characters", index + 3),
    }
}

/// What a whole value moves against what stands, field by field in the form's order.
pub fn diff(kind: &str, standing: Option<&Value>, whole: &Value, env: &Env) -> Vec<Diff> {
    let mut changes = Vec::new();
    let empty = Vec::new();

    for spec in schema(kind).iter().filter(|spec| shows_for(spec, whole)) {
        let (old, new) = (standing.and_then(|standing| at(standing, spec.path)), at(whole, spec.path));
        let mut add = |label: String, old: String, new: String| changes.push(Diff { label, old, new });

        match spec.control {
            Control::List(Line::Price) => {
                let (before, after) = (old.and_then(Value::as_array).unwrap_or(&empty), new.and_then(Value::as_array).unwrap_or(&empty));
                let price = |value: Option<&Value>| value.and_then(Value::as_str).map_or_else(|| "none".to_string(), |raw| format::amount(raw, env.decimals, env.symbol));

                for index in 0..before.len().max(after.len()) {
                    if before.get(index) != after.get(index) {
                        add(price_label(index, after.len().max(1) - 1), price(before.get(index)), price(after.get(index)));
                    }
                }
            }
            Control::List(_) => {
                let (before, after) = (old.and_then(Value::as_array).unwrap_or(&empty), new.and_then(Value::as_array).unwrap_or(&empty));
                let line = |value: &Value| value.as_str().unwrap_or_default().to_string();

                for gone in before.iter().filter(|item| !after.contains(item)) {
                    add(spec.label.to_string(), line(gone), "taken away".to_string());
                }

                for came in after.iter().filter(|item| !before.contains(item)) {
                    add(spec.label.to_string(), "not listed".to_string(), line(came));
                }
            }
            Control::Table(columns) => {
                let (before, after) = (old.and_then(Value::as_array).unwrap_or(&empty), new.and_then(Value::as_array).unwrap_or(&empty));

                for gone in before.iter().filter(|item| !after.contains(item)) {
                    add(spec.label.to_string(), row_words(columns, gone, env), "taken away".to_string());
                }

                for came in after.iter().filter(|item| !before.contains(item)) {
                    add(spec.label.to_string(), "not listed".to_string(), row_words(columns, came, env));
                }
            }
            Control::Switch { .. } => {
                let on = |value: Option<&Value>| value.is_some_and(|value| !value.is_null());

                if on(old) != on(new) {
                    add(spec.label.to_string(), words(spec, old, env), words(spec, new, env));
                }
            }
            _ => {
                if !same(old, new.unwrap_or(&Value::Null)) {
                    add(spec.label.to_string(), words(spec, old, env), words(spec, new, env));
                }
            }
        }
    }

    changes
}

/// The errors no field shown says: of the form as a whole.
pub fn general<'a>(built: &'a Built, kind: &str, draft: &Draft) -> Vec<&'a str> {
    let fields = shown(kind, draft);

    built
        .errors
        .iter()
        .filter(|(path, _)| {
            let root = path.split('#').next().unwrap_or_default();

            path.as_str() == GENERAL || !fields.iter().any(|spec| spec.path == root)
        })
        .map(|(_, why)| why.as_str())
        .collect()
}

/// The form: each field a kind shows now, under its heading, with what stands beside it.
pub fn draw(ui: &mut Ui, palette: &theme::Palette, kind: &str, draft: &mut Draft, standing: Option<&Value>, env: &Env, built: &Built) {
    if kind == "collections" && theme::quiet(ui, palette, "Fill in the standard lists").on_hover_text("The NFT and multi-token standards' own functions").clicked() {
        let standard = CollectionFunctions::standard();
        draft.lists.insert("nft".to_string(), standard.nft);
        draft.lists.insert("multi".to_string(), standard.multi);
    }

    let mut group = "";

    for spec in shown(kind, draft) {
        if spec.group != group && !spec.group.is_empty() {
            ui.add_space(8.0);
            theme::caps(ui, palette, spec.group);
        }

        group = spec.group;
        ui.push_id(("governance field", spec.path), |ui| field(ui, palette, kind, &spec, draft, standing, env, built));
        ui.add_space(6.0);
    }
}

/// The form's text line: as wide as there is, up to a number's fill of a wide screen.
fn line_edit<'a>(text: &'a mut String, hint: &'a str, mono: bool) -> egui::TextEdit<'a> {
    let edit = egui::TextEdit::singleline(text).hint_text(hint).desired_width(f32::INFINITY);

    match mono {
        true => edit.font(egui::TextStyle::Monospace),
        false => edit,
    }
}

/// One field: its name and what stands on one line, what it is for, its control, what is wrong.
#[allow(clippy::too_many_arguments)]
fn field(ui: &mut Ui, palette: &theme::Palette, kind: &str, spec: &Spec, draft: &mut Draft, standing: Option<&Value>, env: &Env, built: &Built) {
    let old = standing.and_then(|standing| at(standing, spec.path));
    let unchanged = draft.unchanged(spec, standing, env);
    let label = match spec.control {
        Control::Amount => format!("{}, in {}", spec.label, env.symbol),
        _ => spec.label.to_string(),
    };
    let mut put_back = false;

    theme::end_row(
        ui,
        |ui| {
            ui.add(egui::Label::new(theme::strong(label, 13.5).color(palette.ink)).wrap());
        },
        |ui| {
            let now = format!("now {}", short_words(spec, old, env));
            ui.add(egui::Label::new(RichText::new(now).small().color(if unchanged { palette.muted } else { palette.accent })).truncate());

            if !unchanged && !spec.fixed && ui.link(RichText::new("Put back").small()).clicked() {
                put_back = true;
            }
        },
    );

    if put_back {
        let start = defaults(kind);
        draft.fill(spec, at(&start, spec.path).unwrap_or(&Value::Null), env);

        if let Some(old) = old {
            draft.fill(spec, old, env);
        }
    }

    if !spec.help.is_empty() {
        ui.add(egui::Label::new(RichText::new(spec.help).small().color(palette.muted)).wrap());
    }

    let price_hint = format!("1.5 {}", env.symbol);
    let bad = |ui: &mut Ui, key: &str| {
        if let Some(why) = built.errors.get(key) {
            ui.add(egui::Label::new(RichText::new(why).small().color(palette.bad)).wrap());
        }
    };

    ui.scope(|ui| {
        ui.set_max_width(ui.available_width().min(FIELD_MOST));

        if spec.fixed {
            fixed(ui, palette, spec, old, env);
        } else {
            control(ui, palette, spec, draft, &price_hint, built);
        }
    });

    bad(ui, spec.path);
}

/// A field that stays as it is: what it says, and no way to change it.
fn fixed(ui: &mut Ui, palette: &theme::Palette, spec: &Spec, old: Option<&Value>, env: &Env) {
    let empty = Vec::new();

    match spec.control {
        Control::Table(columns) => {
            for row in old.and_then(Value::as_array).unwrap_or(&empty) {
                ui.add(egui::Label::new(RichText::new(row_words(columns, row, env)).small().monospace().color(palette.soft)).wrap());
            }
        }
        _ => {
            ui.label(RichText::new("Stays as it is").small().color(palette.soft));
        }
    }
}

/// A field's value in words, cut short to sit beside its name.
fn short_words(spec: &Spec, value: Option<&Value>, env: &Env) -> String {
    let said = words(spec, value, env);

    match said.len() > 24 {
        true => format::short(&said),
        false => said,
    }
}

/// The control a field takes.
fn control(ui: &mut Ui, palette: &theme::Palette, spec: &Spec, draft: &mut Draft, price_hint: &str, built: &Built) {
    let path = spec.path;

    match spec.control {
        Control::Choice(options) => {
            let mut picked = options.iter().position(|(value, _)| *value == draft.text(path)).unwrap_or(0);
            let shown: Vec<(usize, &str)> = options.iter().enumerate().map(|(place, (_, label))| (place, *label)).collect();

            if theme::segmented_rows(ui, palette, &mut picked, &shown) {
                draft.texts.insert(path.to_string(), options[picked].0.to_string());
            }
        }
        Control::MaybeChoice { off, options } => {
            let mut picked = options.iter().position(|(value, _)| *value == draft.text(path)).map_or(0, |place| place + 1);
            let shown: Vec<(usize, &str)> = std::iter::once((0, off)).chain(options.iter().enumerate().map(|(place, (_, label))| (place + 1, *label))).collect();

            if theme::segmented_rows(ui, palette, &mut picked, &shown) {
                draft.texts.insert(path.to_string(), picked.checked_sub(1).map_or_else(String::new, |place| options[place].0.to_string()));
            }
        }
        Control::MaybeWhole { off, on } | Control::MaybeBytes { off, on } => {
            let hint = if matches!(spec.control, Control::MaybeBytes { .. }) { "4 MiB" } else { "210000" };
            let turned = draft.switches.entry(path.to_string()).or_default();
            theme::segmented(ui, palette, turned, &[(false, off), (true, on)]);

            if *turned {
                ui.add(line_edit(draft.texts.entry(path.to_string()).or_default(), hint, false));
            }
        }
        Control::Switch { off, on } | Control::Flag { off, on } => {
            theme::segmented(ui, palette, draft.switches.entry(path.to_string()).or_default(), &[(false, off), (true, on)]);
        }
        Control::List(line) => list(ui, palette, spec, line, draft, price_hint, built),
        Control::Table(columns) => table(ui, palette, spec, columns, draft, built),
        other => {
            let (hint, mono) = match other {
                Control::Amount => (price_hint, false),
                Control::Bytes => ("32 MiB", false),
                Control::Key => ("ed25519:0x…", true),
                Control::Contract | Control::MaybeContract => ("contract:0x… or empty", true),
                Control::MaybeEvm => ("0x… or empty", true),
                Control::Version => ("0.2.0", false),
                Control::Text => ("", false),
                _ => ("0", false),
            };

            ui.add(line_edit(draft.texts.entry(path.to_string()).or_default(), hint, mono));
        }
    }
}

/// A list: a line each, with Remove, and Add under them.
fn list(ui: &mut Ui, palette: &theme::Palette, spec: &Spec, line: Line, draft: &mut Draft, price_hint: &str, built: &Built) {
    let lines = draft.lists.entry(spec.path.to_string()).or_default();
    let count = lines.len();
    let mut gone = None;
    let (hint, mono) = match line {
        Line::Bootnode => ("/ip4/203.0.113.7/udp/4001/quic-v1", true),
        Line::Function => ("owner_of", true),
        Line::TxKind => ("call", true),
        Line::Compressed => ("0x02…", true),
        Line::Price => (price_hint, false),
        Line::Account => ("ed25519:0x…", true),
    };

    for (index, typed) in lines.iter_mut().enumerate() {
        ui.push_id(index, |ui| {
            if line == Line::Price {
                ui.label(RichText::new(price_label(index, count - 1)).small().color(palette.soft));
            }

            let remove = theme::beside(
                ui,
                |ui| theme::quiet(ui, palette, "Remove").clicked(),
                |ui, width| {
                    ui.add(line_edit(typed, hint, mono).desired_width(width));
                },
            );

            if remove {
                gone = Some(index);
            }

            if let Some(why) = built.errors.get(&line_key(spec.path, index)) {
                ui.add(egui::Label::new(RichText::new(why).small().color(palette.bad)).wrap());
            }
        });
    }

    if let Some(index) = gone {
        lines.remove(index);
    }

    if theme::quiet(ui, palette, "Add").clicked() {
        lines.push(String::new());
    }
}

/// A list of objects: a field for each column, with Remove, and Add under them.
fn table(ui: &mut Ui, palette: &theme::Palette, spec: &Spec, columns: &[Column], draft: &mut Draft, built: &Built) {
    let rows = draft.tables.entry(spec.path.to_string()).or_default();
    let mut gone = None;

    for (index, row) in rows.iter_mut().enumerate() {
        ui.push_id(index, |ui| {
            row.resize(columns.len(), String::new());

            egui::Frame::new().stroke(egui::Stroke::new(1.0, palette.line)).corner_radius(6.0).inner_margin(8.0).show(ui, |ui| {
                ui.set_width(ui.available_width());

                for (column, typed) in columns.iter().zip(row.iter_mut()) {
                    ui.label(RichText::new(column.label).small().color(palette.soft));
                    ui.add(line_edit(typed, "", column.cell != Cell::Whole));

                    if let Some(why) = built.errors.get(&cell_key(spec.path, index, column.name)) {
                        ui.add(egui::Label::new(RichText::new(why).small().color(palette.bad)).wrap());
                    }
                }

                if theme::quiet(ui, palette, "Remove").clicked() {
                    gone = Some(index);
                }
            });
        });
    }

    if let Some(index) = gone {
        rows.remove(index);
    }

    if theme::quiet(ui, palette, "Add").clicked() {
        rows.push(vec![String::new(); columns.len()]);
    }
}

#[cfg(test)]
mod tests;
