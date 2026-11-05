//! The anchorer (docs/anchoring.md, The anchorer): `bridge anchor`, or `bridge btc anchor` as it was
//! first named. Each pass writes the chain's newest finalized checkpoint into each chain anchoring
//! takes, Bitcoin and Ethereum, once that chain's newest anchor is anchoring.every epochs old, and
//! shows each written anchor to the chain once its chain holds it: Bitcoin's once the relay holds
//! its block deep enough, Ethereum's once a finality update finalizes its block.
//!
//! The two chains take turns: one list holds every anchor, and each anchor carries the list's root,
//! so an anchor recorded in one chain while another is written in the other leaves that one's root
//! behind and it is wasted. So neither is written while the other has one pending, Bitcoin first
//! when both are due, and each chain's cadence counts from its own newest anchor.

use {
    super::Bridge,
    crate::anchor::Payload,
    crate::cli::CliResult,
    crate::params::anchoring::AnchorChain,
    crate::structs::hex::from_hex,
    serde_json::Value,
    std::path::PathBuf,
};

/// What the anchorer takes.
#[derive(clap::Args, Clone, Debug)]
pub struct AnchorArgs {
    /// The secp256k1 key whose native segwit coins pay for Bitcoin anchors: --evm-key's unless
    /// given. Ethereum anchors are paid by --evm-key.
    #[arg(long)]
    pub btc_key: Option<PathBuf>,
    /// Seconds between passes; 0 for one pass.
    #[arg(long, default_value_t = 60)]
    pub every: u64,
    /// The rate a Bitcoin anchor pays, in satoshis a virtual byte: what Bitcoin's source asks for
    /// within a few blocks unless given.
    #[arg(long)]
    pub fee_rate: Option<u64>,
}

/// Anchors written and not yet shown to the chain, each chain's apart. They live in the
/// anchorer's memory: one stopped before showing its anchor writes another when it starts again.
#[derive(Default)]
pub(super) struct Pending {
    #[cfg(feature = "anchoring-btc")]
    btc: Option<super::btc::anchor::Pending>,
    #[cfg(feature = "anchoring-eth")]
    eth: Option<super::eth::EthPending>,
}

impl Pending {
    /// Whether a chain other than this one has an anchor pending: this one then waits its turn.
    fn other_than(&self, chain: AnchorChain) -> bool {
        let pending: [(AnchorChain, bool); 2] = [
            #[cfg(feature = "anchoring-btc")]
            (AnchorChain::Bitcoin, self.btc.is_some()),
            #[cfg(not(feature = "anchoring-btc"))]
            (AnchorChain::Bitcoin, false),
            #[cfg(feature = "anchoring-eth")]
            (AnchorChain::Ethereum, self.eth.is_some()),
            #[cfg(not(feature = "anchoring-eth"))]
            (AnchorChain::Ethereum, false),
        ];

        pending.iter().any(|(other, held)| *other != chain && *held)
    }
}

/// The checkpoint due to be written into a chain, if one is: the node's newest finalized one, past
/// every anchor's epoch, and anchoring.every epochs past the chain's own newest anchor.
pub(super) fn due(anchors: &Value, finalized: Option<(u64, &str)>, chain: AnchorChain) -> CliResult<Option<Payload>> {
    let Some((epoch, block)) = finalized else {
        return Ok(None);
    };
    let entries = anchors["entries"].as_array().cloned().unwrap_or_default();
    let every = anchors["settings"]["every"].as_u64().unwrap_or(1).max(1);
    let newest = entries.iter().filter_map(|entry| entry["epoch"].as_u64()).max();
    let own = entries.iter().filter(|entry| entry["chain"].as_str() == Some(chain.name())).filter_map(|entry| entry["epoch"].as_u64()).max();

    if epoch == 0 || newest.is_some_and(|newest| epoch <= newest) || own.is_some_and(|own| epoch < own.saturating_add(every)) {
        return Ok(None);
    }

    Ok(Some(Payload {
        chain: from_hex(anchors["chain_tag"].as_str().unwrap_or_default()).and_then(|tag| tag.try_into().ok()).ok_or("GET /anchors gave no chain tag")?,
        epoch,
        block: from_hex(block).and_then(|bytes| bytes.try_into().ok()).ok_or("the node's finalized block is not 32 bytes")?,
        root: from_hex(anchors["root"].as_str().unwrap_or_default()).and_then(|bytes| bytes.try_into().ok()).ok_or("GET /anchors gave a root that is not 32 bytes")?,
    }))
}

/// What became of a pending Ethereum anchor this pass.
#[cfg_attr(not(feature = "anchoring-eth"), allow(dead_code))]
pub(super) enum Step {
    /// Still waiting for its chain.
    Wait,
    /// Shown, recorded, or past use: nothing is pending of this chain any more.
    Done,
}

/// Whether a pending anchor of an epoch, written at a root, still stands: not recorded, and the
/// list's root unmoved.
#[cfg_attr(not(any(feature = "anchoring-btc", feature = "anchoring-eth")), allow(dead_code))]
pub(super) fn still_stands(anchors: &Value, epoch: u64, root: &str, what: &str) -> bool {
    if anchors["entries"].as_array().into_iter().flatten().any(|entry| entry["epoch"].as_u64() == Some(epoch)) {
        println!("epoch {epoch}'s anchor is recorded");

        return false;
    }

    if anchors["root"].as_str() != Some(root) {
        println!("another anchor was recorded since {what} was written: it is past use");

        return false;
    }

    true
}

impl Bridge {
    /// The anchorer: a pass every --every seconds, or one.
    pub(super) fn run_anchorer(&self, args: &AnchorArgs) -> CliResult<()> {
        #[cfg(feature = "anchoring-btc")]
        let btc_secret = match &args.btc_key {
            Some(path) => match crate::cli::signer(Some(path))?.private() {
                crate::crypto::keypair::PrivateKey::Secp256k1(secret) => Some(*secret),
                _ => return Err("--btc-key is a secp256k1 key: make one with keygen --scheme secp256k1".into()),
            },
            None => self.evm_secret().ok(),
        };
        let mut pending = Pending::default();

        loop {
            #[cfg(feature = "anchoring-btc")]
            let pass = self.anchor_pass(btc_secret.as_ref(), &mut pending, args.fee_rate);
            #[cfg(not(feature = "anchoring-btc"))]
            let pass = self.anchor_pass(None, &mut pending, args.fee_rate);

            if let Err(why) = pass {
                eprintln!("the anchor pass: {why}");
            }

            if args.every == 0 {
                return Ok(());
            }

            std::thread::sleep(std::time::Duration::from_secs(args.every));
        }
    }

    /// One pass: each chain's pending anchor carried on, then a new one written into a chain whose
    /// turn it is and that is due.
    #[cfg_attr(not(feature = "anchoring-btc"), allow(unused_variables))]
    pub(super) fn anchor_pass(&self, btc_secret: Option<&[u8; 32]>, pending: &mut Pending, rate: Option<u64>) -> CliResult<()> {
        let anchors = self.anchors()?;
        let settings = &anchors["settings"];
        let on = |name: &str| settings[name].as_bool() == Some(true);

        if !on("bitcoin") && !on("ethereum") {
            println!("anchoring is off on this chain: nothing to do");

            return Ok(());
        }

        let status = self.status()?;
        let finality = &status["finality"]["finalized"];
        let finalized = finality["epoch"].as_u64().zip(finality["block"].as_str());

        #[cfg(feature = "anchoring-btc")]
        if on("bitcoin") {
            if let Some(written) = pending.btc.take() {
                if still_stands(&anchors, written.epoch, &written.root, &written.txid) && !self.show_btc_anchor(&written, &anchors)? {
                    pending.btc = Some(written);
                }
            } else if !pending.other_than(AnchorChain::Bitcoin)
                && let Some(payload) = due(&anchors, finalized, AnchorChain::Bitcoin)?
            {
                // One that cannot be written leaves Ethereum its turn.
                let written = btc_secret.ok_or_else(|| "give --btc-key or --evm-key: the key whose coins pay for Bitcoin anchors".into()).and_then(|secret| self.write_btc_anchor(secret, &payload, &anchors, rate));

                match written {
                    Ok(txid) => {
                        println!("wrote epoch {}'s checkpoint into Bitcoin: {txid}", payload.epoch);
                        pending.btc = Some(super::btc::anchor::Pending { txid, epoch: payload.epoch, root: anchors["root"].as_str().unwrap_or_default().to_string() });
                    }
                    Err(why) => eprintln!("writing the anchor into Bitcoin: {why}"),
                }
            }
        }

        #[cfg(feature = "anchoring-eth")]
        if on("ethereum") {
            if let Some(written) = pending.eth.take() {
                if still_stands(&anchors, written.epoch, &written.root, &written.tx_hash) && matches!(self.show_eth_anchor(&written, &anchors)?, Step::Wait) {
                    pending.eth = Some(written);
                }
            } else if !pending.other_than(AnchorChain::Ethereum)
                && let Some(payload) = due(&anchors, finalized, AnchorChain::Ethereum)?
            {
                // Bitcoin went first above where both were due: its pending anchor holds this back.
                pending.eth = Some(self.write_eth_anchor(&payload, &anchors)?);
            }
        }

        let _ = (finalized, rate, &pending);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use {super::*, serde_json::json};

    fn anchors(entries: Value) -> Value {
        json!({ "settings": { "every": 10 }, "chain_tag": "0x01020304", "root": format!("0x{}", "00".repeat(32)), "entries": entries })
    }

    #[test]
    fn each_chain_is_due_by_its_own_newest_anchor_and_never_at_an_epoch_anchored() {
        let block = format!("0x{}", "ab".repeat(32));
        let at = |epoch: u64| Some((epoch, block.as_str()));
        let none = anchors(json!([]));

        assert_eq!(due(&none, at(5), AnchorChain::Bitcoin).unwrap().map(|payload| (payload.epoch, payload.chain)), Some((5, [1, 2, 3, 4])));
        assert!(due(&none, at(0), AnchorChain::Bitcoin).unwrap().is_none(), "no checkpoint to anchor yet");
        assert!(due(&none, None, AnchorChain::Ethereum).unwrap().is_none());

        let some = anchors(json!([{ "epoch": 20, "chain": "bitcoin" }, { "epoch": 24, "chain": "ethereum" }]));
        assert!(due(&some, at(29), AnchorChain::Bitcoin).unwrap().is_none(), "Bitcoin's newest is 9 epochs old");
        assert_eq!(due(&some, at(30), AnchorChain::Bitcoin).unwrap().map(|payload| payload.epoch), Some(30));
        assert!(due(&some, at(30), AnchorChain::Ethereum).unwrap().is_none(), "Ethereum's newest is 6 epochs old");
        assert!(due(&some, at(24), AnchorChain::Bitcoin).unwrap().is_none(), "an epoch anchored is anchored once");
        assert!(still_stands(&some, 30, &format!("0x{}", "00".repeat(32)), "it"));
        assert!(!still_stands(&some, 24, &format!("0x{}", "00".repeat(32)), "it"), "recorded");
        assert!(!still_stands(&some, 30, "0x11", "it"), "the root moved");
    }

    #[cfg(all(feature = "anchoring-btc", feature = "anchoring-eth"))]
    #[test]
    fn a_chain_waits_while_the_other_has_an_anchor_pending() {
        let mut pending = Pending::default();
        assert!(!pending.other_than(AnchorChain::Bitcoin) && !pending.other_than(AnchorChain::Ethereum));

        pending.btc = Some(crate::cli::bridge::btc::anchor::Pending { txid: "t".into(), epoch: 1, root: "r".into() });
        assert!(pending.other_than(AnchorChain::Ethereum));
        assert!(!pending.other_than(AnchorChain::Bitcoin));
    }
}
