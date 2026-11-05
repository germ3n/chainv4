//! The light client's rules, from the consensus specs' sync protocol (`initialize_light_client_store`,
//! `validate_light_client_update`, `apply_light_client_update`), cut down to what follows finality:
//!
//! - **No optimistic header,** and no best-update tracking or forced updates: an update counts only
//!   once two thirds of the committee signed it, and only for the finalized header it proves.
//! - **Every update must finalize something,** a header after genesis's.
//! - **The next committee may come with it or not,** as the specs allow: an update whose branch is
//!   all zeros brings none.

use {
    crate::{
        Crypto, Error, Root,
        forks::{DOMAIN_SYNC_COMMITTEE, Network, SLOTS_PER_HISTORICAL_ROOT, compute_signing_root},
        ssz,
        types::{Finalized, LightClientBootstrap, LightClientHeader, LightClientUpdate, SYNC_COMMITTEE_SIZE, SyncCommittee},
    },
    alloc::vec::Vec,
};

/// What the light client knows: the newest finalized header, and the sync committees of its period
/// and, once an update named it, the next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Store {
    /// The network followed, whose slot timing and forks the rules go by.
    pub network: Network,
    pub finalized: Finalized,
    pub current: SyncCommittee,
    pub next: Option<SyncCommittee>,
}

/// What an update changed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Applied {
    /// A newer finalized header is held.
    pub finalized: bool,
    /// The store moved into the next period: the next committee is now the current one, and the
    /// update's next committee, if it brought one, the next.
    pub rotated: bool,
    /// The next committee was unknown, and now is.
    pub learned_next: bool,
}

impl Store {
    /// initialize_light_client_store: a store from a bootstrap whose header is a block root the
    /// caller trusts, checked to hold its execution header and its committee.
    pub fn bootstrap<C: Crypto + ?Sized>(crypto: &C, network: &Network, trusted_root: &Root, bootstrap: &LightClientBootstrap) -> Result<Store, Error> {
        bootstrap.header.check(crypto, network)?;

        if bootstrap.header.beacon.hash_tree_root(crypto) != *trusted_root {
            return Err(Error::NotTrusted);
        }

        let fork = network.fork_at_slot(bootstrap.header.beacon.slot).fork;
        let held = ssz::is_valid_normalized_merkle_branch(
            crypto,
            &bootstrap.current_sync_committee.hash_tree_root(crypto),
            &bootstrap.current_sync_committee_branch,
            fork.current_sync_committee_gindex(),
            &bootstrap.header.beacon.state_root,
        );

        if !held {
            return Err(Error::BadCommitteeBranch);
        }

        Ok(Store { network: network.clone(), finalized: Finalized::from_header(crypto, &bootstrap.header), current: bootstrap.current_sync_committee.clone(), next: None })
    }

    /// The sync committee period of the finalized header.
    pub fn period(&self) -> u64 {
        self.network.period_at_slot(self.finalized.beacon.slot)
    }

    /// Checks an update and applies it, or refuses it and changes nothing. current_slot is the
    /// network's slot now, which no signature may be after.
    pub fn process_update<C: Crypto + ?Sized>(&mut self, crypto: &C, update: &LightClientUpdate, current_slot: u64) -> Result<Applied, Error> {
        self.validate_update(crypto, update, current_slot)?;

        Ok(self.apply_update(crypto, update))
    }

    /// validate_light_client_update, for a finalized update that two thirds of the committee signed.
    pub fn validate_update<C: Crypto + ?Sized>(&self, crypto: &C, update: &LightClientUpdate, current_slot: u64) -> Result<(), Error> {
        check_shape(update, current_slot)?;
        let (attested_slot, finalized_slot) = (update.attested_header.beacon.slot, update.finalized_header.beacon.slot);
        let store_period = self.period();
        let signature_period = self.network.period_at_slot(update.signature_slot);
        let known = signature_period == store_period || (self.next.is_some() && signature_period == store_period + 1);

        if !known {
            return Err(Error::UnknownPeriod);
        }

        // Relevant: a newer finalized header, or the next committee while it is unknown, from the
        // store's own period, with the header that finalizes it in that period too.
        let attested_period = self.network.period_at_slot(attested_slot);
        let brings_next = self.next.is_none() && update.next_sync_committee.is_some() && attested_period == store_period && self.network.period_at_slot(finalized_slot) == attested_period;

        if finalized_slot <= self.finalized.beacon.slot && !brings_next {
            return Err(Error::Stale);
        }

        // What the slots above already bound, said outright: while the next committee is unknown,
        // nothing past the store's own period is finalized.
        if self.next.is_none() && self.network.period_at_slot(finalized_slot) != store_period {
            return Err(Error::SkipsPeriod);
        }

        check_finality(crypto, &self.network, update)?;
        let fork = self.network.fork_at_slot(attested_slot).fork;

        if let Some(next) = &update.next_sync_committee {
            if attested_period == store_period && self.next.as_ref().is_some_and(|known| !known.same_as(next)) {
                return Err(Error::NextCommitteeMismatch);
            }

            let next_held = ssz::is_valid_normalized_merkle_branch(crypto, &next.hash_tree_root(crypto), &update.next_sync_committee_branch, fork.next_sync_committee_gindex(), &update.attested_header.beacon.state_root);

            if !next_held {
                return Err(Error::BadNextCommitteeBranch);
            }
        }

        let committee = match signature_period == store_period {
            true => &self.current,
            false => self.next.as_ref().expect("a known period"),
        };

        check_signature(crypto, &self.network, committee, update)
    }

    /// apply_light_client_update, for an update validate_update took.
    fn apply_update<C: Crypto + ?Sized>(&mut self, crypto: &C, update: &LightClientUpdate) -> Applied {
        let store_period = self.period();
        let finalized_period = self.network.period_at_slot(update.finalized_header.beacon.slot);
        let mut applied = Applied::default();

        if self.next.is_none() {
            // validate_update saw to the finalized header being in the store's period.
            debug_assert_eq!(finalized_period, store_period);
            self.next = update.next_sync_committee.clone();
            applied.learned_next = self.next.is_some();
        } else if finalized_period == store_period + 1 {
            self.current = self.next.take().expect("a next committee");
            self.next = update.next_sync_committee.clone();
            applied.rotated = true;
        }

        if update.finalized_header.beacon.slot > self.finalized.beacon.slot {
            self.finalized = Finalized::from_header(crypto, &update.finalized_header);
            applied.finalized = true;
        }

        applied
    }
}

/// Whether a header is an ancestor of one already finalized, by the held header's state: its
/// block_roots keep the roots of the 8192 slots before it. What it gives back is kept as a
/// finalized header too, as an ancestor of one is final as well.
pub fn prove_ancestor<C: Crypto + ?Sized>(crypto: &C, network: &Network, held: &Finalized, ancestor: &LightClientHeader, branch: &[Root]) -> Result<Finalized, Error> {
    let slot = ancestor.beacon.slot;

    if slot >= held.beacon.slot || held.beacon.slot - slot > SLOTS_PER_HISTORICAL_ROOT {
        return Err(Error::NotAncestor);
    }

    ancestor.check(crypto, network)?;
    let gindex = ssz::concat_gindices(network.fork_at_slot(held.beacon.slot).fork.block_roots_gindex(), SLOTS_PER_HISTORICAL_ROOT + slot % SLOTS_PER_HISTORICAL_ROOT);
    let found = ssz::is_valid_normalized_merkle_branch(crypto, &ancestor.beacon.hash_tree_root(crypto), branch, gindex, &held.beacon.state_root);

    found.then(|| Finalized::from_header(crypto, ancestor)).ok_or(Error::BadAncestorBranch)
}

/// What an update must be whatever a store holds: two thirds of the committee signed, its slots in
/// order, and signed no later than the current slot.
fn check_shape(update: &LightClientUpdate, current_slot: u64) -> Result<(), Error> {
    // Two thirds of the committee, and so at least the specs' one participant.
    if update.sync_aggregate.participants() * 3 < SYNC_COMMITTEE_SIZE * 2 {
        return Err(Error::LowParticipation);
    }

    let (attested_slot, finalized_slot) = (update.attested_header.beacon.slot, update.finalized_header.beacon.slot);

    if !(update.signature_slot > attested_slot && attested_slot >= finalized_slot) {
        return Err(Error::BadSlots);
    }

    if update.signature_slot > current_slot {
        return Err(Error::FromTheFuture);
    }

    Ok(())
}

/// Its headers valid, and a finalized header after genesis's in the attested header's state.
fn check_finality<C: Crypto + ?Sized>(crypto: &C, network: &Network, update: &LightClientUpdate) -> Result<(), Error> {
    update.attested_header.check(crypto, network)?;

    if update.finalized_header.beacon.slot == 0 {
        return Err(Error::NotFinality);
    }

    update.finalized_header.check(crypto, network)?;
    let fork = network.fork_at_slot(update.attested_header.beacon.slot).fork;
    let finality_held = ssz::is_valid_normalized_merkle_branch(
        crypto,
        &update.finalized_header.beacon.hash_tree_root(crypto),
        &update.finality_branch,
        fork.finalized_root_gindex(),
        &update.attested_header.beacon.state_root,
    );

    finality_held.then_some(()).ok_or(Error::BadFinalityBranch)
}

/// The sync aggregate's signature, by the committee given, over the attested header under the
/// network's domain for the slot before the signature's, as the specs have it.
fn check_signature<C: Crypto + ?Sized>(crypto: &C, network: &Network, committee: &SyncCommittee, update: &LightClientUpdate) -> Result<(), Error> {
    let aggregate = &update.sync_aggregate;
    let signers: Vec<_> = committee.pubkeys.iter().enumerate().filter(|(index, _)| aggregate.signed(*index)).map(|(_, key)| *key).collect();
    let fork_version = network.fork_at_slot(update.signature_slot.max(1) - 1).version;
    let domain = network.domain(crypto, DOMAIN_SYNC_COMMITTEE, fork_version);
    let signing_root = compute_signing_root(crypto, &update.attested_header.beacon.hash_tree_root(crypto), &domain);

    match crypto.fast_aggregate_verify(&signers, &signing_root, &aggregate.signature) {
        true => Ok(()),
        false => Err(Error::BadSignature),
    }
}

/// Whether an update finalizes its header, on its own: two thirds of a committee signed it, its
/// slots in order and not after the current slot, its headers valid, and its finalized header in
/// the attested header's state. The committee is the caller's to pick: the one it knows for the
/// period of the update's signature slot. What validate_update checks of an update, without a
/// store's periods or the next committee: what a node checks an update kept beside an anchor by,
/// with the committees it learned by following updates from a trusted root.
pub fn verify_finality<C: Crypto + ?Sized>(crypto: &C, network: &Network, committee: &SyncCommittee, update: &LightClientUpdate, current_slot: u64) -> Result<(), Error> {
    check_shape(update, current_slot)?;
    check_finality(crypto, network, update)?;

    check_signature(crypto, network, committee, update)
}
