//! The collection registry: every contract that exports the NFT standard (ERC-721-like, one owner
//! a token) or the multi-token standard (ERC-1155-like, amounts of each id), as genesis or
//! governance names them (docs/nft-standard.md). Contracts keeps it beside the token registry: a
//! deploy or upgrade that leaves a contract exporting one standard's functions registers it, one
//! that leaves it short takes it off.
//!
//! name() and symbol() are optional, as for tokens: those a collection exports are asked when it
//! is deployed or upgraded, and one that answers badly fails the deploy or upgrade.
//!
//! The registry shares Column::Tokens with the token registry, under keys of its own: each
//! collection once by address, and listed by symbol as tokens are.

use {
    crate::crypto::hashable::Hashable,
    crate::db::{Column, Committed, DbError, State, address_key},
    crate::registry::tokens::{MAX_NAME, MAX_SYMBOL},
    crate::structs::address::Address,
    crate::structs::tx::TxCallArgumentType,
    r#macro::document,
    wincode::{SchemaRead, SchemaWrite},
};

#[document(
    summary = "The NFT standard's required functions, sorted: what makes a contract an NFT collection, one owner a token, unless genesis or governance names others.",
    category = "Registries",
    see_also = "CollectionFunctions, Collections",
)]
pub const STANDARD_NFT_FUNCTIONS: [&str; 12] = [
    "approve",
    "balance_of",
    "get_approved",
    "is_approved_for_all",
    "owner_of",
    "safe_transfer",
    "safe_transfer_from",
    "set_approval_for_all",
    "token_uri",
    "tokens_of",
    "transfer",
    "transfer_from",
];

#[document(
    summary = "The multi-token standard's required functions, sorted: what makes a contract a multi-token collection, amounts of each id, unless genesis or governance names others.",
    category = "Registries",
    see_also = "CollectionFunctions, Collections",
)]
pub const STANDARD_MULTI_FUNCTIONS: [&str; 9] = [
    "balance_of",
    "is_approved_for_all",
    "safe_transfer",
    "safe_transfer_from",
    "set_approval_for_all",
    "token_uri",
    "tokens_of",
    "transfer",
    "transfer_from",
];

#[document(
    summary = "The optional functions a collection may describe itself with. Those it exports are asked when it is deployed or upgraded.",
    category = "Registries",
    see_also = "Collection",
)]
pub const COLLECTION_METADATA_FUNCTIONS: [&str; 2] = ["name", "symbol"];

#[document(
    summary = "The function safe transfers call on a contract receiving tokens: on_nft_received(operator, from, id, amount), which must return true. Named here so wallets and contracts agree; the chain never calls it itself.",
    category = "Registries",
    see_also = "STANDARD_NFT_FUNCTIONS",
)]
pub const ON_NFT_RECEIVED: &str = "on_nft_received";

#[document(
    summary = "What makes a contract a collection on a chain: the functions an NFT collection exports, and those a multi-token one does. A contract exporting every NFT function is an NFT collection, else one exporting every multi-token function is a multi-token one. Either list may be empty, keeping no collections of that kind.",
    category = "Registries",
    see_also = "Genesis::collections, Change::Collections",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct CollectionFunctions {
    #[document(summary = "The NFT standard's functions, usually STANDARD_NFT_FUNCTIONS.")]
    pub nft: Vec<String>,
    #[document(summary = "The multi-token standard's functions, usually STANDARD_MULTI_FUNCTIONS.")]
    pub multi: Vec<String>,
}

impl Hashable for CollectionFunctions {}

impl CollectionFunctions {
    #[document(parent = "CollectionFunctions", summary = "The standards' own lists: STANDARD_NFT_FUNCTIONS and STANDARD_MULTI_FUNCTIONS.")]
    pub fn standard() -> CollectionFunctions {
        CollectionFunctions {
            nft: STANDARD_NFT_FUNCTIONS.map(str::to_string).to_vec(),
            multi: STANDARD_MULTI_FUNCTIONS.map(str::to_string).to_vec(),
        }
    }

    #[document(
        parent = "CollectionFunctions",
        summary = "Which kind of collection a contract exporting these names is: NFT before multi-token, so a contract exporting both lists is an NFT collection.",
        returns = "The kind, or None when it exports neither list in full, or the list is empty.",
    )]
    pub fn kind(&self, exports: impl Fn(&str) -> bool) -> Option<CollectionKind> {
        let all = |functions: &[String]| !functions.is_empty() && functions.iter().all(|function| exports(function));

        if all(&self.nft) {
            Some(CollectionKind::Nft)
        } else if all(&self.multi) {
            Some(CollectionKind::Multi)
        } else {
            None
        }
    }

    #[document(
        parent = "CollectionFunctions",
        summary = "Why these lists cannot be a chain's: a function named twice in one list, or an empty name.",
    )]
    pub fn problem(&self) -> Option<&'static str> {
        for functions in [&self.nft, &self.multi] {
            let mut sorted: Vec<&String> = functions.iter().collect();
            sorted.sort();

            if sorted.iter().any(|name| name.is_empty()) || sorted.windows(2).any(|pair| pair[0] == pair[1]) {
                return Some("collections must name each function once in each list");
            }
        }

        None
    }
}

#[document(
    summary = "Which standard a collection follows.",
    category = "Registries",
    see_also = "Collection",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
pub enum CollectionKind {
    #[document(summary = "The NFT standard, ERC-721-like: each token has one owner.")]
    #[wincode(tag = 0)]
    Nft,
    #[document(summary = "The multi-token standard, ERC-1155-like: holders hold amounts of each id.")]
    #[wincode(tag = 1)]
    Multi,
}

impl CollectionKind {
    #[document(parent = "CollectionKind", summary = "\"nft\" or \"multi\", as the API and genesis files write it.")]
    pub fn as_str(self) -> &'static str {
        match self {
            CollectionKind::Nft => "nft",
            CollectionKind::Multi => "multi",
        }
    }
}

/// Key kinds, apart from the token registry's 'a' and 's': the collection by address, and a
/// listing entry by symbol that holds its address key.
const BY_ADDRESS: u8 = b'c';
const BY_SYMBOL: u8 = b'n';

#[document(
    summary = "A registered collection: its address, the standard it follows, and what the optional name() and symbol() answered when it was deployed or last upgraded, for those it exports.",
    category = "Registries",
    see_also = "Collections, CollectionKind",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct Collection {
    #[document(summary = "The collection contract.")]
    pub address: Address,
    #[document(summary = "The standard it follows.")]
    pub kind: CollectionKind,
    #[document(summary = "Its name, such as \"Example Collection\": 1 to MAX_NAME bytes, no control characters. None when it exports no name().")]
    pub name: Option<String>,
    #[document(summary = "Its symbol, such as \"EXC\": 1 to MAX_SYMBOL bytes, no control characters. Not unique. None when it exports no symbol().")]
    pub symbol: Option<String>,
}

impl Hashable for Collection {}

impl Collection {
    #[document(
        parent = "Collection",
        summary = "The collection a contract describes by what its name() and symbol() returned, each None when it does not export it.",
        returns = "The collection, or why an answer does not do: each must be one string within its limits.",
    )]
    pub fn from_answers(
        address: Address,
        kind: CollectionKind,
        name: Option<&[TxCallArgumentType]>,
        symbol: Option<&[TxCallArgumentType]>,
    ) -> Result<Collection, String> {
        Ok(Collection {
            address,
            kind,
            name: name.map(|answer| text("name", answer, MAX_NAME)).transpose()?,
            symbol: symbol.map(|answer| text("symbol", answer, MAX_SYMBOL)).transpose()?,
        })
    }
}

/// One string answer within a limit, without control characters, as tokens' are.
pub(crate) fn text(function: &str, answer: &[TxCallArgumentType], max: usize) -> Result<String, String> {
    match answer {
        [TxCallArgumentType::String(text)] if text.is_empty() || text.len() > max => {
            Err(format!("{function}() must give 1 to {max} bytes, not {}", text.len()))
        }
        [TxCallArgumentType::String(text)] if text.chars().any(char::is_control) => Err(format!("{function}() gave control characters")),
        [TxCallArgumentType::String(text)] => Ok(text.clone()),
        _ => Err(format!("{function}() must return one string")),
    }
}

#[document(
    summary = "The collection registry: every contract that exports the NFT or multi-token standard, by address and by symbol. Contracts keeps it as contracts are deployed and upgraded.",
    category = "Registries",
    see_also = "Collection, Contracts, stored_collections",
)]
pub struct Collections<'a> {
    state: &'a State,
}

impl<'a> Collections<'a> {
    #[document(parent = "Collections", summary = "Reads and writes the registry in the given state layer.")]
    pub fn new(state: &'a State) -> Collections<'a> {
        Collections { state }
    }

    #[document(
        parent = "Collections",
        summary = "A registered collection.",
        returns = "The collection, or None when the address is not one.",
    )]
    pub fn get(&self, address: &Address) -> Result<Option<Collection>, DbError> {
        self.state.get_value(Column::Tokens, &collection_key(address))
    }

    #[document(
        parent = "Collections",
        summary = "Registers a collection, replacing what an earlier deploy or upgrade registered for its address.",
    )]
    pub fn register(&self, collection: &Collection) -> Result<(), DbError> {
        self.unregister(&collection.address)?;
        self.state.put(Column::Tokens, listing(collection.symbol.as_deref(), &collection.address), address_key(&collection.address));
        self.state.put_value(Column::Tokens, collection_key(&collection.address), collection);

        Ok(())
    }

    #[document(parent = "Collections", summary = "Takes a contract off the registry, if it is on it.")]
    pub fn unregister(&self, address: &Address) -> Result<(), DbError> {
        if let Some(old) = self.get(address)? {
            self.state.delete(Column::Tokens, listing(old.symbol.as_deref(), address));
            self.state.delete(Column::Tokens, collection_key(address));
        }

        Ok(())
    }
}

#[document(
    summary = "Where the registry keeps a collection's entry, in Column::Tokens: what a light client proves a collection's kind, name and symbol by.",
    category = "Registries",
    see_also = "Collection",
)]
pub fn collection_key(address: &Address) -> Vec<u8> {
    [&[BY_ADDRESS][..], &address_key(address)].concat()
}

/// Where symbols starting with this go: ASCII letters in lower case, as tokens'.
fn symbol_prefix(symbol: &str) -> Vec<u8> {
    [&[BY_SYMBOL][..], symbol.to_ascii_lowercase().as_bytes()].concat()
}

/// A collection's listing key; one without a symbol lists as the empty one, ahead of every other.
fn listing(symbol: Option<&str>, address: &Address) -> Vec<u8> {
    [&symbol_prefix(symbol.unwrap_or(""))[..], &[0], &address_key(address)].concat()
}

#[document(
    summary = "A page of registered collections by symbol, read straight from a database: alphabetically, ignoring the case of ASCII letters, and by address within a symbol. Collections without a symbol come first.",
    category = "Registries",
    params = {
        db = "The database.",
        symbol = "Only symbols starting with this, ignoring ASCII case; an exact match comes first. None for every collection.",
        cursor = "Where the last page stopped, as it returned; None for the first.",
        limit = "The most collections to return.",
    },
    returns = "The collections, and the cursor for the next page, or None after the last.",
    see_also = "Collections",
)]
pub fn stored_collections(
    db: &dyn Committed,
    symbol: Option<&str>,
    cursor: Option<&[u8]>,
    limit: usize,
) -> Result<(Vec<Collection>, Option<Vec<u8>>), DbError> {
    let prefix = symbol_prefix(symbol.unwrap_or(""));
    let mut found: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    db.scan(Column::Tokens, &prefix, cursor.unwrap_or(&prefix), &mut |key, value| {
        found.push((key.to_vec(), value.to_vec()));

        found.len() <= limit
    })?;
    let next = (found.len() > limit).then(|| found.remove(limit).0);
    let mut collections = Vec::with_capacity(found.len());

    for (_, address) in found {
        let record = db
            .read(Column::Tokens, &[&[BY_ADDRESS][..], &address].concat())?
            .ok_or_else(|| DbError::Corrupt("a collection listing points at nothing".to_string()))?;

        collections.push(
            wincode::deserialize::<Collection>(&record)
                .map_err(|err| DbError::Corrupt(format!("a registered collection would not decode: {err}")))?,
        );
    }

    Ok((collections, next))
}

#[cfg(test)]
mod tests;
