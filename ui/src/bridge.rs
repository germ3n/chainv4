//! Transfers through the bridges' gateways (docs/bridges.md), as both apps say where one stands:
//! being signed, signed, arrived, stuck, being voided, voided or refunded, the furthest along
//! first, so the same whatever order it was learned in. And, with the bridge feature, a transfer
//! that left the gateway here, read back from its sent(nonce) only as the gateway signed it. Plain
//! data, tested as such; what each app reads a transfer from is its own.

#[cfg(feature = "bridge")]
use {
    chainv4::bridge::{AssetKind, Transfer},
    chainv4::structs::hex::from_hex,
    chainv4::structs::u256::u256,
    serde_json::Value,
};
use {crate::theme::Palette, egui::Color32};

/// Where a transfer through the gateways stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Standing {
    /// Its signers are signing it: so many, of how many the federation needs when that is known.
    Signing { signed: u64, threshold: Option<u64> },
    /// Signed by enough: anyone may deliver it where it goes.
    Signed,
    /// Done where it went.
    Arrived,
    /// Left for a gateway the chain's directory no longer names there: no signer signs it, nor
    /// voids it, so it stays where it left unless that peer is named again.
    Stuck,
    /// The federation signing to void it where it was going: so many, of how many.
    Voiding { signed: u64, threshold: Option<u64> },
    /// Refused for good where it was going: where it left pays it back, once the federation signs
    /// that.
    Voided,
    /// Paid back where it left, to who sent it or where they said.
    Refunded,
}

/// What is known of a transfer, which its standing follows from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Known {
    /// It left the chain it is read on, so it is delivered on another.
    pub leaving: bool,
    /// How many signed it, and how many the federation needs, when that is known.
    pub signed: u64,
    pub threshold: Option<u64>,
    pub arrived: bool,
    pub stuck: bool,
    /// How many signed to void it.
    pub void_signed: u64,
    /// Its destination took its nonce for good, or its signers began to refund it, which they do
    /// only once they read that.
    pub voided: bool,
    pub refunded: bool,
}

impl Standing {
    /// Where what is known of it puts it, the furthest along first: paid back, voided, arrived. A
    /// void and a delivery cannot both land, whichever comes first winning, and stuck says why
    /// nothing more happens to one that is neither.
    pub fn of(known: Known) -> Standing {
        match known {
            Known { refunded: true, .. } => Standing::Refunded,
            Known { voided: true, .. } => Standing::Voided,
            Known { arrived: true, .. } => Standing::Arrived,
            Known { stuck: true, .. } => Standing::Stuck,
            Known { void_signed: 1.., threshold, .. } => Standing::Voiding { signed: known.void_signed, threshold },
            Known { leaving: true, signed, threshold: Some(threshold), .. } if signed >= threshold => Standing::Signed,
            Known { signed, threshold, .. } => Standing::Signing { signed, threshold },
        }
    }

    /// Its words, for a person.
    pub fn word(self) -> String {
        match self {
            Standing::Signing { signed, threshold: Some(threshold) } => format!("signing, {signed} of {threshold}"),
            Standing::Signing { signed, threshold: None } => format!("{signed} signed"),
            Standing::Signed => "signed, to deliver".to_string(),
            Standing::Arrived => "arrived".to_string(),
            Standing::Stuck => "stuck: its peer is gone".to_string(),
            Standing::Voiding { signed, threshold: Some(threshold) } if signed >= threshold => "void signed, to deliver".to_string(),
            Standing::Voiding { signed, threshold: Some(threshold) } => format!("voiding, {signed} of {threshold}"),
            Standing::Voiding { signed, threshold: None } => format!("voiding, {signed} signed"),
            Standing::Voided => "voided, refund to come".to_string(),
            Standing::Refunded => "refunded".to_string(),
        }
    }

    /// Its color: done, waiting on someone, or going nowhere.
    pub fn color(self, palette: &Palette) -> Color32 {
        match self {
            Standing::Arrived => palette.good,
            Standing::Signed => palette.accent,
            Standing::Stuck => palette.bad,
            Standing::Signing { .. } | Standing::Voiding { .. } => palette.warn,
            Standing::Voided | Standing::Refunded => palette.soft,
        }
    }

    /// Whether it is on its way back, or back: being voided, voided or refunded.
    pub fn refunding(self) -> bool {
        matches!(self, Standing::Voiding { .. } | Standing::Voided | Standing::Refunded)
    }
}

/// A value as text: a string as it is, anything else as JSON writes it.
#[cfg(feature = "bridge")]
fn text(value: &Value) -> String {
    value["value"].as_str().map_or_else(|| value["value"].to_string(), str::to_string)
}

/// A value's bytes: bytes as they are, text as its own.
#[cfg(feature = "bridge")]
fn raw(value: &Value) -> Vec<u8> {
    match value["type"].as_str() {
        Some("bytes") => value["value"].as_str().and_then(from_hex).unwrap_or_default(),
        _ => value["value"].as_str().unwrap_or_default().as_bytes().to_vec(),
    }
}

#[cfg(feature = "bridge")]
fn whole(value: &Value) -> Option<u64> {
    text(value).parse().ok()
}

#[cfg(feature = "bridge")]
fn wide(value: &Value) -> Option<u256> {
    u256::from_str_radix(&text(value), 10).ok()
}

/// A transfer that left the gateway here, as its sent(nonce) gives it, each value as a node's /call
/// writes it, checked against the digest the gateway keeps, so it is the one the signers signed.
/// identity is the gateway as transfers write it: its address and when it was set up. None for
/// another, which is also what one that left for a Gateway.sol other than evm_gateway reads as.
#[cfg(feature = "bridge")]
pub fn left(values: &[Value], identity: &[u8], evm_gateway: [u8; 20], here: u64, nonce: u64) -> Option<Transfer> {
    let transfer = Transfer {
        from: identity.to_vec(),
        to: evm_gateway.to_vec(),
        source: here,
        target: whole(values.first()?)?,
        nonce,
        kind: AssetKind::from_number(whole(values.get(1)?)?)?,
        origin: whole(values.get(2)?)?,
        asset: raw(values.get(3)?),
        id: wide(values.get(4)?)?,
        amount: wide(values.get(5)?)?,
        recipient: raw(values.get(6)?),
        name: text(values.get(7)?),
        symbol: text(values.get(8)?),
        decimals: u8::try_from(whole(values.get(9)?)?).ok()?,
        uri: text(values.get(10)?),
    };

    (raw(values.get(11)?) == transfer.digest()).then_some(transfer)
}

#[cfg(test)]
mod tests;
