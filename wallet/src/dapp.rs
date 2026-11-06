//! Sites asking the wallet, through the browser extension's window.ethereum: to see its Ethereum
//! accounts, to send a transaction, to sign a message or typed data. The extension's background
//! keeps which sites may see which accounts and answers their reads from the node; what needs a
//! key comes here, through window.chainv4Dapp, for the wallet to show and approve. Only accounts
//! this wallet holds a secp256k1 key for are offered: Ethereum's tools know no other.
//!
//! Requests are read, and signatures made, alike everywhere, which is what the tests check; only
//! the browser has the extension to bring them.

use {
    chainv4::crypto::eip712::typed::TypedData,
    chainv4::crypto::secp256k1,
    chainv4::structs::hex::{from_hex, to_hex},
    chainv4::structs::u256::u256,
    serde_json::{Value, json},
};

/// A site's request, as the background hands it on.
#[derive(Clone, Debug)]
pub struct Request {
    pub id: u64,
    /// The site, as the browser names it: https://app.example.
    pub origin: String,
    pub asked: Asked,
}

/// What a site asks for.
#[derive(Clone, Debug)]
pub enum Asked {
    /// To see accounts: eth_requestAccounts, or wallet_requestPermissions.
    Connect,
    /// To send a transaction: eth_sendTransaction.
    Send(Box<Wanted>),
    /// To sign a message: personal_sign, as EIP-191 has it.
    Message { address: [u8; 20], message: Vec<u8> },
    /// To sign typed data: eth_signTypedData_v4, as EIP-712 has it.
    Typed { address: [u8; 20], data: Box<TypedData>, json: Value },
    /// To follow another chain: wallet_switchEthereumChain, or wallet_addEthereumChain, by its
    /// chain id. The wallet switches only to one it has followed before, at its node.
    SwitchChain { chain_id: u64 },
    /// To show a token: wallet_watchAsset, as EIP-747 has it.
    WatchAsset(Box<Watched>),
}

/// A token a site asks the wallet to show, on this chain's EVM.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Watched {
    pub kind: AssetKind,
    /// The contract, as 0x and 40 hex digits.
    pub address: String,
    pub symbol: String,
    pub decimals: u8,
    /// An NFT's or multi-token's id, in decimal.
    #[serde(default)]
    pub token_id: Option<String>,
    /// The picture the site names. Only ever fetched when Settings lets the wallet fetch pictures.
    #[serde(default)]
    pub image: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AssetKind {
    Erc20,
    Erc721,
    Erc1155,
}

impl AssetKind {
    pub fn name(self) -> &'static str {
        match self {
            AssetKind::Erc20 => "ERC-20",
            AssetKind::Erc721 => "ERC-721",
            AssetKind::Erc1155 => "ERC-1155",
        }
    }
}

/// A watchAsset's params, as EIP-747 writes them: { type, options }, or that in a list as a few
/// sites send it.
fn watched(params: &Value) -> Result<Watched, String> {
    let params = if params.is_array() { &params[0] } else { params };
    let options = &params["options"];
    let kind = match params["type"].as_str().unwrap_or_default().to_ascii_uppercase().as_str() {
        "ERC20" => AssetKind::Erc20,
        "ERC721" => AssetKind::Erc721,
        "ERC1155" => AssetKind::Erc1155,
        other => return Err(format!("{other} is not a kind of token the wallet shows: ERC20, ERC721 or ERC1155")),
    };
    let contract = address(&options["address"]).ok_or("the token's address is 0x and 40 hex digits")?;
    let symbol = options["symbol"].as_str().unwrap_or_default().trim().to_string();

    if symbol.chars().count() > 11 || symbol.chars().any(char::is_control) {
        return Err("a symbol is at most 11 characters".to_string());
    }

    let decimals = match &options["decimals"] {
        Value::Null => if kind == AssetKind::Erc20 { 18 } else { 0 },
        Value::Number(number) => number.as_u64().filter(|decimals| *decimals <= 36).ok_or("decimals are 0 to 36")? as u8,
        Value::String(text) => text.parse().ok().filter(|decimals| *decimals <= 36).ok_or("decimals are 0 to 36")?,
        _ => return Err("decimals are 0 to 36".to_string()),
    };
    let token_id = match (&options["tokenId"], kind) {
        (Value::Null, AssetKind::Erc20) => None,
        (Value::Null, _) => return Err("an NFT is named by its tokenId".to_string()),
        (Value::String(text), _) if text.starts_with("0x") => Some(u256::from_str_radix(&text[2..], 16).map_err(|_| "a tokenId that is no number")?.to_string()),
        (Value::String(text), _) => Some(text.parse::<u256>().map_err(|_| "a tokenId that is no number")?.to_string()),
        (Value::Number(number), _) => Some(number.as_u64().ok_or("a tokenId that is no whole number")?.to_string()),
        _ => return Err("a tokenId that is no number".to_string()),
    };
    let image = options["image"].as_str().filter(|image| image.len() <= 2048).map(str::to_string);

    Ok(Watched { kind, address: to_hex(&contract), symbol, decimals, token_id, image })
}

/// A chain id as a site writes it: 0x and hex digits, or decimal.
fn chain_of(value: &Value) -> Option<u64> {
    match value {
        Value::String(text) => match text.strip_prefix("0x") {
            Some(hex) => u64::from_str_radix(hex, 16).ok(),
            None => text.parse().ok(),
        },
        Value::Number(number) => number.as_u64(),
        _ => None,
    }
}

/// A transaction as a site asks for it: whatever it left out, the wallet works out.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Wanted {
    pub from: [u8; 20],
    /// None to create a contract.
    pub to: Option<[u8; 20]>,
    pub value: u256,
    pub input: Vec<u8>,
    pub gas: Option<u64>,
    /// The most a gas may cost: maxFeePerGas, or gasPrice for a site that names that.
    pub max_price: Option<u256>,
    pub tip: Option<u256>,
    pub nonce: Option<u64>,
}

/// Why a request cannot be read: answered to the site as invalid params, -32602.
pub type Unread = (u64, String);

/// What a site hears: the result, or an error with its EIP-1193 code.
pub type Answer = Result<Value, (i64, String)>;

impl Request {
    /// Reads one as the background sends it: { id, origin, method, params }.
    pub fn read(json: &Value) -> Result<Request, Unread> {
        let id = json["id"].as_u64().ok_or((0, "a request has an id".to_string()))?;
        let bad = |why: &str| (id, why.to_string());
        let origin = json["origin"].as_str().unwrap_or("a site").to_string();
        let params = &json["params"];
        let asked = match json["method"].as_str().unwrap_or_default() {
            "eth_requestAccounts" | "wallet_requestPermissions" => Asked::Connect,
            "eth_sendTransaction" => Asked::Send(Box::new(wanted(&params[0]).map_err(|why| bad(&why))?)),
            "personal_sign" => {
                // [message, address], or the other way round, as a few sites send it.
                let (message, address) = match (address(&params[1]), address(&params[0])) {
                    (Some(address), _) => (&params[0], address),
                    (None, Some(address)) => (&params[1], address),
                    (None, None) => return Err(bad("personal_sign takes a message and an address")),
                };
                let message = match message.as_str() {
                    Some(hex) if hex.starts_with("0x") => from_hex(hex).ok_or_else(|| bad("the message is not hex"))?,
                    Some(text) => text.as_bytes().to_vec(),
                    None => return Err(bad("the message is text or hex")),
                };

                Asked::Message { address, message }
            }
            "eth_signTypedData_v4" => {
                let address = address(&params[0]).ok_or_else(|| bad("eth_signTypedData_v4 takes an address, then the data"))?;
                let data = TypedData::parse(&params[1]).map_err(|why| bad(&why))?;
                data.digest().map_err(|why| bad(&why))?;
                let json = match &params[1] {
                    Value::String(text) => serde_json::from_str(text).unwrap_or(Value::Null),
                    other => other.clone(),
                };

                Asked::Typed { address, data: Box::new(data), json }
            }
            "wallet_switchEthereumChain" | "wallet_addEthereumChain" => {
                Asked::SwitchChain { chain_id: chain_of(&params[0]["chainId"]).ok_or_else(|| bad("a chain is named by its chainId"))? }
            }
            "wallet_watchAsset" => Asked::WatchAsset(Box::new(watched(params).map_err(|why| bad(&why))?)),
            other => return Err(bad(&format!("the wallet is not asked {other}"))),
        };

        Ok(Request { id, origin, asked })
    }

    /// The account it names, for one that signs.
    pub fn signer(&self) -> Option<[u8; 20]> {
        match &self.asked {
            Asked::Connect | Asked::SwitchChain { .. } | Asked::WatchAsset(_) => None,
            Asked::Send(wanted) => Some(wanted.from),
            Asked::Message { address, .. } | Asked::Typed { address, .. } => Some(*address),
        }
    }
}

/// An Ethereum address, 0x and 40 hex digits, in any case.
fn address(value: &Value) -> Option<[u8; 20]> {
    let text = value.as_str()?;

    (text.len() == 42).then(|| from_hex(text))??.try_into().ok()
}

/// A quantity, as Ethereum's JSON-RPC writes it: 0x and hex digits.
fn quantity(value: &Value) -> Result<Option<u256>, String> {
    match value {
        Value::Null => Ok(None),
        Value::String(text) if text.starts_with("0x") => u256::from_str_radix(&text[2..], 16).map(Some).map_err(|_| format!("{text} is not a quantity")),
        other => Err(format!("{other} is not a quantity: 0x and hex digits")),
    }
}

fn wanted(json: &Value) -> Result<Wanted, String> {
    let from = address(&json["from"]).ok_or("a transaction has a from")?;
    let to = match &json["to"] {
        Value::Null => None,
        to => Some(address(to).ok_or("to is an address")?),
    };
    let input = match json.get("input").filter(|input| !input.is_null()).or(json.get("data")) {
        None | Some(Value::Null) => Vec::new(),
        Some(data) => data.as_str().and_then(from_hex).ok_or("the data is hex")?,
    };
    let small = |name: &str| -> Result<Option<u64>, String> { quantity(&json[name])?.map(|value| u64::try_from(value).map_err(|_| format!("{name} is too large"))).transpose() };

    if to.is_none() && input.is_empty() {
        return Err("a transaction with no to creates a contract, from its data".to_string());
    }

    Ok(Wanted {
        from,
        to,
        value: quantity(&json["value"])?.unwrap_or(u256::ZERO),
        input,
        gas: small("gas")?,
        max_price: quantity(&json["maxFeePerGas"])?.or(quantity(&json["gasPrice"])?),
        tip: quantity(&json["maxPriorityFeePerGas"])?,
        nonce: small("nonce")?,
    })
}

pub use crate::metamask::personal_digest;

/// A signature as Ethereum's tools take it from a wallet: r, s and v, 27 or 28, in hex.
pub fn signature(secret: &[u8; 32], digest: &[u8; 32]) -> String {
    let (signature, parity) = secp256k1::sign_recoverable(secret, digest);

    to_hex(&[signature.as_slice(), &[27 + parity]].concat())
}

/// What the site hears for a request the wallet turned down, as EIP-1193 says.
pub fn rejected() -> Answer {
    Err((4001, "User rejected the request.".to_string()))
}

/// What the wallet tells the extension of itself: its node, chain id, the Ethereum accounts it
/// holds keys for, and whether it is unlocked.
pub fn state(node: &str, chain_id: Option<u64>, accounts: &[[u8; 20]], unlocked: bool) -> Value {
    let mut state = json!({ "node": node, "unlocked": unlocked });

    if let Some(chain_id) = chain_id {
        state["chainId"] = json!(format!("{chain_id:#x}"));
    }

    // A locked wallet keeps the accounts it said, so a site connected stays so.
    if unlocked {
        state["accounts"] = json!(accounts.iter().map(|account| to_hex(account)).collect::<Vec<_>>());
    }

    state
}

/// The extension's side of it: there in the browser extension alone.
#[cfg(target_arch = "wasm32")]
mod glue {
    use {serde_json::Value, wasm_bindgen::JsValue};

    fn bridge() -> Option<JsValue> {
        let window = web_sys::window()?;
        let bridge = js_sys::Reflect::get(&window, &"chainv4Dapp".into()).ok()?;

        (!bridge.is_undefined() && !bridge.is_null()).then_some(bridge)
    }

    fn call(name: &str, args: &[JsValue]) -> Option<JsValue> {
        let bridge = bridge()?;
        let function: js_sys::Function = js_sys::Reflect::get(&bridge, &name.into()).ok()?.into();
        let args: js_sys::Array = args.iter().collect();

        function.apply(&bridge, &args).ok()
    }

    pub fn available() -> bool {
        bridge().is_some()
    }

    pub fn approving() -> bool {
        bridge().and_then(|bridge| js_sys::Reflect::get(&bridge, &"approving".into()).ok()).is_some_and(|value| value.is_truthy())
    }

    pub fn pending() -> Vec<Value> {
        call("pending", &[]).and_then(|json| json.as_string()).and_then(|json| serde_json::from_str(&json).ok()).unwrap_or_default()
    }

    pub fn answer(id: u64, json: &Value) {
        call("answer", &[JsValue::from_f64(id as f64), json.to_string().into()]);
    }

    pub fn report(state: &Value) {
        call("report", &[state.to_string().into()]);
    }

    pub fn sites() -> Value {
        call("sites", &[]).and_then(|json| json.as_string()).and_then(|json| serde_json::from_str(&json).ok()).unwrap_or(Value::Null)
    }

    pub fn let_sites(on: bool) {
        call("letSites", &[on.into()]);
    }

    pub fn disconnect(origin: &str) {
        call("disconnect", &[origin.into()]);
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod glue {
    use serde_json::Value;

    pub fn available() -> bool {
        false
    }

    pub fn approving() -> bool {
        false
    }

    pub fn pending() -> Vec<Value> {
        Vec::new()
    }

    pub fn answer(_: u64, _: &Value) {}

    pub fn report(_: &Value) {}

    pub fn sites() -> Value {
        Value::Null
    }

    pub fn let_sites(_: bool) {}

    pub fn disconnect(_: &str) {}
}

pub use glue::{approving, available, disconnect, let_sites, pending, report, sites};

/// Answers a request: its result, or an error with its code.
pub fn answer(id: u64, answer: Answer) {
    let json = match answer {
        Ok(result) => json!({ "result": result }),
        Err((code, message)) => json!({ "error": { "code": code, "message": message } }),
    };

    glue::answer(id, &json);
}

#[cfg(test)]
mod tests;
