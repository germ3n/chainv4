use {
    crate::structs::address::Address,
    crate::structs::u256::{U256Le, u256},
    r#macro::document,
    wincode::{SchemaRead, SchemaWrite},
};

#[document(
    summary = "The most hops one run of contracts records. Past it the rest are only counted, so a contract looping over transfers cannot make its receipt huge.",
    category = "Transactions",
    see_also = "Trace",
)]
pub const MAX_HOPS: usize = 256;

#[document(
    summary = "Where a transaction's contracts went: every call, delegate and transfer they made, in the order they started. Kept in the receipt for clients to follow; nothing hashes it, so it is not consensus.",
    category = "Transactions",
    see_also = "Hop, TxReceipt, MAX_HOPS",
    note = "The hops are depth first: each comes right after the call that made it, and before that call's next hop. Each records its depth, which is all it takes to read the list back as a tree.",
)]
#[derive(Clone, Debug, Default, PartialEq, SchemaWrite, SchemaRead)]
pub struct Trace {
    #[document(summary = "The hops, depth first.")]
    pub hops: Vec<Hop>,
    #[document(summary = "Hops made past MAX_HOPS, which were only counted.")]
    pub dropped: u32,
}

#[document(
    summary = "One step a transaction's contracts took: a call into a contract, running another contract's code as its own, or a payment.",
    category = "Transactions",
    see_also = "Trace",
)]
#[derive(Clone, Debug, PartialEq, SchemaWrite, SchemaRead)]
pub struct Hop {
    #[document(summary = "The calls above it: 0 for what the transaction, or the chain, ran itself.")]
    pub depth: u8,
    #[document(summary = "What kind of step it is.")]
    pub kind: HopKind,
    #[document(summary = "Who took it: the transaction's sender at depth 0, or else the contract running.")]
    pub from: Address,
    #[document(summary = "The contract called, the contract whose code ran, or who was paid.")]
    pub to: Address,
    #[document(summary = "The function asked for, by name; a selector in hex when it named none. Empty for a transfer.")]
    pub function: String,
    #[document(summary = "What it sent: a call's value or a transfer's amount.")]
    #[wincode(with = "U256Le")]
    pub value: u256,
    #[document(summary = "The fuel it burned, the hops under it included. 0 for a transfer, whose cost the contract making it paid.")]
    pub fuel_used: u64,
    #[document(summary = "Why it failed, or None when it went through.")]
    pub error: Option<String>,
    #[document(summary = "Where the events it emitted itself are among the receipt's events. Empty when it, or anything above it, failed, since its events were dropped then.")]
    pub events: Vec<u32>,
}

#[document(
    summary = "What kind of step a hop is.",
    category = "Transactions",
    see_also = "Hop",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
pub enum HopKind {
    #[document(summary = "A call into a contract, on its own storage.")]
    #[wincode(tag = 0)]
    Call,
    #[document(summary = "Another contract's code run as the caller's own, on the caller's storage.")]
    #[wincode(tag = 1)]
    Delegate,
    #[document(summary = "Funds a contract sent from its own balance.")]
    #[wincode(tag = 2)]
    Transfer,
    #[document(summary = "A contract created by another, running a template's code, and its init.")]
    #[wincode(tag = 3)]
    Clone,
    #[document(summary = "An EVM contract created, by a transaction or by another contract's CREATE or CREATE2, and its init code run.")]
    #[wincode(tag = 4)]
    Create,
}

impl Hop {
    #[document(
        parent = "Hop",
        summary = "A hop that has only started: nothing burned, failed or emitted yet.",
    )]
    pub fn start(depth: usize, kind: HopKind, from: Address, to: Address, function: &str, value: u256) -> Hop {
        Hop {
            depth: u8::try_from(depth).unwrap_or(u8::MAX),
            kind,
            from,
            to,
            function: function.to_string(),
            value,
            fuel_used: 0,
            error: None,
            events: Vec::new(),
        }
    }
}

impl Trace {
    #[document(
        parent = "Trace",
        summary = "Records a hop that started.",
        returns = "Where it is in the trace, or None past MAX_HOPS, when it is only counted.",
    )]
    pub fn open(&mut self, hop: Hop) -> Option<usize> {
        if self.hops.len() >= MAX_HOPS {
            self.dropped = self.dropped.saturating_add(1);

            return None;
        }

        self.hops.push(hop);

        Some(self.hops.len() - 1)
    }

    #[document(
        parent = "Trace",
        summary = "Records what came of a hop once it is over. Every hop after it in the trace was under it, so when it failed, their events were dropped with it.",
        params = {
            at = "Where open put it; None does nothing.",
            fuel_used = "What it burned.",
            error = "Why it failed, or None.",
        },
    )]
    pub fn close(&mut self, at: Option<usize>, fuel_used: u64, error: Option<String>) {
        let Some(at) = at else {
            return;
        };

        if error.is_some() {
            self.hops[at..].iter_mut().for_each(|hop| hop.events.clear());
        }

        self.hops[at].fuel_used = fuel_used;
        self.hops[at].error = error;
    }

    #[document(
        parent = "Trace",
        summary = "Names the function a hop ran, once a selector it asked for turned out to name one.",
    )]
    pub fn rename(&mut self, at: Option<usize>, function: &str) {
        if let Some(at) = at {
            self.hops[at].function = function.to_string();
        }
    }

    #[document(
        parent = "Trace",
        summary = "Records that a hop emitted the receipt's event at index.",
    )]
    pub fn emitted(&mut self, at: Option<usize>, index: usize) {
        if let Some(at) = at {
            self.hops[at].events.push(u32::try_from(index).unwrap_or(u32::MAX));
        }
    }

    #[document(
        parent = "Trace",
        summary = "Puts another run's hops in after this trace's, as calls under the one that made them: each deeper by deeper, its events placed where they land among the receipt's by events, which drops those it gives None for. Hops past MAX_HOPS are only counted, as the other run's own dropped ones are.",
        params = {
            sub = "The other run's trace.",
            deeper = "How much deeper each of its hops goes.",
            events = "Where one of its event indices lands among the receipt's events, or None when it was dropped.",
        },
    )]
    pub fn graft(&mut self, sub: Trace, deeper: usize, events: impl Fn(u32) -> Option<u32>) {
        for mut hop in sub.hops {
            hop.depth = u8::try_from(usize::from(hop.depth).saturating_add(deeper)).unwrap_or(u8::MAX);
            hop.events = hop.events.into_iter().filter_map(&events).collect();
            self.open(hop);
        }

        self.dropped = self.dropped.saturating_add(sub.dropped);
    }

    #[document(
        parent = "Trace",
        summary = "Forgets which events every hop emitted, for a run whose events were all dropped.",
    )]
    pub fn forget_events(&mut self) {
        self.hops.iter_mut().for_each(|hop| hop.events.clear());
    }

    #[document(
        parent = "Trace",
        summary = "Adds the hops of a run that came after this one, such as an upgrade's migrate after its check_perms, keeping to MAX_HOPS.",
    )]
    pub fn append(&mut self, later: Trace) {
        let room = MAX_HOPS.saturating_sub(self.hops.len());
        let over = later.hops.len().saturating_sub(room);
        self.dropped = self
            .dropped
            .saturating_add(later.dropped)
            .saturating_add(u32::try_from(over).unwrap_or(u32::MAX));
        self.hops.extend(later.hops.into_iter().take(room));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hop(depth: usize) -> Hop {
        Hop::start(depth, HopKind::Call, Address::Contract([1; 32]), Address::Contract([2; 32]), "f", u256::ZERO)
    }

    #[test]
    fn a_failed_hop_drops_the_events_of_everything_under_it() {
        let mut trace = Trace::default();
        let outer = trace.open(hop(0));
        trace.emitted(outer, 0);
        let inner = trace.open(hop(1));
        trace.emitted(inner, 1);
        trace.close(inner, 10, None);

        assert_eq!(trace.hops[1].events, [1]);

        trace.close(outer, 30, Some("boom".to_string()));

        assert!(trace.hops.iter().all(|hop| hop.events.is_empty()));
        assert_eq!(trace.hops[0].fuel_used, 30);
        assert_eq!(trace.hops[0].error.as_deref(), Some("boom"));
        assert_eq!(trace.hops[1].error, None);
    }

    #[test]
    fn hops_past_the_cap_are_only_counted() {
        let mut trace = Trace::default();

        for _ in 0..MAX_HOPS + 3 {
            trace.open(hop(1));
        }

        assert_eq!(trace.hops.len(), MAX_HOPS);
        assert_eq!(trace.dropped, 3);

        let mut first = Trace::default();
        first.open(hop(0));
        first.append(trace);

        assert_eq!(first.hops.len(), MAX_HOPS);
        assert_eq!(first.dropped, 4);
    }

    #[test]
    fn traces_round_trip() {
        let mut trace = Trace::default();
        let at = trace.open(hop(0));
        trace.emitted(at, 3);
        trace.close(at, 7, Some("no".to_string()));
        let decoded: Trace = wincode::deserialize(&wincode::serialize(&trace).unwrap()).unwrap();

        assert_eq!(decoded, trace);
    }
}
