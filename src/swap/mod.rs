//! Atomic swaps between any two chains: coins or a token on one for coins or a token on another,
//! with no one in between and no way for either side to take both. Each side locks what it pays
//! under the same SHA-256 hash: on chainv4 in the swap contract (contracts/lua/swap.lua), on
//! Bitcoin in an HTLC ([htlc]), on an EVM chain such as Ethereum in Swap.sol ([evm]). Whoever
//! starts knows the secret behind the hash, locks first and with the longer deadline; the other
//! locks second, with a shorter one. The starter claims the other's lock with the secret, which
//! shows it on that chain, and the other claims the starter's lock with it in turn. Whoever does
//! not claim in time gets their own lock back after its deadline.
//!
//! The terms are two legs, the starter's and the other's, each a chain, what is locked there,
//! who locks it and who claims it. They go between the two as codes, ur:chainv4-swap: the
//! starter's offer, the legs, the hash, the deadlines and their own parts, and the answer, the
//! same with the other's added.

pub mod evm;
pub mod htlc;
mod legacy;

use {
    crate::crypto::bitcoin::sha256,
    crate::crypto::bitcoin::tx::Network,
    crate::structs::address::Address,
    crate::structs::hex::to_hex,
    crate::structs::u256::u256,
    r#macro::document,
    wincode::{SchemaRead, SchemaWrite},
};

#[document(summary = "The UR type the terms travel as.", category = "Swaps")]
pub const UR_TYPE: &str = "chainv4-swap";

#[document(
    summary = "The name, under the chain's suffix, that the swap contract a new offer uses by default is found by, such as swap.dev: registered by whoever runs one, and looked up before the node's advertisement. Any contract that follows contracts/lua/swap.lua's functions will do; the terms name the one a swap uses.",
    category = "Swaps",
)]
pub const SWAP_NAME: &str = "swap";

#[document(
    summary = "How long before the first lock's deadline the second's must end, at least: time for the starter, once the second lock stands, to claim it and so show the secret, and for the other to claim the first with it before its refund opens.",
    category = "Swaps",
)]
pub const MIN_GAP_MS: u64 = 4 * 3_600_000;

#[document(
    summary = "The deadlines a swap gets unless its starter says otherwise: the first lock's, then the second's, from when it starts.",
    category = "Swaps",
)]
pub const FIRST_DEADLINE_MS: u64 = 24 * 3_600_000;
#[document(
    summary = "The second lock's deadline unless the starter says otherwise: 12 hours from when the swap starts.",
    category = "Swaps",
)]
pub const SECOND_DEADLINE_MS: u64 = 12 * 3_600_000;

/// The first byte of terms written as two legs. Terms from before, chainv4 against Bitcoin, began
/// with their chain's tag, 0 or 1, and still read.
const LEGS: u8 = 2;

#[document(
    summary = "One side of a swap: the chain it is on, what is locked there, who locks it and who claims it with the secret. An offer leaves out the answerer's part of each.",
    category = "Swaps",
    see_also = "Terms",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
pub enum Leg {
    #[document(summary = "On chainv4, in its swap contract.")]
    #[wincode(tag = 0)]
    Chain(ChainLeg),
    #[document(summary = "On Bitcoin, in an HTLC.")]
    #[wincode(tag = 1)]
    Bitcoin(BitcoinLeg),
    #[document(summary = "On an EVM chain, such as Ethereum, in Swap.sol.")]
    #[wincode(tag = 2)]
    Evm(EvmLeg),
}

#[document(summary = "A swap's leg on chainv4.", category = "Swaps", see_also = "Leg")]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct ChainLeg {
    #[document(summary = "The swap contract the lock goes into, and both sides look in.")]
    pub contract: Address,
    #[document(summary = "The token locked, or None for the coin.")]
    pub token: Option<Address>,
    #[document(summary = "How much, in its smallest units.")]
    #[wincode(with = "crate::structs::u256::U256Le")]
    pub amount: u256,
    #[document(summary = "Who locks it, and gets it back after the deadline.")]
    pub payer: Option<Address>,
    #[document(summary = "Who takes it with the secret.")]
    pub receiver: Option<Address>,
}

#[document(summary = "A swap's leg on Bitcoin: an HTLC.", category = "Swaps", see_also = "Leg, Htlc")]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct BitcoinLeg {
    #[document(summary = "On Bitcoin's test networks, testnet, signet and regtest, rather than Bitcoin.")]
    pub testnet: bool,
    #[document(summary = "The bitcoin locked, in satoshis.")]
    pub sats: u64,
    #[document(summary = "Whose bitcoin it is: the HTLC's refund key, compressed.")]
    pub payer: Option<[u8; 33]>,
    #[document(summary = "Who takes it with the secret: the HTLC's claim key.")]
    pub receiver: Option<[u8; 33]>,
}

#[document(summary = "A swap's leg on an EVM chain, such as Ethereum: a lock in Swap.sol there.", category = "Swaps", see_also = "Leg")]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct EvmLeg {
    #[document(summary = "The chain, by its EIP-155 id: 1 for Ethereum, 11155111 for Sepolia.")]
    pub chain_id: u64,
    #[document(summary = "The Swap.sol the lock goes into there.")]
    pub contract: [u8; 20],
    #[document(summary = "The ERC-20 token locked, or None for the chain's coin.")]
    pub token: Option<[u8; 20]>,
    #[document(summary = "How much, in its smallest units: wei for the coin.")]
    #[wincode(with = "crate::structs::u256::U256Le")]
    pub amount: u256,
    #[document(summary = "Who locks it: an address there.")]
    pub payer: Option<[u8; 20]>,
    #[document(summary = "Who takes it with the secret.")]
    pub receiver: Option<[u8; 20]>,
}

#[document(summary = "Which leg of a swap: the starter's, locked first, or the other's.", category = "Swaps")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    #[document(summary = "The starter's: locked first, with the longer deadline.")]
    First,
    #[document(summary = "The other's: locked second, once the first stands.")]
    Second,
}

#[document(summary = "Who someone is on a leg: the one who locks it, or the one who claims it.", category = "Swaps")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    #[document(summary = "Locks it, and takes it back after its deadline.")]
    Payer,
    #[document(summary = "Takes it with the secret.")]
    Receiver,
}

#[document(summary = "Someone, or a token, on one chain: as a leg names them.", category = "Swaps")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Party {
    #[document(summary = "A chainv4 address.")]
    Chain(Address),
    #[document(summary = "A compressed Bitcoin key.")]
    Bitcoin([u8; 33]),
    #[document(summary = "An address on an EVM chain.")]
    Evm([u8; 20]),
}

#[document(
    summary = "Who someone is on each kind of chain, to offer or answer with: their chainv4 address, their Bitcoin key on the swap's network, and their address on EVM chains. A swap takes the ones its legs need.",
    category = "Swaps",
)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Parties {
    pub chain: Option<Address>,
    pub bitcoin: Option<[u8; 33]>,
    pub evm: Option<[u8; 20]>,
}

#[document(summary = "Where a lock stands, as its chain's swap contract says.", category = "Swaps", see_also = "Lock")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LockState {
    #[document(summary = "Nothing locked under the hash.")]
    None,
    #[document(summary = "Locked, waiting for its claim.")]
    Locked,
    #[document(summary = "Claimed: the secret is out.")]
    Claimed,
    #[document(summary = "Taken back after its deadline.")]
    Refunded,
}

#[document(
    summary = "A lock as a swap contract shows it, on chainv4 or an EVM chain: what to check against the terms before locking second.",
    category = "Swaps",
    see_also = "Leg::stands",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lock {
    pub state: LockState,
    #[document(summary = "Who it pays with the secret.")]
    pub recipient: Option<Party>,
    #[document(summary = "Its token, or None for the coin.")]
    pub token: Option<Party>,
    pub amount: u256,
    #[document(summary = "Its deadline, in Unix milliseconds.")]
    pub deadline: u64,
    #[document(summary = "The secret, once claimed.")]
    pub secret: Option<[u8; 32]>,
}

#[document(
    summary = "A swap's terms, as both sides hold them once agreed: its two legs, the hash and the deadlines. An offer leaves out the answerer's parts.",
    category = "Swaps",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct Terms {
    #[document(summary = "The SHA-256 of the starter's secret.")]
    pub hash: [u8; 32],
    #[document(summary = "The starter's leg: what they pay, locked first.")]
    pub first: Leg,
    #[document(summary = "The other's leg: what they pay, locked second.")]
    pub second: Leg,
    #[document(summary = "When the first lock opens to a refund: Unix milliseconds.")]
    pub first_deadline: u64,
    #[document(summary = "When the second does.")]
    pub second_deadline: u64,
}

#[document(
    summary = "A secret for a swap's starter: 32 random bytes, and their SHA-256, which the terms carry.",
    category = "Swaps",
)]
pub fn secret() -> Result<([u8; 32], [u8; 32]), String> {
    let mut secret = [0u8; 32];
    getrandom::fill(&mut secret).map_err(|err| format!("no randomness for a secret: {err}"))?;

    Ok((secret, sha256(&secret)))
}

impl Leg {
    #[document(parent = "Leg", summary = "The chain's name: chainv4, Bitcoin, Bitcoin testnet, or an EVM chain's, such as Ethereum or chain 8453.")]
    pub fn chain_name(&self) -> String {
        match self {
            Leg::Chain(_) => "chainv4".to_string(),
            Leg::Bitcoin(leg) => leg.network().name().to_string(),
            Leg::Evm(leg) => evm::chain_name(leg.chain_id),
        }
    }

    #[document(parent = "Leg", summary = "Who locks it, or claims it, when they are in.")]
    pub fn party(&self, role: Role) -> Option<Party> {
        match (self, role) {
            (Leg::Chain(leg), Role::Payer) => leg.payer.map(Party::Chain),
            (Leg::Chain(leg), Role::Receiver) => leg.receiver.map(Party::Chain),
            (Leg::Bitcoin(leg), Role::Payer) => leg.payer.map(Party::Bitcoin),
            (Leg::Bitcoin(leg), Role::Receiver) => leg.receiver.map(Party::Bitcoin),
            (Leg::Evm(leg), Role::Payer) => leg.payer.map(Party::Evm),
            (Leg::Evm(leg), Role::Receiver) => leg.receiver.map(Party::Evm),
        }
    }

    #[document(parent = "Leg", summary = "Someone's part of it, from who they are on its kind of chain: an error when they are nobody there.")]
    pub fn set(&mut self, role: Role, parties: &Parties) -> Result<(), String> {
        let chain = self.chain_name();
        let missing = |what: &str| format!("a swap on {chain} takes your {what}");

        match self {
            Leg::Chain(leg) => {
                let party = Some(parties.chain.ok_or_else(|| missing("chainv4 address"))?);
                match role {
                    Role::Payer => leg.payer = party,
                    Role::Receiver => leg.receiver = party,
                }
            }
            Leg::Bitcoin(leg) => {
                let party = Some(parties.bitcoin.ok_or_else(|| missing("Bitcoin key"))?);
                match role {
                    Role::Payer => leg.payer = party,
                    Role::Receiver => leg.receiver = party,
                }
            }
            Leg::Evm(leg) => {
                let party = Some(parties.evm.ok_or_else(|| missing("address there"))?);
                match role {
                    Role::Payer => leg.payer = party,
                    Role::Receiver => leg.receiver = party,
                }
            }
        }

        Ok(())
    }

    #[document(parent = "Leg", summary = "Whether these parties are its payer, or its receiver.")]
    pub fn is(&self, role: Role, parties: &Parties) -> bool {
        match self.party(role) {
            Some(Party::Chain(address)) => parties.chain == Some(address),
            Some(Party::Bitcoin(key)) => parties.bitcoin == Some(key),
            Some(Party::Evm(address)) => parties.evm == Some(address),
            None => false,
        }
    }

    #[document(parent = "Leg", summary = "Whether both its payer and its receiver are in.")]
    pub fn agreed(&self) -> bool {
        self.party(Role::Payer).is_some() && self.party(Role::Receiver).is_some()
    }

    #[document(parent = "Leg", summary = "Whether two legs are on one chain, which a swap is not.")]
    pub fn same_chain(&self, other: &Leg) -> bool {
        match (self, other) {
            (Leg::Chain(_), Leg::Chain(_)) | (Leg::Bitcoin(_), Leg::Bitcoin(_)) => true,
            (Leg::Evm(one), Leg::Evm(two)) => one.chain_id == two.chain_id,
            _ => false,
        }
    }

    #[document(parent = "Leg", summary = "What it locks, in smallest units: 100000 sats, 5000 of the coin, or 5000 of a token.")]
    pub fn what(&self) -> String {
        match self {
            Leg::Chain(leg) => match leg.token {
                None => format!("{} of the coin", leg.amount),
                Some(token) => format!("{} of token {token}", leg.amount),
            },
            Leg::Bitcoin(leg) => format!("{} sats", leg.sats),
            Leg::Evm(leg) => match leg.token {
                None => format!("{} wei", leg.amount),
                Some(token) => format!("{} of token {}", leg.amount, to_hex(&token)),
            },
        }
    }

    #[document(
        parent = "Leg",
        summary = "Checks a lock its chain's contract shows against it: locked, or since claimed, for its receiver, with its token, amount and deadline, at the precision its chain keeps. Lock second only once the first stands.",
    )]
    pub fn stands(&self, lock: &Lock, deadline: u64) -> Result<(), String> {
        let (receiver, token, amount, deadline) = match self {
            Leg::Chain(leg) => (leg.receiver.map(Party::Chain), leg.token.map(Party::Chain), leg.amount, deadline),
            // Swap.sol keeps whole seconds.
            Leg::Evm(leg) => (leg.receiver.map(Party::Evm), leg.token.map(Party::Evm), leg.amount, deadline / 1_000 * 1_000),
            Leg::Bitcoin(_) => return Err("Bitcoin's lock is an HTLC, not a contract's: look its address up".to_string()),
        };

        match lock.state {
            LockState::None => return Err("nothing is locked yet".to_string()),
            LockState::Refunded => return Err("it was refunded".to_string()),
            LockState::Locked | LockState::Claimed => {}
        }

        if lock.recipient != receiver {
            return Err("it is locked for someone else".to_string());
        }

        if lock.token != token || lock.amount != amount {
            return Err(format!("it locks {} of {}, not what the terms say", lock.amount, lock.token.map_or("the coin".to_string(), |token| format!("{token:?}"))));
        }

        if lock.deadline != deadline {
            return Err("its deadline is not the terms'".to_string());
        }

        Ok(())
    }

    fn check(&self, deadline: u64) -> Result<(), String> {
        let distinct = |payer: Option<Party>, receiver: Option<Party>| match payer.is_some() && payer == receiver {
            true => Err(format!("the same party pays and receives on {}", self.chain_name())),
            false => Ok(()),
        };
        distinct(self.party(Role::Payer), self.party(Role::Receiver))?;

        match self {
            Leg::Chain(leg) if leg.amount == u256::ZERO => Err("a swap locks more than nothing on chainv4".to_string()),
            Leg::Chain(_) => Ok(()),
            Leg::Bitcoin(leg) => {
                if leg.sats < htlc::DUST {
                    return Err(format!("a swap locks at least {} satoshis on Bitcoin", htlc::DUST));
                }

                if deadline / 1_000 < u64::from(htlc::LOCKTIME_THRESHOLD) || deadline / 1_000 > u64::from(u32::MAX) {
                    return Err("a Bitcoin deadline Bitcoin cannot write as a time".to_string());
                }

                match [leg.payer, leg.receiver].into_iter().flatten().all(|key| crate::crypto::secp256k1::valid_key(&key)) {
                    true => Ok(()),
                    false => Err("a Bitcoin key that is not a key".to_string()),
                }
            }
            Leg::Evm(leg) => {
                if leg.amount == u256::ZERO {
                    return Err(format!("a swap locks more than nothing on {}", self.chain_name()));
                }

                let zero = [0; 20];
                match leg.contract == zero || leg.token == Some(zero) || leg.payer == Some(zero) || leg.receiver == Some(zero) {
                    true => Err(format!("the zero address, on {}", self.chain_name())),
                    false => Ok(()),
                }
            }
        }
    }
}

impl BitcoinLeg {
    #[document(parent = "BitcoinLeg", summary = "The Bitcoin network it is on.")]
    pub fn network(&self) -> Network {
        match self.testnet {
            true => Network::Test,
            false => Network::Main,
        }
    }
}

impl Terms {
    #[document(parent = "Terms", summary = "A leg, the starter's or the other's.")]
    pub fn leg(&self, side: Side) -> &Leg {
        match side {
            Side::First => &self.first,
            Side::Second => &self.second,
        }
    }

    #[document(parent = "Terms", summary = "When a leg's lock ends: the first's deadline, or the second's.")]
    pub fn deadline(&self, side: Side) -> u64 {
        match side {
            Side::First => self.first_deadline,
            Side::Second => self.second_deadline,
        }
    }

    #[document(parent = "Terms", summary = "Both legs, the first then the second, with their sides.")]
    pub fn legs(&self) -> [(Side, &Leg); 2] {
        [(Side::First, &self.first), (Side::Second, &self.second)]
    }

    #[document(parent = "Terms", summary = "Whether both sides' parts are in: an answer, rather than an offer.")]
    pub fn agreed(&self) -> bool {
        self.first.agreed() && self.second.agreed()
    }

    #[document(parent = "Terms", summary = "The leg these parties pay, and the one they receive, as far as they are in either.")]
    pub fn mine(&self, parties: &Parties) -> (Option<Side>, Option<Side>) {
        let find = |role| self.legs().into_iter().find(|(_, leg)| leg.is(role, parties)).map(|(side, _)| side);

        (find(Role::Payer), find(Role::Receiver))
    }

    #[document(
        parent = "Terms",
        summary = "An offer: the starter pays the first leg and receives the second. Who they are on each leg's chain is set from their parties.",
    )]
    pub fn offer(hash: [u8; 32], mut first: Leg, mut second: Leg, first_deadline: u64, second_deadline: u64, starter: &Parties) -> Result<Terms, String> {
        first.set(Role::Payer, starter)?;
        second.set(Role::Receiver, starter)?;

        Ok(Terms { hash, first, second, first_deadline, second_deadline })
    }

    #[document(parent = "Terms", summary = "The answer to an offer: its terms, with the other side as the first leg's receiver and the second's payer.")]
    pub fn answer(&self, parties: &Parties) -> Result<Terms, String> {
        if self.agreed() {
            return Err("those terms are answered already".to_string());
        }

        let mut answered = self.clone();
        answered.first.set(Role::Receiver, parties)?;
        answered.second.set(Role::Payer, parties)?;

        Ok(answered)
    }

    #[document(
        parent = "Terms",
        summary = "Whether these terms answer an offer: everything it said, as it said it, with the other side's parts added and nothing else.",
    )]
    pub fn answers(&self, offer: &Terms) -> bool {
        let kept = |given: Option<Party>, now: Option<Party>| given.is_none() || given == now;
        let parts = |leg: &Leg| {
            let mut bare = leg.clone();
            match &mut bare {
                Leg::Chain(leg) => (leg.payer, leg.receiver) = (None, None),
                Leg::Bitcoin(leg) => (leg.payer, leg.receiver) = (None, None),
                Leg::Evm(leg) => (leg.payer, leg.receiver) = (None, None),
            }
            bare
        };
        let same = (offer.hash, parts(&offer.first), parts(&offer.second), offer.first_deadline, offer.second_deadline)
            == (self.hash, parts(&self.first), parts(&self.second), self.first_deadline, self.second_deadline);

        same && self.agreed()
            && [(&offer.first, &self.first), (&offer.second, &self.second)]
                .into_iter()
                .all(|(given, now)| kept(given.party(Role::Payer), now.party(Role::Payer)) && kept(given.party(Role::Receiver), now.party(Role::Receiver)))
    }

    #[document(parent = "Terms", summary = "Its Bitcoin leg, if it has one, and which side it is.")]
    pub fn bitcoin(&self) -> Option<(Side, &BitcoinLeg)> {
        self.legs().into_iter().find_map(|(side, leg)| match leg {
            Leg::Bitcoin(leg) => Some((side, leg)),
            _ => None,
        })
    }

    #[document(
        parent = "Terms",
        summary = "The HTLC of its Bitcoin leg, once both its keys are in. Its lock time is that leg's deadline in seconds.",
    )]
    pub fn htlc(&self) -> Option<htlc::Htlc> {
        let (side, leg) = self.bitcoin()?;

        Some(htlc::Htlc {
            hash: self.hash,
            claim: leg.receiver?,
            refund: leg.payer?,
            locktime: u32::try_from(self.deadline(side) / 1_000).ok()?,
        })
    }

    #[document(
        parent = "Terms",
        summary = "Checks the terms hold together, at a time: two chains, something on each leg, deadlines ahead and far enough apart, keys that are keys and nobody on both ends of a leg.",
    )]
    pub fn check(&self, now: u64) -> Result<(), String> {
        if self.first.same_chain(&self.second) {
            return Err(format!("both legs on {}: a swap is between two chains", self.first.chain_name()));
        }

        if self.second_deadline <= now {
            return Err("its deadlines have passed".to_string());
        }

        if self.second_deadline + MIN_GAP_MS > self.first_deadline {
            return Err(format!("the second lock must end at least {} hours before the first, so the second side has time to claim", MIN_GAP_MS / 3_600_000));
        }

        for (side, leg) in self.legs() {
            leg.check(self.deadline(side))?;
        }

        Ok(())
    }

    #[document(parent = "Terms", summary = "The terms as a code: ur:chainv4-swap, in capitals.")]
    pub fn to_text(&self) -> String {
        ur::encode(&self.to_bytes(), &ur::Type::Custom(UR_TYPE)).to_ascii_uppercase()
    }

    #[document(parent = "Terms", summary = "Its encoding: a 2, then its fields.")]
    pub fn to_bytes(&self) -> Vec<u8> {
        [vec![LEGS], wincode::serialize(self).expect("terms always encode")].concat()
    }

    #[document(
        parent = "Terms",
        summary = "Reads terms from their code, whole: anything past them is refused.",
    )]
    pub fn from_text(text: &str) -> Result<Terms, String> {
        let text = text.trim().to_ascii_lowercase();

        if !text.starts_with(&format!("ur:{UR_TYPE}/")) {
            return Err(format!("not a swap's terms: those start ur:{UR_TYPE}/"));
        }

        match ur::decode(&text).map_err(|err| format!("a code that does not read: {err:?}"))? {
            (ur::ur::Kind::SinglePart, bytes) => Terms::from_bytes(&bytes),
            (ur::ur::Kind::MultiPart, _) => Err("one part of an animated code: read every part".to_string()),
        }
    }

    #[document(
        parent = "Terms",
        summary = "Reads terms from their encoding, all of these bytes: two legs, or the chainv4-against-Bitcoin terms written before legs.",
    )]
    pub fn from_bytes(bytes: &[u8]) -> Result<Terms, String> {
        match bytes.first() {
            Some(&LEGS) => {
                let terms: Terms = wincode::deserialize(&bytes[1..]).map_err(|_| "terms that do not decode".to_string())?;

                match terms.to_bytes() == bytes {
                    true => Ok(terms),
                    false => Err("terms with more in them than they hold".to_string()),
                }
            }
            Some(0 | 1) => legacy::read(bytes),
            _ => Err("terms of a kind this build does not read".to_string()),
        }
    }
}

impl Lock {
    #[document(
        parent = "Lock",
        summary = "A lock as chainv4's swap contract shows it, from get's values as text: state, locker, recipient, token (the zero address for the coin), amount, deadline and secret.",
    )]
    pub fn from_chain(values: &[String]) -> Result<Lock, String> {
        let state = match values.first().map(String::as_str) {
            Some("none") => LockState::None,
            Some("locked") => LockState::Locked,
            Some("claimed") => LockState::Claimed,
            Some("refunded") => LockState::Refunded,
            _ => return Err("an answer the swap contract does not give".to_string()),
        };

        if state == LockState::None {
            return Ok(Lock { state, recipient: None, token: None, amount: u256::ZERO, deadline: 0, secret: None });
        }

        let [_, _, recipient, token, amount, deadline, secret] = values else {
            return Err("an answer the swap contract does not give".to_string());
        };
        let address = |text: &str| text.parse::<Address>().map_err(|_| format!("{text} is not an address"));
        let number = |text: &str| match text.strip_prefix("0x") {
            Some(hex) => u256::from_str_radix(hex, 16),
            None => u256::from_str_radix(text, 10),
        }
        .map_err(|_| format!("{text} is not a number"));
        let token = address(token)?;
        let secret = crate::structs::hex::from_hex(secret).and_then(|bytes| <[u8; 32]>::try_from(bytes).ok());

        Ok(Lock {
            state,
            recipient: Some(Party::Chain(address(recipient)?)),
            token: (token != Address::Ed25519([0; 32])).then_some(Party::Chain(token)),
            amount: number(amount)?,
            deadline: u64::try_from(number(deadline)?).map_err(|_| "a deadline past any time".to_string())?,
            secret,
        })
    }
}

#[cfg(test)]
mod tests;
