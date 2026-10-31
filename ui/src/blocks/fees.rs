//! What checked blocks say a transaction should offer, with no node to ask: what its kind was seen to
//! use lately, and what anyone was seen to pay. Both apps go by it when their reading is on.

use {
    chainv4::light::{CheckedItem, RangeBody},
    chainv4::structs::tx::{FeeOffer, Tx},
    chainv4::structs::u256::u256,
    serde::{Deserialize, Serialize},
    std::collections::BTreeMap,
};

/// The most kinds of transaction kept, and how many blocks old a figure may be before a lower one
/// replaces it: what a call used a month ago says little of today.
const MOST_HINTS: usize = 256;
pub const HINT_BLOCKS: u64 = 17_280;
/// What to offer where no checked block has shown what a kind of transaction uses: round figures, on
/// the high side, as what it does not use is refunded.
const GUESS_TRANSFER_GAS: u64 = 200_000;
pub const GUESS_CALL_GAS: u64 = 2_000_000;

/// What a kind of transaction was last seen to use.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Hint {
    gas: u64,
    height: u64,
}

/// What the kinds of transaction are, for suggesting fees: what is sent by one kind says little of
/// another, and a call of a contract's function only of that function.
pub fn kind_of(tx: &Tx) -> Option<String> {
    match tx {
        Tx::Transfer { .. } => Some("transfer".to_string()),
        Tx::ContractCall { to, func, .. } => Some(format!("{to}/{func}")),
        _ => None,
    }
}

/// What a block's transactions used, by kind, and the most any paid per gas: nothing of a block whose
/// receipts did not come.
pub fn used_in(item: &CheckedItem) -> (Vec<(String, u64)>, Option<u256>) {
    let (mut used, mut price) = (Vec::new(), None);

    if let RangeBody::Whole { txs, receipts: Some(receipts), .. } = &item.body {
        for (signed, receipt) in txs.iter().zip(receipts) {
            if receipt.gas_used == 0 || receipt.outcome.failed() {
                continue;
            }

            if let Some(kind) = kind_of(&signed.tx) {
                used.push((kind, receipt.gas_used));
            }

            let paid = receipt.fee / u256::from(receipt.gas_used);

            if price.is_none_or(|price| paid > price) {
                price = Some(paid);
            }
        }
    }

    (used, price)
}

/// What was seen of fees in the blocks read.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FeesSeen {
    hints: BTreeMap<String, Hint>,
    /// The highest price per gas seen paid lately, in decimal, and at what height.
    price: Option<(String, u64)>,
}

impl FeesSeen {
    /// What a transaction of this kind should offer for gas, as the blocks read show.
    pub fn hinted(&self, kind: &str) -> Option<u64> {
        self.hints.get(kind).map(|hint| hint.gas)
    }

    /// Takes in what a block used: the most in a while, or the latest once that is old.
    pub fn take(&mut self, height: u64, seen: &[(String, u64)], price: Option<u256>) {
        for (kind, gas) in seen {
            let hint = self.hints.entry(kind.clone()).or_default();

            if *gas >= hint.gas || height.saturating_sub(hint.height) > HINT_BLOCKS {
                *hint = Hint { gas: *gas, height };
            }
        }

        if self.hints.len() > MOST_HINTS {
            let oldest = self.hints.iter().min_by_key(|(_, hint)| hint.height).map(|(kind, _)| kind.clone());

            if let Some(oldest) = oldest {
                self.hints.remove(&oldest);
            }
        }

        if let Some(price) = price {
            let old = self.price.as_ref().filter(|(_, at)| height.saturating_sub(*at) <= HINT_BLOCKS).and_then(|(seen, _)| u256::from_str_radix(seen, 10).ok());

            if old.is_none_or(|old| price >= old) {
                self.price = Some((price.to_string(), height));
            }
        }
    }

    /// The most anyone paid per gas lately, as checked blocks show.
    pub fn price_seen(&self) -> Option<u256> {
        self.price.as_ref().and_then(|(price, _)| u256::from_str_radix(price, 10).ok())
    }

    /// What a transaction of this kind used lately, as checked blocks show, if any showed it.
    pub fn gas_seen(&self, tx: &Tx) -> Option<u64> {
        self.hinted(&kind_of(tx)?)
    }

    /// What to offer for this transaction, with nothing to ask a node: the most its kind was seen to
    /// use lately and a quarter more, or a round guess; and twice the base price the newest checked
    /// header says, plus the tip, or what was seen paid if that is more. Whether a block showed the
    /// gas, rather than a guess. On a chain with no fees it comes to nothing.
    pub fn suggest(&self, tx: &Tx, base: u256, tip: u256) -> (FeeOffer, bool) {
        let seen = self.price_seen().unwrap_or(u256::ZERO);

        if base == u256::ZERO && seen == u256::ZERO && tip == u256::ZERO {
            return (FeeOffer::NONE, true);
        }

        let gas = self.gas_seen(tx);
        let guess = match tx {
            Tx::Transfer { .. } => GUESS_TRANSFER_GAS,
            _ => GUESS_CALL_GAS,
        };
        let gas_limit = gas.map_or(guess, |gas| gas.saturating_add(gas.div_ceil(4)));
        let double = base.saturating_add(base);

        (FeeOffer { gas_limit, max_price: double.max(seen).saturating_add(tip), tip }, gas.is_some())
    }
}
