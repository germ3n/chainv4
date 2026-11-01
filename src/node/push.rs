//! A pool's node claiming for its miners: every minute, for each miner whose unclaimed earnings
//! pass the operator's threshold, a Tx::Claim from the operator's key, which pays the fee. The
//! coins go to the miner, as any claim's do; the operator spends only the gas.

use {
    super::Node,
    crate::crypto::keypair::Keypair,
    crate::registry::Accounts,
    crate::registry::pools::Pools,
    crate::structs::tx::{FeeOffer, SignedTx, Tx},
    crate::structs::u256::u256,
    r#macro::document,
};

/// How often the node looks for claims to push, in milliseconds.
const PUSH_EVERY: u64 = 60_000;
/// The most claims one look pushes, so a large pool's spread over several blocks.
const MOST_PUSHED: usize = 64;

#[document(
    summary = "A pool's node claiming for its miners: --pool-push, the least it claims for, and --pool-push-key, the key that sends the claims and pays their fees.",
    category = "Node",
    see_also = "Node::pushing, Tx::Claim",
)]
#[derive(Clone)]
pub struct PoolPush {
    #[document(summary = "The key that sends the claims and pays their fees.")]
    pub key: Keypair,
    #[document(summary = "The least unclaimed a miner has before the node claims for it, in the coin's smallest unit.")]
    pub least: u256,
}

impl Node {
    #[document(
        parent = "Node",
        summary = "Claims for the miners of the pool this node runs, as PoolPush says: every minute, for each miner owed at least its least, up to 64 at a time.",
    )]
    pub fn pushing(mut self, push: PoolPush) -> Node {
        self.push = Some(push);

        self
    }

    /// Pushes the claims due, once a minute.
    pub(super) fn push_if_due(&mut self, now: u64) {
        if self.push.is_none() || now < self.pushed.saturating_add(PUSH_EVERY) {
            return;
        }

        self.pushed = now;

        if let Err(why) = self.push_claims() {
            log::warn!("[pool] pushing claims: {why}");
        }
    }

    fn push_claims(&mut self) -> Result<(), String> {
        let Some(push) = self.push.clone() else {
            return Ok(());
        };
        let Some(pool) = self.producer.as_ref().and_then(|producer| producer.mining()).and_then(|mining| mining.pool) else {
            return Ok(());
        };
        let producer = pool.producer.ok_or("the pool does not say its address")?;
        let (hash, state) = (self.chain.hash(), self.chain.state());
        let pools = Pools::new(&state, hash);

        // The newest tree whose root a block on this chain carries, at that block's height.
        let mut found = None;

        for tree in &pool.claims {
            for (height, _) in tree.committed.iter().rev() {
                if pools.root(*height).map_err(|err| err.to_string())?.is_some_and(|root| root.root == tree.root && root.pool == producer) {
                    found = Some((tree.clone(), *height));
                    break;
                }
            }

            if found.is_some() {
                break;
            }
        }

        let Some((tree, height)) = found else {
            return Ok(());
        };
        let from = push.key.public().address();
        let account = Accounts::new(&state).get(&from).map_err(|err| err.to_string())?;
        // After the claims of this key's still waiting in the mempool, if any.
        let waiting = self.gate.mempool().iter().filter(|tx| tx.sender() == from).map(|tx| tx.nonce + 1).max();
        let mut nonce = account.nonce.max(waiting.unwrap_or(0));
        let base = self.chain.next_base_price();
        let genesis = self.chain.genesis();
        let mut pushed = 0;

        for (index, (to, earned)) in tree.leaves.iter().enumerate() {
            if pushed == MOST_PUSHED {
                break;
            }

            let claimed = pools.claimed(&producer, to).map_err(|err| err.to_string())?;

            if earned.saturating_sub(claimed) < push.least.max(u256::ONE) {
                continue;
            }

            let path = tree.proof(hash, to).map(|(_, _, path)| path).ok_or("a leaf the tree does not hold")?;
            let tx = Tx::Claim { height, to: *to, earned: *earned, index: index as u64, path };
            let probe = SignedTx::sign(tx.clone(), nonce, FeeOffer::NONE, &push.key, &genesis);
            // A claim runs no contract, so it needs its intrinsic gas alone: no fuel cap to add.
            let offer = self.chain.fees().map_or(FeeOffer::NONE, |fees| fees.offer(base, 0, &probe, Default::default()));

            match self.submit(SignedTx::sign(tx, nonce, offer, &push.key, &genesis)) {
                Ok(()) => {
                    log::info!("[pool] claimed {} for {to}", earned.saturating_sub(claimed));
                    nonce += 1;
                    pushed += 1;
                }
                Err(err) => {
                    log::warn!("[pool] the node would not take a claim for {to}: {err}");

                    break;
                }
            }
        }

        Ok(())
    }
}
