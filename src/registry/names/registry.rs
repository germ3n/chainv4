//! The registry itself, in a node's state: lookups, and what name transactions change.

use {
    super::*,
    crate::db::{Column, Database, DbError, State},
    crate::registry::{Accounts, RegistryError},
};

#[document(
    summary = "The name registry in a state layer: lookups, reverse lookups, and the changes name transactions make, keeping each owner's listing in step.",
    category = "Names",
    see_also = "NamesConfig, Tx::Name, Tx::PrimaryName",
)]
pub struct Names<'a> {
    state: &'a State,
}

impl<'a> Names<'a> {
    #[document(parent = "Names", summary = "The registry in a state layer.")]
    pub fn new(state: &'a State) -> Names<'a> {
        Names { state }
    }

    #[document(parent = "Names", summary = "A name's record as kept, whether or not it stands now.")]
    pub fn record(&self, name: &str) -> Result<Option<NameRecord>, DbError> {
        self.state.get_value(Column::Names, &record_key(name))
    }

    #[document(
        parent = "Names",
        summary = "A name or subname that stands at a time, as standing judges it from the records of its lineage.",
        params = {
            name = "The name, without the suffix.",
            now = "The time, in milliseconds since the Unix epoch.",
            grace_ms = "The chain's grace period.",
        },
        returns = "The name, or None when it is available.",
    )]
    pub fn lookup(&self, name: &str, now: u64, grace_ms: u64) -> Result<Option<FoundName>, DbError> {
        let mut records = Vec::new();

        for name in lineage(name) {
            let record = self.record(&name)?;
            let missing = record.is_none();
            records.push(record);

            if missing {
                break;
            }
        }

        Ok(standing(&records, now, grace_ms))
    }

    #[document(parent = "Names", summary = "The address a name resolves to at a time: its target while it is active, None otherwise.")]
    pub fn resolve(&self, name: &str, now: u64, grace_ms: u64) -> Result<Option<Address>, DbError> {
        Ok(self
            .lookup(name, now, grace_ms)?
            .filter(|found| found.status == NameStatus::Active)
            .and_then(|found| found.record.target))
    }

    #[document(
        parent = "Names",
        summary = "An account's primary name at a time, for reverse lookups: the name it chose, while that name still resolves to it.",
        returns = "The name, without the suffix, or None.",
    )]
    pub fn primary(&self, address: &Address, now: u64, grace_ms: u64) -> Result<Option<String>, DbError> {
        let Some(PrimaryName { name }) = self.state.get_value::<PrimaryName>(Column::Names, &primary_key(address))? else {
            return Ok(None);
        };

        Ok((self.resolve(&name, now, grace_ms)?.as_ref() == Some(address)).then_some(name))
    }

    #[document(
        parent = "Names",
        summary = "Runs a name transaction's action for its sender.",
        params = {
            sender = "Who sent it.",
            name = "The name it is for, without the suffix.",
            action = "What it does.",
            now = "Its block's time, in milliseconds since the Unix epoch.",
            height = "Its block's height, which a registration remembers.",
            config = "The chain's names.",
            prices = "The prices in force: genesis's, or those governance decided last.",
        },
        returns = "What it burned, or why it cannot go through: as a RegistryError::Name with the reason, or for want of balance.",
    )]
    #[allow(clippy::too_many_arguments)]
    pub fn act(&self, sender: &Address, name: &str, action: &NameAction, now: u64, height: u64, config: &NamesConfig, prices: &[u256]) -> Result<u256, RegistryError> {
        let refuse = |why: String| Err(RegistryError::Name(why));
        let shown = config.display(name);

        if let Some(problem) = name_problem(name).or_else(|| action.problem()) {
            return refuse(problem);
        }

        let found = self.lookup(name, now, config.grace_ms)?;
        let top_level = !name.contains('.');

        match action {
            NameAction::Register { years } => {
                if !top_level {
                    return refuse(format!("{shown} is a subname: its name's owner creates it"));
                }

                if let Some(found) = found {
                    return match found.record.owner == *sender {
                        true => refuse(format!("{shown} is yours already: renew it")),
                        false => refuse(format!("{shown} is taken")),
                    };
                }

                if *years > config.max_years {
                    return refuse(format!("a name is held at most {} years ahead", config.max_years));
                }

                let cost = price(prices, name, *years).ok_or_else(|| RegistryError::Name("the price overflows".to_string()))?;
                Accounts::new(self.state).debit(sender, cost)?;
                self.put(name, &NameRecord {
                    owner: *sender,
                    target: Some(*sender),
                    expires: now.saturating_add(u64::from(*years).saturating_mul(YEAR_MS)),
                    root: height,
                    texts: Vec::new(),
                })?;

                return Ok(cost);
            }
            NameAction::Renew { years } => {
                let Some(found) = found else {
                    return refuse(format!("nobody holds {shown}: register it"));
                };

                if !top_level {
                    return refuse(format!("{shown} is a subname: it lasts as long as its name"));
                }

                if found.record.owner != *sender {
                    return refuse(format!("{shown} is not yours"));
                }

                let expires = found.expires.saturating_add(u64::from(*years).saturating_mul(YEAR_MS));

                if expires > now.saturating_add(u64::from(config.max_years).saturating_mul(YEAR_MS)) {
                    return refuse(format!("a name is held at most {} years ahead", config.max_years));
                }

                let cost = price(prices, name, *years).ok_or_else(|| RegistryError::Name("the price overflows".to_string()))?;
                Accounts::new(self.state).debit(sender, cost)?;
                self.put(name, &NameRecord { expires, ..found.record })?;

                return Ok(cost);
            }
            _ => {}
        }

        // Everything else changes a name that is active, and its owner's.
        let mut record = match found {
            Some(found) if found.status == NameStatus::Active && found.record.owner == *sender => found.record,
            Some(found) if found.status == NameStatus::Active => return refuse(format!("{shown} is not yours")),
            Some(_) => return refuse(format!("{shown} has expired: renew it first")),
            None => return refuse(format!("nobody holds {shown}")),
        };

        match action {
            NameAction::Transfer { to } => record.owner = *to,
            NameAction::Target { target } => record.target = *target,
            NameAction::Text { key, value } => {
                record.texts.retain(|text| text.key != *key);

                if let Some(value) = value {
                    if record.texts.len() >= MAX_TEXTS {
                        return refuse(format!("a name holds at most {MAX_TEXTS} text records"));
                    }

                    let at = record.texts.partition_point(|text| text.key < *key);
                    record.texts.insert(at, TextRecord { key: key.clone(), value: value.clone() });
                }
            }
            NameAction::Subname { label, owner } => {
                let subname = format!("{label}.{name}");

                if let Some(problem) = name_problem(&subname).or_else(|| (config.display(&subname).len() > MAX_NAME_LENGTH).then(|| format!("a name holds at most {MAX_NAME_LENGTH} characters"))) {
                    return refuse(problem);
                }

                self.put(&subname, &NameRecord {
                    owner: *owner,
                    target: Some(*owner),
                    expires: 0,
                    root: record.root,
                    texts: Vec::new(),
                })?;

                return Ok(u256::ZERO);
            }
            NameAction::Register { .. } | NameAction::Renew { .. } => unreachable!("handled above"),
        }

        self.put(name, &record)?;

        Ok(u256::ZERO)
    }

    #[document(
        parent = "Names",
        summary = "Sets the sender's primary name, which must resolve to the sender, or clears it.",
        returns = "Nothing, or why not: a name that does not resolve to the sender.",
    )]
    pub fn set_primary(&self, sender: &Address, name: Option<&str>, now: u64, config: &NamesConfig) -> Result<(), RegistryError> {
        let key = primary_key(sender);
        let Some(name) = name else {
            self.state.delete(Column::Names, key);

            return Ok(());
        };

        if self.resolve(name, now, config.grace_ms)?.as_ref() != Some(sender) {
            return Err(RegistryError::Name(format!("{} does not resolve to {sender}", config.display(name))));
        }

        self.state.put_value(Column::Names, key, &PrimaryName { name: name.to_string() });

        Ok(())
    }

    /// Writes a record, moving the name in the owners' listings when its owner changes.
    fn put(&self, name: &str, record: &NameRecord) -> Result<(), DbError> {
        if let Some(old) = self.record(name)?
            && old.owner != record.owner
        {
            self.state.delete(Column::Names, owned_key(&old.owner, name));
        }

        self.state.put(Column::Names, owned_key(&record.owner, name), Vec::new());
        self.state.put_value(Column::Names, record_key(name), record);

        Ok(())
    }
}


#[document(
    summary = "The names an account owns, in the committed state, in name order from start: names and subnames whose record names it owner, standing or not, which the caller judges with Names::lookup.",
    params = {
        db = "The database.",
        owner = "The account.",
        start = "Where to start, a name; empty for the first.",
        limit = "The most to list.",
    },
    returns = "The names, without the suffix, and the name to start the next page at, if any.",
    category = "Names",
    see_also = "owned_prefix",
)]
pub fn stored_owned_names(db: &dyn Database, owner: &Address, start: &str, limit: usize) -> Result<(Vec<String>, Option<String>), DbError> {
    let prefix = owned_prefix(owner);
    let from = [prefix.clone(), start.as_bytes().to_vec()].concat();
    let mut names = Vec::new();
    let mut next = None;

    db.scan(Column::Names, &prefix, &from, &mut |key, _| {
        let name = String::from_utf8_lossy(&key[prefix.len()..]).into_owned();

        if names.len() == limit {
            next = Some(name);

            return false;
        }

        names.push(name);

        true
    })?;

    Ok((names, next))
}
