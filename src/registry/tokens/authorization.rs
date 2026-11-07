//! Signed authorizations, as the token standard's optional functions take them
//! (docs/token-standard.md, "Signed authorizations"): an allowance an owner signs off the chain and
//! anyone sends, as EIP-2612's permit, and a transfer a relayer sends, as EIP-3009's. What the owner
//! signs is text the token's permit_message or transfer_message returns; this writes the same text
//! from the terms alone, so a wallet signs only what it can read back, and checks a signature
//! someone hands it before sending it.

use {
    crate::crypto::keypair::{Keypair, PublicKey, Signature},
    crate::crypto::hasher::Hasher,
    crate::structs::address::Address,
    crate::structs::amount::Amount,
    crate::structs::hash::HashType,
    crate::structs::hex::{from_hex, to_hex},
    crate::structs::tx::{Tx, TxCallArgumentType},
    crate::structs::u256::u256,
    r#macro::document,
    serde_json::{Value, json},
};

#[document(
    summary = "What an owner signs off the chain for a token: an allowance or a transfer, with the terms the token's message function writes.",
    category = "Registries",
    see_also = "SignedAuthorization",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TokenAuthorization {
    #[document(summary = "An allowance, as permit sets it: spender may move up to amount of owner's tokens, at owner's next permit nonce, until deadline, in milliseconds.")]
    Permit { token: Address, owner: Address, spender: Address, amount: u256, nonce: u256, deadline: u64 },
    #[document(summary = "A transfer, as transfer_with_authorization makes it: amount from from to to, after valid_after and before valid_before, in milliseconds, once, by the 32-byte nonce from picks.")]
    Transfer { token: Address, from: Address, to: Address, amount: u256, valid_after: u64, valid_before: u64, nonce: [u8; 32] },
}

impl TokenAuthorization {
    #[document(parent = "TokenAuthorization", summary = "The token it is for.")]
    pub fn token(&self) -> Address {
        match self {
            TokenAuthorization::Permit { token, .. } | TokenAuthorization::Transfer { token, .. } => *token,
        }
    }

    #[document(parent = "TokenAuthorization", summary = "Who signs it: the owner, or the transfer's from.")]
    pub fn owner(&self) -> Address {
        match self {
            TokenAuthorization::Permit { owner, .. } => *owner,
            TokenAuthorization::Transfer { from, .. } => *from,
        }
    }

    #[document(
        parent = "TokenAuthorization",
        summary = "The text it signs, as the reference tokens write it, line by line: what permit_message or transfer_message returns on the chain with this id.",
        params = { chain_id = "The chain's id, as chain.id() and the EVM's CHAINID give it." },
    )]
    pub fn message(&self, chain_id: u64) -> String {
        match self {
            TokenAuthorization::Permit { token, owner, spender, amount, nonce, deadline } => {
                format!("chainv4 token permit\nchain: {chain_id}\ntoken: {token}\nowner: {owner}\nspender: {spender}\namount: {amount}\nnonce: {nonce}\ndeadline: {deadline}")
            }
            TokenAuthorization::Transfer { token, from, to, amount, valid_after, valid_before, nonce } => format!(
                "chainv4 token transfer\nchain: {chain_id}\ntoken: {token}\nfrom: {from}\nto: {to}\namount: {amount}\nvalid after: {valid_after}\nvalid before: {valid_before}\nnonce: {}",
                to_hex(nonce)
            ),
        }
    }

    #[document(parent = "TokenAuthorization", summary = "The token's function that writes its text, and the arguments it takes: permit_message's four, or transfer_message's six.")]
    pub fn message_call(&self) -> (&'static str, Vec<TxCallArgumentType>) {
        match self {
            TokenAuthorization::Permit { owner, spender, amount, deadline, .. } => (
                "permit_message",
                vec![TxCallArgumentType::Address(*owner), TxCallArgumentType::Address(*spender), TxCallArgumentType::U256(*amount), TxCallArgumentType::U64(*deadline)],
            ),
            TokenAuthorization::Transfer { from, to, amount, valid_after, valid_before, nonce, .. } => (
                "transfer_message",
                vec![
                    TxCallArgumentType::Address(*from),
                    TxCallArgumentType::Address(*to),
                    TxCallArgumentType::U256(*amount),
                    TxCallArgumentType::U64(*valid_after),
                    TxCallArgumentType::U64(*valid_before),
                    TxCallArgumentType::Bytes(nonce.to_vec()),
                ],
            ),
        }
    }

    #[document(parent = "TokenAuthorization", summary = "What it reads as, in a line: 'Let ed25519:0x… move up to 300' or 'Send 250 to ed25519:0x…', amounts in whole units.")]
    pub fn summary(&self) -> String {
        match self {
            TokenAuthorization::Permit { spender, amount, .. } => format!("Let {spender} move up to {amount}"),
            TokenAuthorization::Transfer { to, amount, .. } => format!("Send {amount} to {to}"),
        }
    }

    #[document(parent = "TokenAuthorization", summary = "Its terms as JSON: its kind, addresses and amounts as text, times in milliseconds, a transfer's nonce in hex. What a signed one's text holds beside its key and signature, and what a wallet hands an offline signer to sign.")]
    pub fn to_json(&self) -> Value {
        match self {
            TokenAuthorization::Permit { token, owner, spender, amount, nonce, deadline } => json!({
                "kind": "chainv4 token permit",
                "token": token.to_string(), "owner": owner.to_string(), "spender": spender.to_string(),
                "amount": amount.to_string(), "nonce": nonce.to_string(), "deadline": deadline,
            }),
            TokenAuthorization::Transfer { token, from, to, amount, valid_after, valid_before, nonce } => json!({
                "kind": "chainv4 token transfer",
                "token": token.to_string(), "from": from.to_string(), "to": to.to_string(), "amount": amount.to_string(),
                "valid_after": valid_after, "valid_before": valid_before, "nonce": to_hex(nonce),
            }),
        }
    }

    #[document(parent = "TokenAuthorization", summary = "Reads terms to_json wrote; any other fields beside them are left be.", returns = "The terms, or what does not read.")]
    pub fn from_json(value: &Value) -> Result<TokenAuthorization, String> {
        let text = |key: &str| value[key].as_str().ok_or_else(|| format!("it has no {key}"));
        let address = |key: &str| text(key)?.parse::<Address>().map_err(|_| format!("its {key} is not an address"));
        let number = |key: &str| text(key).and_then(|digits| u256::from_str_radix(digits, 10).map_err(|_| format!("its {key} is not a whole number")));
        let time = |key: &str| value[key].as_u64().ok_or_else(|| format!("its {key} is not a time in milliseconds"));
        let bytes = |key: &str| text(key).and_then(|hex| from_hex(hex).ok_or_else(|| format!("its {key} is not hex")));

        Ok(match text("kind")? {
            "chainv4 token permit" => TokenAuthorization::Permit { token: address("token")?, owner: address("owner")?, spender: address("spender")?, amount: number("amount")?, nonce: number("nonce")?, deadline: time("deadline")? },
            "chainv4 token transfer" => TokenAuthorization::Transfer {
                token: address("token")?,
                from: address("from")?,
                to: address("to")?,
                amount: number("amount")?,
                valid_after: time("valid_after")?,
                valid_before: time("valid_before")?,
                nonce: bytes("nonce")?.try_into().map_err(|_| "its nonce is not 32 bytes".to_string())?,
            },
            other => return Err(format!("not terms: its kind is {other}")),
        })
    }
}

#[document(
    summary = "An authorization with the owner's key and signature, ready for anyone to send: what a wallet hands on as text.",
    category = "Registries",
    see_also = "TokenAuthorization",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedAuthorization {
    #[document(summary = "What it authorizes.")]
    pub authorization: TokenAuthorization,
    #[document(summary = "The owner's public key, its bytes as crypto.verify takes them.")]
    pub key: Vec<u8>,
    #[document(summary = "Its signature over the text, 64 bytes.")]
    pub signature: Vec<u8>,
}

/// A key's own bytes and a signature's, as crypto.verify takes them.
fn raw(key: &PublicKey, signature: &Signature) -> Option<(Vec<u8>, Vec<u8>)> {
    let key = match key {
        #[cfg(feature = "ed25519")]
        PublicKey::Ed25519(bytes) => bytes.to_vec(),
        #[cfg(feature = "edblake")]
        PublicKey::EdBlake(bytes) => bytes.to_vec(),
        #[cfg(feature = "secp256k1")]
        PublicKey::Secp256k1(bytes) => bytes.to_vec(),
        #[cfg(feature = "bitcoin")]
        PublicKey::Bitcoin(bytes) => bytes.to_vec(),
    };
    let signature = match signature {
        #[cfg(feature = "ed25519")]
        Signature::Ed25519(bytes) => bytes.to_vec(),
        #[cfg(feature = "edblake")]
        Signature::EdBlake(bytes) => bytes.to_vec(),
        #[cfg(feature = "secp256k1")]
        Signature::Secp256k1(bytes) => bytes.to_vec(),
        #[cfg(feature = "bitcoin")]
        Signature::Bitcoin(bytes) => bytes.to_vec(),
        #[allow(unreachable_patterns)]
        _ => return None,
    };

    Some((key, signature))
}

impl SignedAuthorization {
    #[document(
        parent = "SignedAuthorization",
        summary = "Signs an authorization with the owner's key, over its text on this chain.",
        returns = "It signed, or why not: the key is not the owner's, or the owner is a contract, which signs nothing.",
    )]
    pub fn sign(authorization: TokenAuthorization, keypair: &Keypair, chain_id: u64) -> Result<SignedAuthorization, String> {
        if keypair.public().address() != authorization.owner() {
            return Err("the key is not the owner's".to_string());
        }

        let signature = keypair.sign(authorization.message(chain_id).as_bytes());
        let (key, signature) = raw(&keypair.public(), &signature).ok_or("a key that signs no text")?;

        Ok(SignedAuthorization { authorization, key, signature })
    }

    #[document(
        parent = "SignedAuthorization",
        summary = "Whether it holds as the token checks it: the key controls the owner's address, and the signature is over its text on this chain, by the key's scheme.",
        returns = "Ok, or why the token would refuse it, in the token's words.",
    )]
    pub fn check(&self, chain_id: u64) -> Result<(), String> {
        let owner = self.authorization.owner();
        let scheme = owner.kind();

        if matches!(scheme, "contract" | "evm" | "multisig") {
            return Err("only a key's account signs".to_string());
        }

        if crate::vm::crypto::key_address(scheme, &self.key).ok() != Some(owner) {
            return Err("the key is not the owner's".to_string());
        }

        match crate::vm::crypto::check_signature(scheme, &self.key, self.authorization.message(chain_id).as_bytes(), &self.signature) {
            Ok(true) => Ok(()),
            _ => Err("the signature does not hold".to_string()),
        }
    }

    #[document(parent = "SignedAuthorization", summary = "The call that uses it, from whoever sends it: permit or transfer_with_authorization on the token, the terms then the key and the signature.")]
    pub fn call(&self) -> Tx {
        let (function, mut args) = match &self.authorization {
            TokenAuthorization::Permit { .. } => ("permit", self.authorization.message_call().1),
            TokenAuthorization::Transfer { .. } => ("transfer_with_authorization", self.authorization.message_call().1),
        };
        args.push(TxCallArgumentType::Bytes(self.key.clone()));
        args.push(TxCallArgumentType::Bytes(self.signature.clone()));

        Tx::ContractCall { to: self.authorization.token(), func: Hasher::digest(HashType::Xxh3_64, function.as_bytes()), amount: Amount::U128(0), data: args }
    }

    #[document(parent = "SignedAuthorization", summary = "The function its call names: permit or transfer_with_authorization.")]
    pub fn function(&self) -> &'static str {
        match self.authorization {
            TokenAuthorization::Permit { .. } => "permit",
            TokenAuthorization::Transfer { .. } => "transfer_with_authorization",
        }
    }

    #[document(parent = "SignedAuthorization", summary = "It as JSON text to hand on: its kind, its terms, addresses and amounts as text, the key and signature in hex.")]
    pub fn to_text(&self) -> String {
        let mut value = self.authorization.to_json();
        value["key"] = to_hex(&self.key).into();
        value["signature"] = to_hex(&self.signature).into();

        value.to_string()
    }

    #[document(parent = "SignedAuthorization", summary = "Reads what to_text wrote.", returns = "It, or what does not read.")]
    pub fn from_text(text: &str) -> Result<SignedAuthorization, String> {
        let value: Value = serde_json::from_str(text.trim()).map_err(|_| "not a signed authorization: it is not JSON".to_string())?;
        let authorization = TokenAuthorization::from_json(&value).map_err(|why| why.replace("not terms", "not a signed authorization"))?;
        let bytes = |key: &str| value[key].as_str().ok_or_else(|| format!("it has no {key}")).and_then(|hex| from_hex(hex).ok_or_else(|| format!("its {key} is not hex")));

        Ok(SignedAuthorization { authorization, key: bytes("key")?, signature: bytes("signature")? })
    }
}

#[cfg(test)]
mod tests;
