//! The fork schedule: rules that change from a height on, so a live chain takes a consensus change
//! without a new genesis. A fork is a name this release knows, with what it changes (FORKS); the
//! chain schedules it, in genesis or by governance (Change::Fork), at a height or at a finality
//! epoch's first block. Every block runs by the forks in force at its height (Forks, in
//! VmRules::forks), so a node replays history under each block's own rules. A node whose release
//! does not know a fork the chain scheduled stops before its height rather than run past it alone.

use {
    crate::crypto::hashable::Hashable,
    crate::structs::version::Version,
    r#macro::document,
    wincode::{SchemaRead, SchemaWrite},
};

#[document(
    summary = "A fork this release knows: its name, which the chain schedules it by, and what it changes. Code that a fork changes asks Forks::has for it.",
    category = "Governance",
    see_also = "FORKS, Forks, ScheduledFork",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fork {
    #[document(summary = "Its name: lowercase letters, digits and dashes.")]
    pub name: &'static str,
    #[document(summary = "What it changes, in a line, as GET /status and the docs list it.")]
    pub summary: &'static str,
}

/// A fork that exists in tests alone: a transfer's intrinsic gas one higher from it on, to show a
/// chain replayed across a fork and nodes on both sides of one.
#[cfg(test)]
pub const TEST_FORK: Fork = Fork { name: "test-fork", summary: "Only in tests: a transfer's intrinsic gas one higher." };

#[document(
    summary = "Every fork this release knows, oldest first: none yet. Each new one goes on the end, and is scheduled by name; a release never drops one, as blocks from its height on run by it.",
    category = "Governance",
    see_also = "Fork, Forks",
)]
#[cfg(not(test))]
pub const FORKS: &[Fork] = &[];

#[cfg(test)]
pub const FORKS: &[Fork] = &[TEST_FORK];

#[document(
    summary = "The most forks a release may know: Forks holds them as the bits of a u64.",
    category = "Governance",
)]
pub const MAX_FORKS: usize = 64;

#[document(
    summary = "The longest name a fork may have.",
    category = "Governance",
)]
pub const MAX_FORK_NAME: usize = 40;

#[document(
    summary = "How far ahead governance must schedule a fork unless the chain says otherwise: a day of the chain's own blocks, so nodes have time to update.",
    category = "Governance",
    see_also = "ForkSchedule::lead",
)]
pub const DEFAULT_LEAD_MS: u64 = 86_400_000;

#[document(
    summary = "The fork this release knows by that name.",
    category = "Governance",
)]
pub fn known(name: &str) -> Option<&'static Fork> {
    FORKS.iter().find(|fork| fork.name == name)
}

#[document(
    summary = "Why a fork's name cannot be one: empty, longer than MAX_FORK_NAME, or holding anything but lowercase letters, digits and dashes.",
    category = "Governance",
)]
pub fn fork_name_problem(name: &str) -> Option<&'static str> {
    if name.is_empty() || name.len() > MAX_FORK_NAME {
        return Some("a fork's name is 1 to 40 characters");
    }

    (!name.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')).then_some("a fork's name is lowercase letters, digits and dashes")
}

#[document(
    summary = "A fork the chain scheduled: its name, the first block that runs by it, the finality epoch that block starts if it was scheduled by one, and the least release that knows it, as governance named it.",
    category = "Governance",
    see_also = "ForkSchedule, Change::Fork",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct ScheduledFork {
    #[document(summary = "The fork's name.")]
    pub name: String,
    #[document(summary = "The first block that runs by it.")]
    pub height: u64,
    #[document(summary = "The epoch it was scheduled at, whose first block height is, or None when it was scheduled at a height.")]
    pub epoch: Option<u64>,
    #[document(summary = "The oldest release that knows it, which governance names as it schedules it; None for one genesis lists.")]
    pub least: Option<Version>,
}

#[document(
    summary = "The forks a chain scheduled, by height, and how far ahead governance must schedule another: genesis's while governance has changed neither, and kept whole in change:forks once it has.",
    category = "Governance",
    see_also = "ScheduledFork, Change::Fork",
)]
#[derive(Clone, Debug, Default, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct ForkSchedule {
    #[document(summary = "Each fork, by height, each name once.")]
    pub forks: Vec<ScheduledFork>,
    #[document(summary = "How many blocks ahead of the vote deciding it a fork must be scheduled, moved or cancelled, or None for DEFAULT_LEAD_MS of the chain's blocks.")]
    pub lead: Option<u64>,
}

impl Hashable for ForkSchedule {}

impl ForkSchedule {
    #[document(
        parent = "ForkSchedule",
        summary = "How many blocks ahead a fork must be scheduled: the chain's own lead, or DEFAULT_LEAD_MS of blocks at its target block time, a block's worth at least.",
        params = { target_block_ms = "The chain's target block time, if its consensus has one." },
    )]
    pub fn lead(&self, target_block_ms: Option<u64>) -> u64 {
        self.lead.unwrap_or_else(|| DEFAULT_LEAD_MS / target_block_ms.unwrap_or(1_000).max(1)).max(1)
    }

    #[document(parent = "ForkSchedule", summary = "The forks in force at a height: each this release knows scheduled at or below it.")]
    pub fn at(&self, height: u64) -> Forks {
        Forks::at(&self.forks, height)
    }

    #[document(parent = "ForkSchedule", summary = "The first fork by height this release does not know, which a node stops before.")]
    pub fn unknown(&self) -> Option<&ScheduledFork> {
        self.forks.iter().filter(|fork| known(&fork.name).is_none()).min_by_key(|fork| fork.height)
    }

    #[document(parent = "ForkSchedule", summary = "Why a genesis cannot list these forks: a name that cannot be one or that this release does not know, or one named twice.")]
    pub fn genesis_problem(&self) -> Option<String> {
        for (at, fork) in self.forks.iter().enumerate() {
            if let Some(why) = fork_name_problem(&fork.name) {
                return Some(why.to_string());
            }

            if known(&fork.name).is_none() {
                return Some(format!("genesis schedules fork {}, which this release does not know: update", fork.name));
            }

            if self.forks[..at].iter().any(|before| before.name == fork.name) {
                return Some(format!("genesis schedules fork {} twice", fork.name));
            }
        }

        None
    }
}

#[document(
    summary = "Where a fork is scheduled: from a height, or from a finality epoch's first block, which is the epoch times the chain's checkpoint interval.",
    category = "Governance",
    see_also = "ForkChange",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
pub enum ForkAt {
    #[document(summary = "From this block.")]
    #[wincode(tag = 0)]
    Height(u64),
    #[document(summary = "From this epoch's first block: on a chain without finality, no block.")]
    #[wincode(tag = 1)]
    Epoch(u64),
}

impl ForkAt {
    #[document(
        parent = "ForkAt",
        summary = "The first block it names.",
        params = { interval = "The chain's blocks per finality epoch, or None on a chain without finality." },
        returns = "The height, or None for an epoch on a chain without finality, or one past the last height.",
    )]
    pub fn height(self, interval: Option<u64>) -> Option<u64> {
        match self {
            ForkAt::Height(height) => Some(height),
            ForkAt::Epoch(epoch) => epoch.checked_mul(interval?),
        }
    }

    fn epoch(self) -> Option<u64> {
        match self {
            ForkAt::Height(_) => None,
            ForkAt::Epoch(epoch) => Some(epoch),
        }
    }
}

#[document(
    summary = "What governance does to the fork schedule: schedules a fork or moves one not yet passed, cancels one not yet passed, or sets how far ahead those must be.",
    category = "Governance",
    see_also = "Change::Fork, ForkSchedule",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
pub enum ForkChange {
    #[document(
        summary = "Schedules a fork, or moves one by that name not yet passed: from at on, blocks run by it. It also tells nodes older than least to stop there, as Change::Upgrade does, so none runs past it by the old rules.",
        params = { name = "The fork's name.", at = "Where it takes effect.", least = "The oldest release that knows it." },
    )]
    #[wincode(tag = 0)]
    Schedule { name: String, at: ForkAt, least: Version },
    #[document(summary = "Cancels a fork not yet passed, by its name.", params = { name = "The fork's name." })]
    #[wincode(tag = 1)]
    Cancel { name: String },
    #[document(summary = "How many blocks ahead of the vote deciding it a fork must be scheduled, moved or cancelled, 1 at least.", params = { blocks = "The lead." })]
    #[wincode(tag = 2)]
    Lead { blocks: u64 },
}

impl ForkChange {
    #[document(
        parent = "ForkChange",
        summary = "Why a chain cannot decide it at this height: a name that cannot be one, a place before the lead or with no block, a fork already passed, which is fixed for good, a cancel of none, or a lead of 0. A fork the release does not know may be scheduled: nodes that do not know it stop before it.",
        params = {
            schedule = "The schedule in force.",
            height = "The height of the block deciding it.",
            interval = "The chain's blocks per finality epoch, if it has finality.",
            target_block_ms = "The chain's target block time, if its consensus has one.",
        },
    )]
    pub fn problem(&self, schedule: &ForkSchedule, height: u64, interval: Option<u64>, target_block_ms: Option<u64>) -> Option<&'static str> {
        let lead = schedule.lead(target_block_ms);
        let passed = |name: &str| schedule.forks.iter().any(|fork| fork.name == name && fork.height <= height.saturating_add(lead));

        match self {
            ForkChange::Schedule { name, at, least } => {
                if let Some(why) = fork_name_problem(name) {
                    return Some(why);
                }

                if *least == Version::default() {
                    return Some("a fork's least version is above 0.0.0");
                }

                let Some(from) = at.height(interval) else {
                    return Some("a fork scheduled by epoch needs a chain with finality");
                };

                if from < height.saturating_add(lead) {
                    return Some("a fork is scheduled at least the chain's lead ahead");
                }

                passed(name).then_some("that fork has passed, or is within the lead: it stays as it is")
            }
            ForkChange::Cancel { name } => match schedule.forks.iter().any(|fork| fork.name == *name) {
                false => Some("no fork by that name is scheduled"),
                true => passed(name).then_some("that fork has passed, or is within the lead: it stays as it is"),
            },
            ForkChange::Lead { blocks: 0 } => Some("a fork's lead is a block at least"),
            ForkChange::Lead { .. } => None,
        }
    }

    #[document(parent = "ForkChange", summary = "What it does, in a line, as the CLI and the apps say it: 'fork x from block 900, for chainv4 0.3.0 or newer', 'cancel fork x' or 'forks scheduled 20000 blocks ahead at least'.")]
    pub fn describe(&self) -> String {
        match self {
            ForkChange::Schedule { name, at: ForkAt::Height(height), least } => format!("fork {name} from block {height}, for chainv4 {least} or newer"),
            ForkChange::Schedule { name, at: ForkAt::Epoch(epoch), least } => format!("fork {name} from epoch {epoch}, for chainv4 {least} or newer"),
            ForkChange::Cancel { name } => format!("cancel fork {name}"),
            ForkChange::Lead { blocks } => format!("forks scheduled {blocks} blocks ahead at least"),
        }
    }

    #[document(
        parent = "ForkChange",
        summary = "The schedule once it is decided: the fork added or moved, kept by height; cancelled; or the lead set.",
        params = { schedule = "The schedule in force.", interval = "The chain's blocks per finality epoch, if it has finality." },
    )]
    pub fn apply(&self, schedule: &ForkSchedule, interval: Option<u64>) -> ForkSchedule {
        let mut next = schedule.clone();

        match self {
            ForkChange::Schedule { name, at, least } => {
                next.forks.retain(|fork| fork.name != *name);
                next.forks.push(ScheduledFork { name: name.clone(), height: at.height(interval).unwrap_or(u64::MAX), epoch: at.epoch(), least: Some(*least) });
                next.forks.sort_by_key(|fork| fork.height);
            }
            ForkChange::Cancel { name } => next.forks.retain(|fork| fork.name != *name),
            ForkChange::Lead { blocks } => next.lead = Some(*blocks),
        }

        next
    }
}

#[document(
    summary = "The forks in force at a block: one bit for each in FORKS, by its place. What every rule a fork changes asks, through VmRules::forks; none at all on a chain that scheduled none, which runs as it always did.",
    category = "Governance",
    see_also = "Fork, ForkSchedule::at",
)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Forks(u64);

impl Forks {
    #[document(parent = "Forks", summary = "No forks: a chain's rules as they were before any.")]
    pub const NONE: Forks = Forks(0);

    #[document(parent = "Forks", summary = "The forks this release knows among those scheduled at or below a height.")]
    pub fn at(scheduled: &[ScheduledFork], height: u64) -> Forks {
        let bits = FORKS
            .iter()
            .take(MAX_FORKS)
            .enumerate()
            .filter(|(_, fork)| scheduled.iter().any(|at| at.name == fork.name && at.height <= height))
            .fold(0, |bits, (place, _)| bits | 1 << place);

        Forks(bits)
    }

    #[document(parent = "Forks", summary = "Whether a fork is in force.")]
    pub fn has(self, fork: &Fork) -> bool {
        FORKS.iter().take(MAX_FORKS).position(|known| known.name == fork.name).is_some_and(|place| self.0 & (1 << place) != 0)
    }

    #[document(parent = "Forks", summary = "The names of those in force, oldest first.")]
    pub fn names(self) -> Vec<&'static str> {
        FORKS.iter().take(MAX_FORKS).enumerate().filter(|(place, _)| self.0 & (1 << place) != 0).map(|(_, fork)| fork.name).collect()
    }
}

#[cfg(test)]
mod tests;
