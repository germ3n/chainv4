//! Signing on a device that never goes online. One install is the signer: it holds the keys and
//! makes no requests at all. Another is the sender: it holds the signer's public keys alone, proves
//! their balances, builds what to send and gets it priced, and takes it to the node once signed.
//! Between them go three messages, each a QR code one device shows and the other photographs, or
//! the same text copied or in a file:
//! - **Pairing:** an account's public keys, from the signer, for the sender to watch.
//! - **A request:** the transaction, its nonce and fee offer, the key to sign with, and how the
//!   sender's node names the coin and the token, for the signer to show what it signs.
//! - **The signed transaction,** back to the sender, which checks it is the one it asked for.
//!
//! Each is `CV4:1:`, its kind, and its encoding in base45, whose 45 characters are just those a QR
//! code packs densest. Text read from a code that is not exactly one message is refused.
//!
//! A message too long for one code that photographs well goes as parts, each a code of its own:
//! `CV4:1:PART:2/5:`, a short hash of the whole text, and a stretch of it. They are read in any
//! order and put back together, and the whole is checked against the hash before it is read.

use {
    crate::format,
    chainv4::crypto::hasher::Hasher,
    chainv4::crypto::keypair::{Keypair, PublicKey, Signature},
    chainv4::crypto::verifiable::Verifiable,
    chainv4::structs::hash::HashType,
    chainv4::registry::names::NameAction,
    chainv4::structs::tx::{FeeOffer, SignedTx, Tx, TxCallArgumentType},
    chainv4::structs::u256::u256,
    wincode::{SchemaRead, SchemaWrite},
};

/// What every message starts with: the wallet's, and the version of the messages.
pub const PREFIX: &str = "CV4:1:";

/// The most of a message's text one code holds: about as dense as a code a phone photographs off
/// a screen reliably. Longer ones go as parts.
pub const PART_SIZE: usize = 600;

/// The most parts a message goes as, and so the longest one read by camera: 38,400 characters.
const MAX_PARTS: usize = 64;

/// The widths a photo is read at, the first that finds a code winning: a phone's camera takes
/// thousands of pixels across, more than a code needs and slow to search, so smaller first.
const SEARCH_WIDTHS: [u32; 3] = [1_200, 2_000, 800];

/// An account's public keys, one per scheme it has an address in, as the signer shows them.
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct Pairing {
    pub name: String,
    pub keys: Vec<PublicKey>,
}

/// How the sender's node names an asset: what the signer shows amounts in. The signer cannot
/// check it, so it shows the raw units signed beside them.
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct Shown {
    pub symbol: String,
    pub decimals: u8,
}

/// A transaction to sign, as the sender asks for it.
#[derive(Clone, Debug, PartialEq, SchemaWrite, SchemaRead)]
pub struct Request {
    pub tx: Tx,
    pub nonce: u64,
    pub fee: FeeOffer,
    /// The key to sign with: the sender's address is its.
    pub signer: PublicKey,
    pub coin: Shown,
    /// The token a token transfer sends, as the registry lists it.
    pub token: Option<Shown>,
}

/// One of the three messages.
#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    Pair(Pairing),
    Sign(Request),
    Signed(SignedTx),
}

impl Message {
    /// Its text: what its QR code holds.
    pub fn to_text(&self) -> String {
        let (kind, bytes) = match self {
            Message::Pair(pairing) => ("PAIR", wincode::serialize(pairing)),
            Message::Sign(request) => ("SIGN", wincode::serialize(request)),
            Message::Signed(signed) => ("SIGNED", wincode::serialize(signed)),
        };

        format!("{PREFIX}{kind}:{}", base45::encode(&bytes.expect("messages always encode")))
    }

    /// Reads a message's text, whole: one with anything past its encoding is refused. Spaces
    /// around it go, as pasting brings them: base45 has a space, but never last, since a group's
    /// last character is its high part, at most W.
    pub fn from_text(text: &str) -> Result<Message, String> {
        let body = text.trim().strip_prefix(PREFIX).ok_or("not a chainv4 wallet code")?;
        let (kind, encoded) = body.split_once(':').ok_or("a code with no kind")?;
        let bytes = base45::decode(encoded).ok_or("a code that does not read: is it all there?")?;

        match kind {
            "PAIR" => whole::<Pairing>(&bytes).map(Message::Pair),
            "SIGN" => whole::<Request>(&bytes).map(Message::Sign),
            "SIGNED" => whole::<SignedTx>(&bytes).map(Message::Signed),
            other => Err(format!("a kind of code this wallet does not know: {other}")),
        }
    }

    /// What it is, in a word or two, for saying what came instead of what was wanted.
    pub fn name(&self) -> &'static str {
        match self {
            Message::Pair(_) => "an account to pair with",
            Message::Sign(_) => "a request to sign",
            Message::Signed(_) => "a signed transaction",
        }
    }
}

/// What one code holds: a whole message, boxed as it is many times a part, or a part of one too
/// long for a code.
#[derive(Clone, Debug, PartialEq)]
pub enum Code {
    Whole(Box<Message>),
    Part(Part),
}

/// A part of a message's text, and which.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Part {
    /// The short hash of the whole text it is part of.
    pub id: String,
    /// Which part, from 1, of how many.
    pub index: usize,
    pub count: usize,
    pub text: String,
}

impl Code {
    /// Reads a code's text: a whole message, or a part of one.
    pub fn from_text(text: &str) -> Result<Code, String> {
        let text = text.trim();

        match text.strip_prefix(PREFIX).and_then(|rest| rest.strip_prefix("PART:")) {
            Some(rest) => part(rest).map(Code::Part),
            None => Message::from_text(text).map(|message| Code::Whole(Box::new(message))),
        }
    }
}

/// The codes a message's text goes as: itself, when it fits one, or parts of about equal length.
pub fn codes(text: &str) -> Vec<String> {
    if text.len() <= PART_SIZE {
        return vec![text.to_string()];
    }

    let count = text.len().div_ceil(PART_SIZE);
    let size = text.len().div_ceil(count);
    let id = ident(text);

    // The text is all ASCII, so any stretch of its bytes is text too.
    text.as_bytes()
        .chunks(size)
        .enumerate()
        .map(|(index, stretch)| format!("{PREFIX}PART:{}/{count}:{id}:{}", index + 1, String::from_utf8_lossy(stretch)))
        .collect()
}

/// The short hash parts name the whole text by: eight hex digits, in the characters a QR code
/// packs densest.
fn ident(text: &str) -> String {
    Hasher::digest(HashType::Xxh3_64, text.as_bytes()).to_bytes()[..4].iter().map(|byte| format!("{byte:02X}")).collect()
}

/// Reads a part's text past `CV4:1:PART:`: which of how many, the whole's hash, and its stretch.
fn part(rest: &str) -> Result<Part, String> {
    let bad = || "a part of a code that does not read".to_string();
    let (which, rest) = rest.split_once(':').ok_or_else(bad)?;
    let (id, text) = rest.split_once(':').ok_or_else(bad)?;
    let (index, count) = which.split_once('/').ok_or_else(bad)?;
    let (index, count): (usize, usize) = (index.parse().map_err(|_| bad())?, count.parse().map_err(|_| bad())?);

    if index == 0 || index > count || !(2..=MAX_PARTS).contains(&count) || id.len() != 8 || text.is_empty() {
        return Err(bad());
    }

    Ok(Part {
        id: id.to_string(),
        index,
        count,
        text: text.to_string(),
    })
}

/// The parts of one message read so far, until every one is in.
#[derive(Clone, Debug, Default)]
pub struct Assembly {
    id: String,
    parts: Vec<Option<String>>,
}

impl Assembly {
    /// Takes a part. A part of another message starts over with it. Gives the message once every
    /// part is in, put back together and checked against the hash its parts name it by.
    pub fn add(&mut self, part: Part) -> Result<Option<Message>, String> {
        if part.id != self.id || part.count != self.parts.len() {
            *self = Assembly {
                id: part.id.clone(),
                parts: vec![None; part.count],
            };
        }

        self.parts[part.index - 1] = Some(part.text);

        if self.parts.iter().any(Option::is_none) {
            return Ok(None);
        }

        let text: String = std::mem::take(self).parts.into_iter().flatten().collect();

        match ident(&text) == part.id {
            true => Message::from_text(&text).map(Some),
            false => Err("the parts do not make the message they name: read them again".to_string()),
        }
    }

    /// How many parts the message goes as, and which, from 1, are still to read; none before the
    /// first part.
    pub fn missing(&self) -> (usize, Vec<usize>) {
        let missing = self.parts.iter().enumerate().filter(|(_, part)| part.is_none()).map(|(index, _)| index + 1).collect();

        (self.parts.len(), missing)
    }
}

/// Decodes a value that is all of these bytes.
fn whole<T>(bytes: &[u8]) -> Result<T, String>
where
    T: for<'de> SchemaRead<'de, wincode::config::DefaultConfig, Dst = T> + SchemaWrite<wincode::config::DefaultConfig, Src = T>,
{
    let value: T = wincode::deserialize(bytes).map_err(|_| "a code that does not decode".to_string())?;

    match wincode::serialize(&value).ok().as_deref() == Some(bytes) {
        true => Ok(value),
        false => Err("a code with more in it than it should hold".to_string()),
    }
}

/// Reads a code from a file: a message's text, or a photo or image of its QR code or of a part's.
pub fn read(bytes: &[u8]) -> Result<Code, String> {
    match std::str::from_utf8(bytes) {
        Ok(text) if text.trim().starts_with(PREFIX) => Code::from_text(text),
        _ => Code::from_text(&code_in(bytes)?),
    }
}

/// The longest side a camera's frame is searched at: frames come many a second, so one size, and a
/// small one, as a code held up to the camera fills much of it.
const FRAME_WIDTH: u32 = 800;

/// The text of a QR code in a camera's frame, in grey: the first that reads, of any kind; None
/// when none does. Whose code it is, the reader says.
pub fn code_in_frame(width: usize, height: usize, luma: &[u8]) -> Option<String> {
    let image = image::GrayImage::from_raw(u32::try_from(width).ok()?, u32::try_from(height).ok()?, luma.to_vec())?;
    let longest = image.width().max(image.height());
    let image = match longest > FRAME_WIDTH {
        true => {
            let scale = FRAME_WIDTH as f32 / longest as f32;
            let (w, h) = ((image.width() as f32 * scale) as u32, (image.height() as f32 * scale) as u32);

            image::imageops::resize(&image, w.max(1), h.max(1), image::imageops::FilterType::Triangle)
        }
        false => image,
    };
    let mut prepared = rqrr::PreparedImage::prepare_from_greyscale(image.width() as usize, image.height() as usize, |x, y| image.get_pixel(x as u32, y as u32).0[0]);

    prepared.detect_grids().into_iter().find_map(|grid| grid.decode().ok().map(|(_, text)| text))
}

/// The text of the wallet's QR code in an image.
fn code_in(bytes: &[u8]) -> Result<String, String> {
    let image = image::load_from_memory(bytes).map_err(|_| "not an image this wallet reads: a PNG or a JPEG".to_string())?.into_luma8();
    let mut found_other = false;

    for width in SEARCH_WIDTHS {
        let scaled = match image.width().max(image.height()) > width {
            true => {
                let scale = width as f32 / image.width().max(image.height()) as f32;
                let (w, h) = ((image.width() as f32 * scale) as u32, (image.height() as f32 * scale) as u32);

                image::imageops::resize(&image, w.max(1), h.max(1), image::imageops::FilterType::Triangle)
            }
            false => image.clone(),
        };
        let mut prepared = rqrr::PreparedImage::prepare_from_greyscale(scaled.width() as usize, scaled.height() as usize, |x, y| {
            scaled.get_pixel(x as u32, y as u32).0[0]
        });

        for grid in prepared.detect_grids() {
            match grid.decode() {
                Ok((_, text)) if text.starts_with(PREFIX) => return Ok(text),
                Ok(_) => found_other = true,
                Err(_) => {}
            }
        }

        // Smaller than it was taken at already: a larger search finds no more.
        if image.width().max(image.height()) <= width {
            break;
        }
    }

    Err(match found_other {
        true => "that QR code is not one of this wallet's".to_string(),
        false => "no QR code found: hold the camera steady, close enough that the code fills most of the photo".to_string(),
    })
}

/// What the signer holds a request against before it signs: the key it names is one of its, and
/// the transaction is one the chain could take from that key.
pub fn sign(request: &Request, key: &Keypair) -> Result<SignedTx, String> {
    if key.public() != request.signer {
        return Err("that key is not the one the request names".to_string());
    }

    let signed = SignedTx::sign(request.tx.clone(), request.nonce, request.fee, key);
    signed.verify().map_err(|err| format!("the chain would refuse this transaction: {err}"))?;

    Ok(signed)
}

/// Whether a signed transaction is exactly the one asked for, and its signature holds.
pub fn check(request: &Request, signed: &SignedTx) -> Result<(), String> {
    let same = signed.tx == request.tx && signed.nonce == request.nonce && signed.fee == request.fee && signed.signer == request.signer;

    if !same {
        return Err("that is not the transaction this wallet asked to sign".to_string());
    }

    signed.verify().map_err(|err| format!("its signature does not hold: {err}"))
}

/// What the node prices a transaction from: the transaction as the signer would sign it, with a
/// signature of the right length that signs nothing, since the node checks none for a price.
pub fn probe(tx: Tx, nonce: u64, signer: PublicKey) -> SignedTx {
    let signature = match signer {
        PublicKey::Ed25519(_) => Signature::Ed25519([0; 64]),
        PublicKey::EdBlake(_) => Signature::EdBlake([0; 64]),
        PublicKey::Secp256k1(_) => Signature::Secp256k1([0; 64]),
    };

    SignedTx {
        tx,
        nonce,
        fee: FeeOffer::NONE,
        signer,
        signature,
    }
}

/// What a request asks to sign, as the signer lays it out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Described {
    /// What kind of transaction: sending the coin, sending a token, or something else.
    pub kind: String,
    /// What it sends, in the asset's units as the sender names them, and the raw units signed.
    pub what: Option<(String, String)>,
    pub from: String,
    pub to: Option<String>,
    /// The token's contract, for a token transfer.
    pub token: Option<String>,
    /// The most the fee may come to, in the coin, and the gas and price it is from.
    pub fee: String,
    pub gas: String,
    pub nonce: u64,
    /// Everything the transaction holds, for any kind the page lays out no better.
    pub everything: String,
}

/// Lays a request out for the signer to read.
pub fn describe(request: &Request) -> Described {
    let coin = &request.coin;
    let most = u256::from(request.fee.gas_limit) * request.fee.max_price;
    let shown = |raw: u256, asset: &Shown| (format::amount(&raw.to_string(), asset.decimals, &asset.symbol), raw.to_string());
    let (kind, what, to, token) = match &request.tx {
        Tx::Transfer { to, amount, .. } => (format!("Send {}", coin.symbol), Some(shown(amount.to_u256(), coin)), Some(to.to_string()), None),
        Tx::ContractCall { to: contract, func, amount, data } if is_token_transfer(func, amount.to_u256(), data) => {
            let [TxCallArgumentType::Address(recipient), TxCallArgumentType::U256(units)] = data.as_slice() else {
                unreachable!("is_token_transfer looked")
            };
            let asset = request.token.clone().unwrap_or(Shown {
                symbol: "units of the token".to_string(),
                decimals: 0,
            });

            (format!("Send {}", asset.symbol), Some(shown(*units, &asset)), Some(recipient.to_string()), Some(contract.to_string()))
        }
        Tx::ContractCall { to: contract, .. } => ("Call a contract".to_string(), None, Some(contract.to_string()), None),
        Tx::Name { name, action } => (name_kind(action), None, Some(name_detail(name, action)), None),
        Tx::PrimaryName { name: Some(name) } => ("Choose a primary name".to_string(), None, Some(name.clone()), None),
        Tx::PrimaryName { name: None } => ("Clear the primary name".to_string(), None, None, None),
        other => (format!("{} transaction", variant(other)), None, None, None),
    };

    Described {
        kind,
        what,
        from: request.signer.address().to_string(),
        to,
        token,
        fee: format::amount(&most.to_string(), coin.decimals, &coin.symbol),
        gas: format!("{} gas at up to {} per gas", request.fee.gas_limit, format::amount(&request.fee.max_price.to_string(), coin.decimals, &coin.symbol)),
        nonce: request.nonce,
        everything: format!("{:#?}", request.tx),
    }
}

/// What a name transaction does, in a few words.
fn name_kind(action: &NameAction) -> String {
    match action {
        NameAction::Register { .. } => "Register a name",
        NameAction::Renew { .. } => "Renew a name",
        NameAction::Transfer { .. } => "Give a name away",
        NameAction::Target { .. } => "Point a name",
        NameAction::Text { .. } => "Set a name's text record",
        NameAction::Subname { .. } => "Make a subname",
    }
    .to_string()
}

/// The name, without the chain's suffix, and what the action sets of it: the signer cannot know
/// the price a registration burns, which the chain charges as it runs.
fn name_detail(name: &str, action: &NameAction) -> String {
    match action {
        NameAction::Register { years } | NameAction::Renew { years } => format!("{name}, for {years} year{}", if *years == 1 { "" } else { "s" }),
        NameAction::Transfer { to } => format!("{name}, to {to}"),
        NameAction::Target { target: Some(target) } => format!("{name}, at {target}"),
        NameAction::Target { target: None } => format!("{name}, at nothing"),
        NameAction::Text { key, value: Some(value) } => format!("{name}: {key} = {value}"),
        NameAction::Text { key, value: None } => format!("{name}: clear {key}"),
        NameAction::Subname { label, owner } => format!("{label}.{name}, owned by {owner}"),
    }
}

/// Whether a call is the token standard's transfer, as the Send page makes one: to, and how much.
fn is_token_transfer(func: &chainv4::structs::hash::Hash, value: u256, data: &[TxCallArgumentType]) -> bool {
    *func == Hasher::digest(HashType::Xxh3_64, b"transfer")
        && value == u256::ZERO
        && matches!(data, [TxCallArgumentType::Address(_), TxCallArgumentType::U256(_)])
}

/// A transaction's kind, as its type names it: Bond, Deploy, and so on.
fn variant(tx: &Tx) -> String {
    let debug = format!("{tx:?}");

    debug.split([' ', '{', '(']).next().unwrap_or("A").to_string()
}

/// RFC 9285's base45: two bytes to three characters of the 45 a QR code's alphanumeric mode holds,
/// which it packs at 5.5 bits each.
mod base45 {
    const ALPHABET: &[u8; 45] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ $%*+-./:";

    pub fn encode(bytes: &[u8]) -> String {
        let mut out = String::with_capacity(bytes.len().div_ceil(2) * 3);
        let digit = |value: usize| ALPHABET[value] as char;

        for pair in bytes.chunks(2) {
            match *pair {
                [high, low] => {
                    let value = usize::from(high) * 256 + usize::from(low);
                    out.extend([digit(value % 45), digit(value / 45 % 45), digit(value / 2025)]);
                }
                [one] => out.extend([digit(usize::from(one) % 45), digit(usize::from(one) / 45)]),
                _ => unreachable!("chunks of two"),
            }
        }

        out
    }

    pub fn decode(text: &str) -> Option<Vec<u8>> {
        let values: Vec<usize> = text.bytes().map(|byte| ALPHABET.iter().position(|letter| *letter == byte)).collect::<Option<_>>()?;
        let mut out = Vec::with_capacity(values.len() / 3 * 2 + 1);

        for group in values.chunks(3) {
            match *group {
                [c, d, e] => {
                    let value = c + d * 45 + e * 2025;
                    out.extend(u16::try_from(value).ok()?.to_be_bytes());
                }
                [c, d] => out.push(u8::try_from(c + d * 45).ok()?),
                _ => return None,
            }
        }

        Some(out)
    }
}

#[cfg(test)]
mod tests;
