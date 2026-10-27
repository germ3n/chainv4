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
//! Each is a Uniform Resource, as MetaMask's and Keystone's codes are: `ur:chainv4-pairing`,
//! `ur:chainv4-sign-request` or `ur:chainv4-signed`, carrying the message's encoding, the chain's
//! own, in bytewords, and written in capitals, which a QR code packs densest. Bytes that are not
//! exactly one message are refused.
//!
//! A message too long for one code goes as fountain-coded parts that the screen shows one after
//! another: any enough of them, read in any order, make the message, so a camera that missed one
//! needs no full round for it.

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

/// The UR types of the three messages.
pub const PAIR_TYPE: &str = "chainv4-pairing";
pub const SIGN_TYPE: &str = "chainv4-sign-request";
pub const SIGNED_TYPE: &str = "chainv4-signed";

/// The most bytes of a message one code holds: as dense as a code a phone reads off a screen
/// reliably, about 620 characters, as many as a token transfer's request takes. Longer ones
/// animate, a part this long at a time.
pub const FRAGMENT: usize = 300;

/// The widths a photo is read at, the first that finds a code winning: a phone's camera takes
/// thousands of pixels across, more than a code needs and slow to search, so smaller first.
const SEARCH_WIDTHS: [u32; 3] = [1_200, 2_000, 800];

/// An account's public keys, one per scheme it has an address in, as the signer shows them, and
/// where its words derive it, when they do.
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct Pairing {
    pub name: String,
    pub keys: Vec<PublicKey>,
    pub origin: Option<Origin>,
}

/// Where the signer's words derive an account: its number, and the fingerprint of the seed. What
/// a request in Ethereum's own codes names the key by, m/44'/60'/0'/0/number under the
/// fingerprint, for a site's send or signature through the browser extension.
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaWrite, SchemaRead, serde::Serialize, serde::Deserialize)]
pub struct Origin {
    pub account: u32,
    pub fingerprint: [u8; 4],
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

/// One of the three messages, a request MetaMask shows for its QR hardware wallet to sign, or a
/// PSBT a Bitcoin wallet shows, asking or signed.
#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    Pair(Pairing),
    Sign(Request),
    Signed(SignedTx),
    MetaMask(Box<crate::hardware::SignRequest>),
    /// The signer's answer to one: what the browser extension reads back for a site.
    EthSignature(Box<crate::hardware::Signature>),
    Bitcoin(Box<crate::bitcoin::Request>),
    /// A swap's terms: an offer, or its answer.
    #[cfg(feature = "swap")]
    Swap(Box<chainv4::swap::Terms>),
}

impl Message {
    /// Its UR type.
    pub fn ur_type(&self) -> &'static str {
        match self {
            Message::Pair(_) => PAIR_TYPE,
            Message::Sign(_) => SIGN_TYPE,
            Message::Signed(_) => SIGNED_TYPE,
            Message::MetaMask(_) => crate::hardware::REQUEST_TYPE,
            Message::EthSignature(_) => crate::hardware::SIGNATURE_TYPE,
            Message::Bitcoin(request) => request.kind,
            #[cfg(feature = "swap")]
            Message::Swap(_) => chainv4::swap::UR_TYPE,
        }
    }

    /// Its encoding: the chain's own, or for MetaMask's request and a PSBT, their CBOR.
    pub fn bytes(&self) -> Vec<u8> {
        match self {
            Message::Pair(pairing) => wincode::serialize(pairing),
            Message::Sign(request) => wincode::serialize(request),
            Message::Signed(signed) => wincode::serialize(signed),
            Message::MetaMask(request) => return request.to_cbor().expect("a request read always writes back"),
            Message::EthSignature(signature) => return signature.to_cbor().expect("a signature always writes"),
            Message::Bitcoin(request) => return request.to_cbor(),
            #[cfg(feature = "swap")]
            Message::Swap(terms) => return terms.to_bytes(),
        }
        .expect("messages always encode")
    }

    /// Its text, whole, in one UR: what one code holds when it fits, and what Copy as text gives.
    pub fn to_text(&self) -> String {
        crate::hardware::ur(self.ur_type(), &self.bytes())
    }

    /// Reads a message from a UR's type and bytes, whole: anything past its encoding is refused.
    pub fn from_ur(kind: &str, bytes: &[u8]) -> Result<Message, String> {
        match kind {
            PAIR_TYPE => whole::<Pairing>(bytes).map(Message::Pair),
            SIGN_TYPE => whole::<Request>(bytes).map(Message::Sign),
            SIGNED_TYPE => whole::<SignedTx>(bytes).map(Message::Signed),
            crate::hardware::REQUEST_TYPE => crate::hardware::SignRequest::from_cbor(bytes).map(|request| Message::MetaMask(Box::new(request))),
            crate::hardware::SIGNATURE_TYPE => crate::hardware::Signature::from_cbor(bytes).map(|signature| Message::EthSignature(Box::new(signature))),
            #[cfg(feature = "swap")]
            chainv4::swap::UR_TYPE => chainv4::swap::Terms::from_bytes(bytes).map(|terms| Message::Swap(Box::new(terms))),
            crate::bitcoin::PSBT_TYPE => crate::bitcoin::Request::from_cbor(crate::bitcoin::PSBT_TYPE, bytes).map(|request| Message::Bitcoin(Box::new(request))),
            crate::bitcoin::NEW_PSBT_TYPE => crate::bitcoin::Request::from_cbor(crate::bitcoin::NEW_PSBT_TYPE, bytes).map(|request| Message::Bitcoin(Box::new(request))),
            other => Err(format!("a {other} code, which this wallet does not read")),
        }
    }

    /// Reads a message from its text, as Copy as text gave it: one UR, whole. Spaces around it go,
    /// as pasting brings them. The wallet reads text through Scan, which takes parts too.
    #[cfg(test)]
    pub fn from_text(text: &str) -> Result<Message, String> {
        let text = text.trim();

        if !is_ur(text) {
            return Err("not a code of this wallet's, MetaMask's or a Bitcoin wallet's: those start ur:".to_string());
        }

        let kind = text[3..].split('/').next().unwrap_or_default().to_ascii_lowercase();

        match ur::decode(text).map_err(|err| format!("a code that does not read: {err:?}"))? {
            (ur::ur::Kind::SinglePart, bytes) => Message::from_ur(&kind, &bytes),
            (ur::ur::Kind::MultiPart, _) => Err("one part of an animated code: read it with the camera, which takes the others as they come".to_string()),
        }
    }

    /// What it is, in a word or two, for saying what came instead of what was wanted.
    pub fn name(&self) -> &'static str {
        match self {
            Message::Pair(_) => "an account to pair with",
            Message::Sign(_) => "a request to sign",
            Message::Signed(_) => "a signed transaction",
            Message::MetaMask(_) => "a request from MetaMask",
            Message::EthSignature(_) => "an Ethereum signature from the signer",
            Message::Bitcoin(_) => "a Bitcoin wallet's PSBT",
            #[cfg(feature = "swap")]
            Message::Swap(_) => "a swap's terms",
        }
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

/// Reads a message from a file: its text, or a photo or image of its QR code.
#[cfg(test)]
pub fn read(bytes: &[u8]) -> Result<Message, String> {
    Message::from_text(&text_in(bytes)?)
}

/// Whether a code's text is a Uniform Resource, as MetaMask's are: ur:, in either case.
pub fn is_ur(text: &str) -> bool {
    text.trim().get(..3).is_some_and(|scheme| scheme.eq_ignore_ascii_case("ur:"))
}

/// The text of a code in a file: the file's own, when it is a code's text, or the QR code's in a
/// photo or image of one, this wallet's or MetaMask's.
pub fn text_in(bytes: &[u8]) -> Result<String, String> {
    match std::str::from_utf8(bytes) {
        Ok(text) if is_ur(text) => Ok(text.trim().to_string()),
        _ => code_in(bytes),
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
                Ok((_, text)) if is_ur(&text) => return Ok(text),
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
        PublicKey::Bitcoin(_) => Signature::Bitcoin([0; 64]),
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

#[cfg(test)]
mod tests;
