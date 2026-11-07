//! Signing on a device that never goes online. One install is the signer: it holds the keys and
//! makes no requests at all. Another is the sender: it holds the signer's public keys alone, proves
//! their balances, builds what to send and gets it priced, and takes it to the node once signed.
//! Between them go three messages, each a QR code one device shows and the other photographs, or
//! the same text copied or in a file:
//! - **Pairing:** an account's public keys, from the signer, for the sender to watch.
//! - **A request:** the transaction, its nonce and fee offer, the key to sign with, how the sender's
//!   node names the coin and the token, for the signer to show what it signs, and the chain it is
//!   for, by its genesis id: the signer follows no chain of its own, and a signature holds on the
//!   one named alone, so it shows that too, and holds it against the chain it was told is its own.
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
    chainv4::structs::hash::{Hash, HashType},
    chainv4::registry::names::NameAction,
    chainv4::structs::tx::{FeeOffer, SignedTx, Tx, TxCallArgumentType},
    chainv4::structs::u256::u256,
    wincode::{SchemaRead, SchemaWrite},
};

/// The UR types of the three messages.
pub const PAIR_TYPE: &str = "chainv4-pairing";
pub const SIGN_TYPE: &str = "chainv4-sign-request";
pub const SIGNED_TYPE: &str = "chainv4-signed";
/// A multisig's transaction on its way round the members: the same bundle chainv4 multisig
/// passes as text, read by the signer to add its member's signature, and shown back.
pub const MULTISIG_TYPE: &str = "chainv4-multisig";
/// A multisig's call on an EVM chain, for the signer to sign as one of its contract's owners, or
/// as an owner of another multisig that owns it: asked and answered as the same request, the
/// answer's bundle holding the signature.
pub const EVM_TYPE: &str = "chainv4-multisig-evm";

/// A token's signed authorization for the offline signer to sign, and signed: the terms, as JSON
/// text, and its answer, as the text that is handed on.
pub const AUTHORIZE_TYPE: &str = "chainv4-authorize";
pub const AUTHORIZED_TYPE: &str = "chainv4-authorized";

/// The most bytes of a message one code holds, about 700 characters: as many as a token transfer's
/// request takes, the chain it names included. A code that stays still may be denser than one of
/// those that pass by, each of which a phone has a moment to read.
pub const ONE_CODE: usize = 340;

/// The most bytes of a message each of a longer one's parts holds, as they animate: as dense as a
/// code a phone reads off a screen reliably as it goes by, about 620 characters.
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
    /// The chain it is for, by its genesis id: the sender's word, which a hostile sender may set to
    /// another chain's, so the signer shows it. Last, so a request from before there was one stops
    /// short of it, and one read by a signer from before has bytes left over: refused either way,
    /// with nothing read out of place.
    pub genesis: Hash,
    /// The NFT or multi-token collection a call goes to, as the sender's registry names it: the
    /// sender's word, as a token is. Not in the encoding above: written after it, and only when
    /// there is one, so a request without one reads in a signer from before as it always did, and
    /// one with one is refused there, having bytes left over.
    #[wincode(skip)]
    pub collection: Option<NamedCollection>,
}

/// A collection as a request names it for the signer.
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct NamedCollection {
    pub name: String,
    pub symbol: String,
    /// Whether it is a multi-token collection, whose transfers carry an amount, rather than an
    /// NFT collection.
    pub multi: bool,
}

/// A request from its bytes: its encoding, then, if anything is left, the collection it names,
/// whole.
fn request(bytes: &[u8]) -> Result<Request, String> {
    let mut request: Request = wincode::deserialize(bytes).map_err(|_| "a code that does not decode".to_string())?;
    let body = wincode::serialize(&request).map_err(|_| "a code that does not decode".to_string())?;
    let rest = bytes.strip_prefix(body.as_slice()).ok_or("a code that does not decode")?;

    if !rest.is_empty() {
        request.collection = Some(whole::<NamedCollection>(rest).map_err(|_| "a code with more in it than it should hold".to_string())?);
    }

    Ok(request)
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
    /// A multisig's transaction and the members' signatures so far: to sign, and signed.
    Multisig(Box<chainv4::registry::multisig::MultisigBundle>),
    /// A multisig's call on an EVM chain, with the key to sign it with: to sign, and signed.
    Evm(Box<chainv4::abi::multisig::EvmRequest>),
    /// A token's allowance or transfer to sign off the chain, as the Authorize page prepared it.
    Authorize(Box<AuthorizeRequest>),
    /// The signer's answer to one: the authorization, signed, to hand on.
    Authorized(Box<chainv4::registry::tokens::authorization::SignedAuthorization>),
}

/// A token's authorization for the signer to sign: its terms, the chain whose id its text names,
/// and the token as the sender names it, which is the sender's word, as a transfer's token is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthorizeRequest {
    pub authorization: chainv4::registry::tokens::authorization::TokenAuthorization,
    pub chain_id: u64,
    pub token: Option<Shown>,
}

impl AuthorizeRequest {
    /// Its encoding: the terms as JSON, with the chain's id and the token's symbol and decimals.
    fn to_bytes(&self) -> Vec<u8> {
        let mut value = self.authorization.to_json();
        value["chain_id"] = self.chain_id.into();

        if let Some(token) = &self.token {
            value["symbol"] = token.symbol.clone().into();
            value["decimals"] = token.decimals.into();
        }

        value.to_string().into_bytes()
    }

    fn from_bytes(bytes: &[u8]) -> Result<AuthorizeRequest, String> {
        let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| "a code that does not decode".to_string())?;
        let authorization = chainv4::registry::tokens::authorization::TokenAuthorization::from_json(&value)?;
        let chain_id = value["chain_id"].as_u64().ok_or("a request that names no chain")?;
        let token = match (value["symbol"].as_str(), value["decimals"].as_u64().and_then(|decimals| u8::try_from(decimals).ok())) {
            (Some(symbol), Some(decimals)) => Some(Shown { symbol: symbol.to_string(), decimals }),
            _ => None,
        };

        Ok(AuthorizeRequest { authorization, chain_id, token })
    }
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
            Message::Multisig(_) => MULTISIG_TYPE,
            Message::Evm(_) => EVM_TYPE,
            Message::Authorize(_) => AUTHORIZE_TYPE,
            Message::Authorized(_) => AUTHORIZED_TYPE,
        }
    }

    /// Its encoding: the chain's own, or for MetaMask's request and a PSBT, their CBOR.
    pub fn bytes(&self) -> Vec<u8> {
        match self {
            Message::Pair(pairing) => wincode::serialize(pairing),
            Message::Sign(request) => wincode::serialize(request).map(|mut bytes| {
                if let Some(collection) = &request.collection {
                    bytes.extend(wincode::serialize(collection).expect("a collection always encodes"));
                }

                bytes
            }),
            Message::Signed(signed) => wincode::serialize(signed),
            Message::Multisig(bundle) => wincode::serialize(bundle),
            Message::Evm(request) => wincode::serialize(request),
            Message::MetaMask(request) => return request.to_cbor().expect("a request read always writes back"),
            Message::EthSignature(signature) => return signature.to_cbor().expect("a signature always writes"),
            Message::Bitcoin(request) => return request.to_cbor(),
            #[cfg(feature = "swap")]
            Message::Swap(terms) => return terms.to_bytes(),
            Message::Authorize(request) => return request.to_bytes(),
            Message::Authorized(signed) => return signed.to_text().into_bytes(),
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
            SIGN_TYPE => request(bytes).map(Message::Sign),
            SIGNED_TYPE => whole::<SignedTx>(bytes).map(Message::Signed),
            MULTISIG_TYPE => whole::<chainv4::registry::multisig::MultisigBundle>(bytes).map(|bundle| Message::Multisig(Box::new(bundle))),
            EVM_TYPE => whole::<chainv4::abi::multisig::EvmRequest>(bytes).map(|request| Message::Evm(Box::new(request))),
            AUTHORIZE_TYPE => AuthorizeRequest::from_bytes(bytes).map(|request| Message::Authorize(Box::new(request))),
            AUTHORIZED_TYPE => std::str::from_utf8(bytes)
                .map_err(|_| "a code that does not decode".to_string())
                .and_then(chainv4::registry::tokens::authorization::SignedAuthorization::from_text)
                .map(|signed| Message::Authorized(Box::new(signed))),
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
            Message::Multisig(_) => "a multisig's transaction",
            Message::Evm(_) => "a multisig's call on an EVM chain",
            Message::Authorize(_) => "a token's authorization to sign",
            Message::Authorized(_) => "a signed authorization",
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

/// Why a signer told which chain is its own will not sign for this one, if it will not: the same id
/// Settings takes, and MetaMask's requests name, from the genesis id. Told none, it only shows it.
pub fn wrong_chain(genesis: &Hash, yours: Option<u64>) -> Option<String> {
    let chain = chainv4::crypto::ethereum::chain_id(genesis);

    yours.filter(|yours| *yours != chain).map(|yours| format!("This is for chain {chain}, not yours, {yours}: a signature here counts on chain {chain} alone, so this signer will not make one."))
}

/// What the signer holds a request against before it signs: the chain it names is the one this
/// signer was told is its own, if it was told one; the key it names is one of its; and the
/// transaction is one the chain could take from that key.
pub fn sign(request: &Request, key: &Keypair, yours: Option<u64>) -> Result<SignedTx, String> {
    if let Some(why) = wrong_chain(&request.genesis, yours) {
        return Err(why);
    }

    if key.public() != request.signer {
        return Err("that key is not the one the request names".to_string());
    }

    let signed = SignedTx::sign(request.tx.clone(), request.nonce, request.fee, key, &request.genesis);
    signed.verify_on(&request.genesis).map_err(|err| format!("the chain would refuse this transaction: {err}"))?;

    Ok(signed)
}

/// Whether a signed transaction is exactly the one asked for, and its signature holds.
pub fn check(request: &Request, signed: &SignedTx) -> Result<(), String> {
    let same = signed.tx == request.tx && signed.nonce == request.nonce && signed.fee == request.fee && signed.signer == request.signer;

    if !same {
        return Err("that is not the transaction this wallet asked to sign".to_string());
    }

    signed.verify_on(&request.genesis).map_err(|err| format!("its signature does not hold: {err}"))
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
        functions: Vec::new(),
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
    /// Each step, for a batch, or a member's approval of one.
    pub steps: Option<crate::steps::Steps>,
    /// The chain a signature counts on: its id, as Settings and MetaMask's requests name it.
    pub chain: u64,
}

/// Lays a request out for the signer to read.
pub fn describe(request: &Request) -> Described {
    // The coin's and a token's symbols are the node's and the site's word, so they are cut short
    // and escaped before they reach a headline, as the step frame does with everything it shows.
    let coin = &Shown { symbol: crate::steps::plain(&request.coin.symbol), decimals: request.coin.decimals };
    // Saturating, as these are the sender's word: a sum past what fits must not panic the signer,
    // nor, in a build that wraps, show a small fee for a large one.
    let most = u256::from(request.fee.gas_limit).saturating_mul(request.fee.max_price);
    let shown = |raw: u256, asset: &Shown| (format::amount(&raw.to_string(), asset.decimals, &asset.symbol), raw.to_string());
    let (kind, what, to, token) = match &request.tx {
        Tx::Transfer { to, amount, .. } => (format!("Send {}", coin.symbol), Some(shown(amount.to_u256(), coin)), Some(to.to_string()), None),
        // An NFT's, or a multi-token amount's, as the sender names the collection.
        Tx::ContractCall { to: contract, func, amount, data } if amount.to_u256() == u256::ZERO && request.collection.is_some() && sends_from_collection(func, data, request.collection.as_ref()).is_some() => {
            let collection = request.collection.as_ref().expect("looked");
            let (recipient, id, units) = sends_from_collection(func, data, Some(collection)).expect("looked");
            let symbol = crate::steps::plain(&collection.symbol);
            let what = match units {
                Some(units) => (format!("{units} of {symbol} #{id}"), units.to_string()),
                None => (format!("{symbol} #{id}"), "1".to_string()),
            };

            (format!("Send {} from {}", if units.is_some() { "items" } else { "an NFT" }, crate::steps::plain(&collection.name)), Some(what), Some(recipient.to_string()), Some(contract.to_string()))
        }
        // An EVM collection's, as the NFTs page sends one, by safeTransferFrom or safeBatchTransferFrom.
        Tx::EvmCall { to: contract, value, input } if value.to_u256() == u256::ZERO && request.collection.as_ref().and_then(|collection| sends_from_evm_collection(input, collection)).is_some() => {
            let collection = request.collection.as_ref().expect("looked");
            let (recipient, items) = sends_from_evm_collection(input, collection).expect("looked");
            let symbol = crate::steps::plain(&collection.symbol);
            let what = match items.as_slice() {
                [(id, None)] => (format!("{symbol} #{id}"), "1".to_string()),
                [(id, Some(units))] => (format!("{units} of {symbol} #{id}"), units.to_string()),
                many => {
                    let listed: Vec<String> = many.iter().take(5).map(|(id, units)| format!("{} of #{id}", units.unwrap_or(u256::ONE))).collect();
                    let more = if many.len() > 5 { format!(" and {} more", many.len() - 5) } else { String::new() };
                    let total = many.iter().fold(u256::ZERO, |sum, (_, units)| sum.saturating_add(units.unwrap_or(u256::ONE)));

                    (format!("{symbol}: {}{more}", listed.join(", ")), total.to_string())
                }
            };

            (format!("Send {} from {}", if collection.multi { "items" } else { "an NFT" }, crate::steps::plain(&collection.name)), Some(what), Some(chainv4::structs::address::Address::Evm(recipient).to_string()), Some(contract.to_string()))
        }
        // A token's transfer only as the sender names its token: the same call to an NFT collection
        // sends an id, which no amount is to be read from.
        Tx::ContractCall { to: contract, func, amount, data } if is_token_transfer(func, amount.to_u256(), data) && request.token.is_some() => {
            let ([TxCallArgumentType::Address(recipient), TxCallArgumentType::U256(units)], Some(asset)) = (data.as_slice(), &request.token) else {
                unreachable!("is_token_transfer and the token looked")
            };
            let asset = Shown { symbol: crate::steps::plain(&asset.symbol), decimals: asset.decimals };

            (format!("Send {}", asset.symbol), Some(shown(*units, &asset)), Some(recipient.to_string()), Some(contract.to_string()))
        }
        Tx::ContractCall { to: contract, .. } => ("Call a contract".to_string(), None, Some(contract.to_string()), None),
        Tx::Name { name, action } => (name_kind(action), None, Some(name_detail(name, action)), None),
        Tx::PrimaryName { name: Some(name) } => ("Choose a primary name".to_string(), None, Some(name.clone()), None),
        Tx::PrimaryName { name: None } => ("Clear the primary name".to_string(), None, None, None),
        Tx::Claim { to, .. } => ("Claim mining pool earnings".to_string(), None, Some(to.to_string()), None),
        Tx::SetDirectory { .. } => ("Set the chain's directory".to_string(), None, None, None),
        Tx::HandDirectory { to: Some(to) } => ("Hand the directory's admin role on".to_string(), None, Some(to.to_string()), None),
        Tx::HandDirectory { to: None } => ("Renounce the directory's admin role, for good".to_string(), None, None, None),
        // A token the sender could only name for one: in a batch, its whole units as signed.
        Tx::Batch { steps } => {
            let laid = crate::steps::of(steps, &request.coin, &|_| crate::steps::Known::Unknown, None);

            (format!("A batch of {} step{}", steps.len(), if steps.len() == 1 { "" } else { "s" }), Some(laid.total), None, None)
        }
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
        steps: request.tx.batch_steps().map(|steps| crate::steps::of(steps, &request.coin, &|_| crate::steps::Known::Unknown, None)),
        chain: chainv4::crypto::ethereum::chain_id(&request.genesis),
    }
}

/// A single, non-batch transaction laid out as a batch's step is, from the request's own coin and
/// token, which is all an offline signer has to name them by. So a lone contract or EVM call shows
/// its amount, its recipient and its data with the raw selector and size, as a batch's step does,
/// rather than a one-line summary that leaves them out. The caller lays a batch out with
/// `steps::of` on its own steps instead.
pub fn laid(request: &Request) -> crate::steps::Steps {
    let known = match &request.token {
        Some(token) => crate::steps::Known::Token(token.clone()),
        None => crate::steps::Known::Unknown,
    };

    crate::steps::of(std::slice::from_ref(&request.tx), &request.coin, &|_| known.clone(), None)
}

/// Why the chain would refuse a batch a request or a bundle holds, if it would: said as soon as it
/// is read, as a batch that could never land is no one's to sign. What it is sent as is checked with
/// it, for the steps that say whom they take from.
pub fn refused(request: &Request) -> Option<String> {
    request.tx.batch_steps()?;

    let signed = probe(request.tx.clone(), request.nonce, request.signer);
    let held = signed.batch_problem().or_else(|| signed.tx.verify().err());

    held.map(|why| format!("The chain would refuse this batch: {why}."))
}

/// What a name transaction does, in a few words.
pub(crate) fn name_kind(action: &NameAction) -> String {
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
pub(crate) fn name_detail(name: &str, action: &NameAction) -> String {
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

/// Whom an NFT or multi-token transfer, as the NFTs page makes one, sends to, which id, and for a
/// multi-token one how many: transfer or safe_transfer, to, id and amount.
fn sends_from_collection(func: &chainv4::structs::hash::Hash, data: &[TxCallArgumentType], collection: Option<&NamedCollection>) -> Option<(chainv4::structs::address::Address, u256, Option<u256>)> {
    let collection = collection?;
    let named = [b"transfer".as_slice(), b"safe_transfer".as_slice()].iter().any(|name| *func == Hasher::digest(HashType::Xxh3_64, name));

    match (named, collection.multi, data) {
        (true, false, [TxCallArgumentType::Address(to), TxCallArgumentType::U256(id)]) => Some((*to, *id, None)),
        (true, true, [TxCallArgumentType::Address(to), TxCallArgumentType::U256(id), TxCallArgumentType::U256(units)]) => Some((*to, *id, Some(*units))),
        _ => None,
    }
}

/// One id an EVM collection's transfer sends, and for an ERC-1155 how many of it.
type Sent = (u256, Option<u256>);

/// Whom an EVM collection's transfer, as the NFTs page makes one, sends to, and which ids, with how
/// many of each for an ERC-1155: safeTransferFrom of either standard, or safeBatchTransferFrom.
fn sends_from_evm_collection(input: &[u8], collection: &NamedCollection) -> Option<([u8; 20], Vec<Sent>)> {
    use chainv4::abi::{AbiType, AbiValue, decode_values};

    let selector = |signature: &str| chainv4::crypto::secp256k1::keccak(signature.as_bytes())[..4].to_vec();
    let (head, body) = (input.get(..4)?, input.get(4..)?);
    let uints = || AbiType::Array(Box::new(AbiType::Uint(256)));
    let number = |value: &AbiValue| match value {
        AbiValue::Uint(number) => Some(*number),
        _ => None,
    };
    let numbers = |value: &AbiValue| match value {
        AbiValue::Array(items) => items.iter().map(number).collect::<Option<Vec<u256>>>(),
        _ => None,
    };
    let to = |values: &[AbiValue]| match values.get(1) {
        Some(AbiValue::Address(to)) => Some(*to),
        _ => None,
    };

    match collection.multi {
        false if head == selector("safeTransferFrom(address,address,uint256)") => {
            let values = decode_values(&[AbiType::Address, AbiType::Address, AbiType::Uint(256)], body).ok()?;

            Some((to(&values)?, vec![(number(&values[2])?, None)]))
        }
        true if head == selector("safeTransferFrom(address,address,uint256,uint256,bytes)") => {
            let values = decode_values(&[AbiType::Address, AbiType::Address, AbiType::Uint(256), AbiType::Uint(256), AbiType::Bytes], body).ok()?;

            Some((to(&values)?, vec![(number(&values[2])?, Some(number(&values[3])?))]))
        }
        true if head == selector("safeBatchTransferFrom(address,address,uint256[],uint256[],bytes)") => {
            let values = decode_values(&[AbiType::Address, AbiType::Address, uints(), uints(), AbiType::Bytes], body).ok()?;
            let (ids, amounts) = (numbers(&values[2])?, numbers(&values[3])?);

            (ids.len() == amounts.len() && !ids.is_empty()).then(|| ids.into_iter().zip(amounts).map(|(id, units)| (id, Some(units))).collect()).map(|items| (to(&values).expect("decoded as an address"), items))
        }
        _ => None,
    }
}

/// Whether a call is the token standard's transfer, as the Send page makes one: to, and how much.
fn is_token_transfer(func: &chainv4::structs::hash::Hash, value: u256, data: &[TxCallArgumentType]) -> bool {
    *func == Hasher::digest(HashType::Xxh3_64, b"transfer")
        && value == u256::ZERO
        && matches!(data, [TxCallArgumentType::Address(_), TxCallArgumentType::U256(_)])
}

/// A transaction's kind, as its type names it: Bond, Deploy, and so on.
pub(crate) fn variant(tx: &Tx) -> String {
    let debug = format!("{tx:?}");

    debug.split([' ', '{', '(']).next().unwrap_or("A").to_string()
}

#[cfg(test)]
mod tests;
