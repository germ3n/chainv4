//! MetaMask, and the hardware wallets it drives, signing for the wallet in the browser. An account
//! of MetaMask's lives in a wallet beside the others with its public key alone; what it sends,
//! MetaMask signs as EIP-712 typed data, each field shown by name, bound to this chain by its
//! genesis id.
//!
//! MetaMask names an account by its address, not its key, so connecting has it sign a fixed
//! message once: the key that signature recovers to, checked against the address, is the
//! account's. Each signature it gives is recovered the same way and checked before it is sent.
//!
//! Only the browser reaches MetaMask, through the page's `window.ethereum`. The typed data is
//! built alike everywhere, which is what the tests check against MetaMask's own library.

use {
    chainv4::crypto::eip712::{self, Typed, Value},
    chainv4::crypto::keypair::PublicKey,
    chainv4::crypto::secp256k1,
    chainv4::structs::hash::Hash,
    chainv4::structs::hex::to_hex,
    chainv4::structs::tx::{FeeOffer, Tx},
    serde_json::{Map, Value as Json, json},
};

/// What connecting has MetaMask sign, once, so the wallet learns the account's public key.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub const CONNECT: &str = "Connect this account to the chainv4 wallet. This signs nothing that moves funds.";

/// What the wallet says for a batch from an account MetaMask holds: it never asks for that signature.
pub const NO_BATCH: &str = "MetaMask would show a batch as one opaque hash, not its steps, and the chain takes no batch signed that way. Send it from an account this wallet or the offline signer holds.";

/// EIP-191's hash of a message, as MetaMask's personal_sign signs it, and the wallet's own for sites.
pub fn personal_digest(message: &[u8]) -> [u8; 32] {
    let mut bytes = format!("\x19Ethereum Signed Message:\n{}", message.len()).into_bytes();
    bytes.extend_from_slice(message);

    secp256k1::keccak(&bytes)
}

/// A transaction's typed data for this chain, as eth_signTypedData_v4 takes it.
pub fn typed_data(tx: &Tx, nonce: u64, fee: &FeeOffer, genesis: &Hash) -> Json {
    let (domain, message) = (eip712::domain(genesis), eip712::message(tx, nonce, fee));
    let mut types = Map::new();
    types.insert(domain.name.to_string(), fields(&domain));
    types.insert(message.name.to_string(), fields(&message));

    json!({
        "types": types,
        "primaryType": message.name,
        "domain": values(&domain),
        "message": values(&message),
    })
}

fn fields(typed: &Typed) -> Json {
    typed.fields.iter().map(|(name, kind, _)| json!({ "name": name, "type": kind })).collect()
}

fn values(typed: &Typed) -> Json {
    typed
        .fields
        .iter()
        .map(|(name, _, value)| {
            let value = match value {
                Value::String(text) => json!(text),
                // Decimal text, as wide numbers are too wide for JSON's.
                Value::Uint(number) => json!(number.to_string()),
                Value::Bytes(bytes) => json!(to_hex(bytes)),
                Value::Bytes32(bytes) => json!(to_hex(bytes)),
                Value::Address(bytes) => json!(to_hex(bytes)),
                Value::Struct(inner) => values(inner),
            };

            (name.to_string(), value)
        })
        .collect::<Map<_, _>>()
        .into()
}

/// The public key that made a signature over a hash, if it is the account at this address, as
/// MetaMask writes it, with the signature as chainv4 takes it: r and s, s made low.
pub fn key_of(hash: &[u8; 32], signature: &[u8; 65], address: &str) -> Result<([u8; 33], [u8; 64]), String> {
    let (key, low) = secp256k1::recover(hash, signature).ok_or("MetaMask's signature recovers no key")?;
    let recovered = secp256k1::address(&key).map(|bytes| to_hex(&bytes)).unwrap_or_default();

    match recovered.eq_ignore_ascii_case(address) {
        true => Ok((key, low)),
        false => Err(format!("MetaMask signed with {recovered}, not {address}, the account asked")),
    }
}

/// A signature as MetaMask gives it: 0x and 65 bytes in hex, r, s and v.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn signature(text: &str) -> Result<[u8; 65], String> {
    chainv4::structs::hex::from_hex(text).and_then(|bytes| bytes.try_into().ok()).ok_or_else(|| "MetaMask sent a signature that does not read".to_string())
}

/// Whether this page reaches MetaMask, or another wallet that speaks as it does.
pub fn available() -> bool {
    #[cfg(target_arch = "wasm32")]
    return web::ethereum().is_some();

    #[cfg(not(target_arch = "wasm32"))]
    false
}

/// Asks MetaMask for an account, and its key, by having it sign CONNECT.
pub fn connect(done: impl FnOnce(Result<PublicKey, String>) + 'static) {
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(async move { done(web::connect().await) });

    #[cfg(not(target_arch = "wasm32"))]
    done(Err("MetaMask signs in the browser alone".to_string()));
}

/// Asks MetaMask to sign typed data with the account at this address.
pub fn sign(address: String, typed: Json, done: impl FnOnce(Result<[u8; 65], String>) + 'static) {
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(async move { done(web::sign(&address, &typed).await) });

    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (address, typed);
        done(Err("MetaMask signs in the browser alone".to_string()));
    }
}

#[cfg(target_arch = "wasm32")]
mod web {
    use {
        super::{CONNECT, key_of, personal_digest, signature},
        chainv4::crypto::keypair::PublicKey,
        chainv4::structs::hex::to_hex,
        js_sys::{Array, Function, Object, Promise, Reflect},
        serde_json::Value as Json,
        wasm_bindgen::{JsCast, JsValue},
    };

    /// The page's window.ethereum, when a wallet put one there.
    pub fn ethereum() -> Option<JsValue> {
        Reflect::get(&js_sys::global(), &"ethereum".into()).ok().filter(|ethereum| !ethereum.is_undefined() && !ethereum.is_null())
    }

    /// What MetaMask says went wrong, as its errors carry a message.
    fn why(err: JsValue) -> String {
        Reflect::get(&err, &"message".into()).ok().and_then(|message| message.as_string()).unwrap_or_else(|| "MetaMask refused".to_string())
    }

    /// One request to window.ethereum, answered.
    async fn request(method: &str, params: Array) -> Result<JsValue, String> {
        let ethereum = ethereum().ok_or("no MetaMask in this browser")?;
        let call: Function = Reflect::get(&ethereum, &"request".into()).map_err(why)?.dyn_into().map_err(|_| "window.ethereum has no request".to_string())?;
        let asked = Object::new();
        Reflect::set(&asked, &"method".into(), &method.into()).map_err(why)?;
        Reflect::set(&asked, &"params".into(), &params).map_err(why)?;
        let promise: Promise = call.call1(&ethereum, &asked).map_err(why)?.dyn_into().map_err(|_| "MetaMask did not answer".to_string())?;

        wasm_bindgen_futures::JsFuture::from(promise).await.map_err(why)
    }

    pub async fn connect() -> Result<PublicKey, String> {
        let accounts = request("eth_requestAccounts", Array::new()).await?;
        let address = Array::from(&accounts).get(0).as_string().ok_or("MetaMask gave no account")?;
        let message = to_hex(CONNECT.as_bytes());
        let signed = request("personal_sign", Array::of2(&message.into(), &address.clone().into())).await?;
        let signed = signature(&signed.as_string().ok_or("MetaMask sent no signature")?)?;
        let (key, _) = key_of(&personal_digest(CONNECT.as_bytes()), &signed, &address)?;

        Ok(PublicKey::Secp256k1(key))
    }

    pub async fn sign(address: &str, typed: &Json) -> Result<[u8; 65], String> {
        let signed = request("eth_signTypedData_v4", Array::of2(&address.into(), &typed.to_string().into())).await?;

        signature(&signed.as_string().ok_or("MetaMask sent no signature")?)
    }
}

#[cfg(test)]
mod tests;
