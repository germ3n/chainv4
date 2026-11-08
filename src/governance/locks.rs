//! Locks: settings no vote may change. Genesis names the first, `Change::Locks` adds more, and none
//! is ever taken away. A lock is a path: a change's name, alone or followed by the dotted names of
//! fields as `change_json` writes the change's value, such as `reward`, `anchoring.enforce` or
//! `limits.tx_limits.fuel`. A change whose value at a locked path differs from what stands there is
//! refused, compared as JSON, so every kind is held the same way. Plain data and checks, in every
//! build, so the CLI and the apps refuse what the chain would before anything is sent.

use r#macro::document;

#[document(
    summary = "The names a lock's path may start with: every change's name, as change_json writes it, and locks itself, which freezes the set once it is locked.",
    category = "Governance",
    see_also = "path_problem, Change::Locks",
)]
pub const ROOTS: [&str; 24] = [
    "consensus",
    "bootnodes",
    "names",
    "collections",
    "federation",
    "directory",
    "fees",
    "reward",
    "limits",
    "finality",
    "switch_finality",
    "upgrade",
    "name",
    "governance",
    "token_functions",
    "evm_spec",
    "dex",
    "anchoring",
    "locks", "engines", "fork", "proofs", "attestations", "btc_path"];

/// Fees' fields, each variant's and its host costs' among them.
const FEES: [&str; 48] = [
    "block_gas_limit",
    "burn_permille",
    "vote_rewards",
    "schedule.type",
    "schedule.per_tx",
    "schedule.per_byte",
    "schedule.per_code_byte",
    "schedule.fuel_per_gas",
    "schedule.contract",
    "schedule.fuel",
    "schedule.kinds",
    "pricing.type",
    "pricing.initial",
    "pricing.min",
    "pricing.elasticity",
    "pricing.change_denominator",
    "pricing.min_price",
    "costs.storage_read",
    "costs.storage_write",
    "costs.storage_bytes_per_fuel",
    "costs.storage_scan",
    "costs.storage_scan_entry",
    "costs.hash",
    "costs.hash_bytes_per_fuel",
    "costs.call",
    "costs.balance",
    "costs.transfer",
    "costs.event",
    "costs.event_bytes_per_fuel",
    "costs.copy_bytes_per_fuel",
    "costs.load_byte",
    "costs.token_refresh",
    "costs.check_layout",
    "costs.clone",
    "costs.name",
    "costs.abi",
    "costs.verify",
    "costs.recover",
    "costs.key_address",
    "costs.check_proof",
    "costs.proof_sibling",
    "costs.federation",
    "costs.chain_id",
    "costs.evm_native_call",
    "costs.evm_native_word",
    "costs.evm_name_lookup",
    "costs.bls_verify",
    "costs.bls_key",
];

#[document(
    summary = "The fields under a change's name a lock may name, each variant's among them, as change_json writes the change's value, leaves alone: an object's own path, such as fees.costs or limits.tx_limits, is one too, as the start of one of them. Empty for a change whose value has no fields, a list or a string, which is locked whole; None for a name that is no change's.",
    category = "Governance",
    see_also = "path_problem, ROOTS",
    note = "switch_finality has none: no switch stands to compare one with, so it is locked whole, and what a switch sets is held by the locks under finality, which a switch's finality is checked against.",
)]
pub fn fields(root: &str) -> Option<&'static [&'static str]> {
    Some(match root {
        "consensus" => &["type", "hash", "target_block_ms", "initial_difficulty", "min_difficulty", "window", "adjustment", "max_steps_down", "max_swing", "key", "slot_ms", "schedule"],
        "names" => &["prices", "grace_ms", "max_years"],
        "collections" => &["nft", "multi"],
        "federation" => &[
            "signers",
            "threshold",
            "chain_depth",
            "evm_depth",
            "bitcoin_depth",
            "btc.fee_cap",
            "btc.fee_floor",
            "btc.grace_ms",
            "btc.cancel_after_ms",
            "btc.fee_part",
            "btc.bump_after_ms",
            "btc.bump_below_pct",
            "btc.repay_pct",
            "btc.repay_most",
        ],
        "directory" => &["gateway", "btc_gateway", "evm_gateways", "swap", "dex", "dex_router", "dex_split", "nft_factory"],
        "fees" => &FEES,
        "reward" => &["initial", "halving_interval"],
        "limits" => &[
            "tx_limits.fuel",
            "tx_limits.memory",
            "max_block_bytes",
            "max_tx_bytes",
            "multisig_members",
            "contract.call_depth",
            "contract.key_len",
            "contract.value_len",
            "contract.scan",
            "contract.event_data",
            "contract.events",
            "contract.module_len",
            "contract.source_len",
        ],
        "finality" => &[
            "type",
            "checkpoint_interval",
            "activation_delay",
            "vote_depth",
            "min_bond",
            "withdrawal_delay",
            "reporter_divisor",
            "max_validators",
            "vote_reward",
            "max_proofs_per_block",
            "validators",
            "slash_permille",
            "leak.after_epochs",
            "leak.permille",
            // From the proofs fork on: refused in a vote before it.
            "justify_permille",
            "max_signers",
            "signers",
            "threshold",
            "change_threshold",
            "approval_epochs",
            "depth",
        ],
        "upgrade" => &["least", "from"],
        // Nothing stands for the fork schedule to compare a change with: fork whole freezes it,
        // fork.blocks the lead alone, and so on.
        "fork" => &["type", "name", "height", "epoch", "least", "blocks"],
        "proofs" => &["kinds", "program", "reward_per_block", "gateway", "finality_program", "headers_program", "reward_per_epoch", "withdrawal_program"],
        "governance" => &["type", "members", "threshold", "approval_epochs", "quorum_permille"],
        "evm_spec" => &["spec", "from"],
        "dex" => &["tiers", "protocol_permille"],
        "anchoring" => &["bitcoin", "ethereum", "enforce", "relay", "btc_start.header", "btc_start.height", "btc_start.network", "eth_relay", "eth_start.root", "eth_start.network", "depth", "every", "reward"],
        "bootnodes" | "name" | "token_functions" | "switch_finality" | "locks" | "engines" | "attestations" | "btc_path" => &[],
        _ => return None,
    })
}

#[document(
    summary = "Why a path is no lock: empty, a name between dots empty or other than lowercase letters, digits and underscores, a first name that is no change's, or what follows it naming no field of that change, nor an object holding one. None for a path a lock may name.",
    category = "Governance",
    see_also = "fields, ROOTS, Genesis::locked",
)]
pub fn path_problem(path: &str) -> Option<String> {
    let names: Vec<&str> = path.split('.').collect();

    if names.iter().any(|name| name.is_empty() || !name.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')) {
        return Some(format!("{path:?} is no path: names of lowercase letters, digits and underscores, joined by dots, such as anchoring.enforce"));
    }

    let (root, rest) = path.split_once('.').map_or((path, None), |(root, rest)| (root, Some(rest)));
    let Some(fields) = fields(root) else {
        return Some(format!("{root} is not something governance changes; a lock starts with one of {}", ROOTS.join(", ")));
    };
    let rest = rest?;

    if root == "switch_finality" {
        return Some("switch_finality is locked whole: no switch stands to compare one with; lock finality's fields to hold what a switch sets".to_string());
    }

    match fields.iter().any(|field| *field == rest || field.strip_prefix(rest).is_some_and(|after| after.starts_with('.'))) {
        true => None,
        false => Some(format!("{path} names nothing in {root}")),
    }
}

#[document(
    summary = "Whether a lock holds a path: the same path, or one under it, as reward holds reward.initial.",
    category = "Governance",
    see_also = "path_problem",
)]
pub fn covers(lock: &str, path: &str) -> bool {
    path == lock || path.strip_prefix(lock).is_some_and(|rest| rest.starts_with('.'))
}

#[document(
    summary = "Why a list of locks cannot be genesis's: a path that names nothing, or one named twice.",
    category = "Governance",
    see_also = "Genesis::locked, path_problem",
)]
pub fn genesis_problem(locked: &[String]) -> Option<String> {
    for (at, path) in locked.iter().enumerate() {
        if let Some(problem) = path_problem(path) {
            return Some(format!("locked: {problem}"));
        }

        if locked[..at].contains(path) {
            return Some(format!("locked names {path} twice"));
        }
    }

    None
}

#[document(
    summary = "Why a change of locks cannot add its paths to those that stand: it names none, they are not in order each once, one names nothing, or one is locked already, by itself or a lock over it. A lock on locks itself refuses every change of locks, as any lock refuses what it holds.",
    category = "Governance",
    see_also = "Change::Locks, path_problem",
    params = { standing = "The locks that stand.", adding = "The paths the change adds." },
)]
pub fn adding_problem(standing: &[String], adding: &[String]) -> Option<String> {
    if adding.is_empty() {
        return Some("a change of locks names at least one path to lock".to_string());
    }

    if adding.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Some("a change of locks names its paths in order, each once".to_string());
    }

    for path in adding {
        if let Some(problem) = path_problem(path) {
            return Some(problem);
        }

        if let Some(lock) = standing.iter().find(|lock| covers(lock, path)) {
            return Some(match lock == path {
                true => format!("{path} is locked already"),
                false => format!("{path} is locked already, under {lock}"),
            });
        }
    }

    None
}

#[document(
    summary = "The lock that holds a kind of change whole, if one does: one on its name, so the propose forms show the kind locked. A switch of finality is held whole by a lock on finality or its type too, since every switch changes the kind.",
    category = "Governance",
    see_also = "covers",
)]
pub fn whole<'l>(locks: &'l [String], change: &str) -> Option<&'l str> {
    locks
        .iter()
        .find(|lock| lock.as_str() == change || (change == "switch_finality" && matches!(lock.as_str(), "finality" | "finality.type")))
        .map(String::as_str)
}

#[cfg(feature = "json")]
#[document(
    summary = "The first lock a change breaks: a locked path where its value differs from what stands, compared as JSON, a field left out the same as null. A change of another variant breaks a lock under its kind as soon as what it holds there differs, as a field the new variant lacks, or one it adds, does. A switch of finality is held by the locks on switch_finality and, against the finality that stands, by those under finality.",
    category = "Governance",
    see_also = "path_problem, Change::Locks",
    params = {
        locks = "The locks that stand.",
        change = "The change, as change_json writes it: one field, named for what it changes.",
        standing = "What stands under a name, as change_json writes its value: GET /governance's in_force, or null where nothing does.",
    },
)]
pub fn broken<'l>(locks: &'l [String], change: &serde_json::Value, standing: &dyn Fn(&str) -> serde_json::Value) -> Option<&'l str> {
    use serde_json::Value;

    let (name, value) = change.as_object().and_then(|object| object.iter().next())?;
    let mut checked: Vec<(&str, &Value)> = vec![(name.as_str(), value)];

    // A switch sets finality whole: what finality's locks hold, it must keep.
    if name == "switch_finality" {
        checked.push(("finality", &value["finality"]));
    }

    for lock in locks {
        let (root, rest) = lock.split_once('.').map_or((lock.as_str(), ""), |(root, rest)| (root, rest));

        for (under, proposed) in &checked {
            if root != *under {
                continue;
            }

            let stands = standing(root);

            if at(&stands, rest) != at(proposed, rest) {
                return Some(lock);
            }
        }
    }

    None
}

#[cfg(feature = "json")]
/// What a value holds at a dotted path, None for nothing there or null.
fn at<'v>(value: &'v serde_json::Value, path: &str) -> Option<&'v serde_json::Value> {
    let mut here = value;

    for name in path.split('.').filter(|name| !name.is_empty()) {
        here = here.get(name)?;
    }

    (!here.is_null()).then_some(here)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_names_a_change_and_then_its_fields_or_an_object_holding_them() {
        for good in ["reward", "anchoring.enforce", "federation.threshold", "fees.burn_permille", "limits.tx_limits.fuel", "limits.tx_limits", "fees.costs", "fees.costs.bls_key", "fees.pricing.min_price", "consensus.target_block_ms", "switch_finality", "upgrade", "locks", "finality.leak"] {
            assert_eq!(path_problem(good), None, "{good}");
        }

        for (bad, says) in [
            ("", "no path"),
            ("reward.", "no path"),
            ("fees..burn_permille", "no path"),
            ("Fees", "no path"),
            ("colour", "not something governance changes"),
            ("name_prices", "not something governance changes"),
            ("reward.initia", "names nothing in reward"),
            ("limits.tx_limits.fuel.more", "names nothing in limits"),
            ("bootnodes.0", "names nothing in bootnodes"),
            ("switch_finality.epoch", "locked whole"),
            ("locks.paths", "names nothing in locks"),
        ] {
            assert!(path_problem(bad).is_some_and(|problem| problem.contains(says)), "{bad}: {:?}", path_problem(bad));
        }

    }

    #[test]
    fn genesis_names_each_lock_once_and_a_change_adds_only_new_ones_in_order() {
        let paths = |list: &[&str]| list.iter().map(|path| path.to_string()).collect::<Vec<_>>();

        assert_eq!(genesis_problem(&paths(&["reward", "fees.burn_permille"])), None);
        assert!(genesis_problem(&paths(&["reward", "reward"])).is_some_and(|problem| problem.contains("twice")));
        assert!(genesis_problem(&paths(&["reward.nothing"])).is_some_and(|problem| problem.starts_with("locked: ")));

        let standing = paths(&["fees", "reward.initial"]);
        assert_eq!(adding_problem(&standing, &paths(&["anchoring.enforce", "reward.halving_interval"])), None);
        assert!(adding_problem(&standing, &[]).is_some());
        assert!(adding_problem(&standing, &paths(&["reward.halving_interval", "anchoring.enforce"])).is_some_and(|problem| problem.contains("in order")));
        assert!(adding_problem(&standing, &paths(&["dex", "dex"])).is_some_and(|problem| problem.contains("each once")));
        assert_eq!(adding_problem(&standing, &paths(&["reward.initial"])).as_deref(), Some("reward.initial is locked already"));
        assert_eq!(adding_problem(&standing, &paths(&["fees.burn_permille"])).as_deref(), Some("fees.burn_permille is locked already, under fees"));
        assert_eq!(adding_problem(&standing, &paths(&["reward"])), None, "a wider lock over a narrower one");
    }

    #[test]
    fn a_kind_locked_whole_is_its_name_and_a_switch_is_held_by_finality_s_kind_too() {
        let locks = vec!["reward".to_string(), "finality.type".to_string(), "fees.burn_permille".to_string()];

        assert_eq!(whole(&locks, "reward"), Some("reward"));
        assert_eq!(whole(&locks, "fees"), None, "a field of fees is not all of it");
        assert_eq!(whole(&locks, "switch_finality"), Some("finality.type"));
        assert_eq!(whole(&locks, "finality"), None);
        assert!(covers("reward", "reward.initial") && covers("reward", "reward") && !covers("reward", "rewards") && !covers("reward.initial", "reward"));
    }

    #[cfg(feature = "json")]
    #[test]
    fn a_lock_holds_its_field_while_the_rest_of_the_kind_changes() {
        use serde_json::{Value, json};

        let stands = json!({ "initial": "50", "halving_interval": null });
        let standing = |name: &str| if name == "reward" { stands.clone() } else { Value::Null };
        let locks = vec!["reward.initial".to_string()];

        assert_eq!(broken(&locks, &json!({ "reward": { "initial": "50", "halving_interval": 1000 } }), &standing), None);
        assert_eq!(broken(&locks, &json!({ "reward": { "initial": "60", "halving_interval": null } }), &standing), Some("reward.initial"));
        assert_eq!(broken(&locks, &json!({ "fees": { "burn_permille": 600 } }), &standing), None, "a lock holds its own kind alone");

        // Left out and null are the same.
        let locks = vec!["reward.halving_interval".to_string()];
        assert_eq!(broken(&locks, &json!({ "reward": { "initial": "60" } }), &standing), None);
    }

    #[cfg(feature = "json")]
    #[test]
    fn variant_kinds_are_held_by_what_each_variant_holds_at_the_path() {
        use serde_json::{Value, json};

        let authority = json!({ "type": "authority", "key": "ed25519:0x01", "slot_ms": 2000 });
        let finality = json!({ "type": "ffg", "checkpoint_interval": 8, "vote_reward": "5", "max_validators": 10 });
        let standing = |name: &str| match name {
            "consensus" => authority.clone(),
            "finality" => finality.clone(),
            _ => Value::Null,
        };
        let lock = |path: &str| vec![path.to_string()];

        // A lock on the consensus refuses any change of it.
        assert_eq!(broken(&lock("consensus"), &json!({ "consensus": { "type": "authority", "key": "ed25519:0x02", "slot_ms": 2000 } }), &standing), Some("consensus"));

        // A field of one variant: kept, a change of key goes; another variant without it does not.
        let slot = lock("consensus.slot_ms");
        assert_eq!(broken(&slot, &json!({ "consensus": { "type": "authority", "key": "ed25519:0x02", "slot_ms": 2000 } }), &standing), None);
        assert_eq!(broken(&slot, &json!({ "consensus": { "type": "staked", "slot_ms": 2000, "schedule": "draw" } }), &standing), None, "staked holds slot_ms too");
        assert_eq!(broken(&slot, &json!({ "consensus": { "type": "pow", "hash": "blake3", "target_block_ms": 2000 } }), &standing), Some("consensus.slot_ms"), "proof of work has none");

        // A field the standing variant lacks keeps it lacking.
        let target = lock("consensus.target_block_ms");
        assert_eq!(broken(&target, &json!({ "consensus": { "type": "staked", "slot_ms": 1000, "schedule": "draw" } }), &standing), None);
        assert_eq!(broken(&target, &json!({ "consensus": { "type": "pow", "hash": "blake3", "target_block_ms": 2000 } }), &standing), Some("consensus.target_block_ms"));

        // A switch: refused whole by its own lock, and held to what finality's locks hold.
        let switch = json!({ "switch_finality": { "epoch": 40, "finality": { "type": "authority", "checkpoint_interval": 8, "vote_reward": "5" } } });
        assert_eq!(broken(&lock("switch_finality"), &switch, &standing), Some("switch_finality"));
        assert_eq!(broken(&lock("finality.vote_reward"), &switch, &standing), None, "it keeps the vote reward");
        assert_eq!(broken(&lock("finality.type"), &switch, &standing), Some("finality.type"));
        assert_eq!(broken(&lock("finality.max_validators"), &switch, &standing), Some("finality.max_validators"), "authority has no max_validators");

        // An upgrade, with none standing, is refused by any lock on it.
        let upgrade = json!({ "upgrade": { "least": "0.2.0", "from": 500 } });
        assert_eq!(broken(&lock("upgrade"), &upgrade, &standing), Some("upgrade"));
        assert_eq!(broken(&lock("upgrade.least"), &upgrade, &standing), Some("upgrade.least"));
    }
}
