//! The token registry: every contract that exports the functions genesis says make a token, by
//! default ERC-20's required six (docs/token-standard.md). Contracts keeps it: a deploy or upgrade
//! that leaves a contract exporting them all registers it, one that leaves it short of one takes it
//! off. So a contract is in the registry exactly when it exports them.
//!
//! As in ERC-20, name(), symbol() and decimals() are optional. Those a token exports are asked once,
//! when it is deployed or upgraded, and their answers kept; one that answers badly fails the deploy
//! or upgrade, so what the registry says can be trusted.
//!
//! Each token is stored once, by address, and listed by symbol: a key of its symbol, in ASCII
//! lower case, then a zero byte, then its address. Symbols hold no control characters, so an exact
//! match sorts before every longer symbol it starts, and tokens without one come first.

use {
    crate::crypto::hashable::Hashable,
    crate::db::{Column, Database, DbError, State, address_key},
    crate::structs::address::Address,
    crate::structs::tx::TxCallArgumentType,
    r#macro::document,
    wincode::{SchemaRead, SchemaWrite},
};

#[document(
    summary = "ERC-20's required functions, sorted: what makes a contract a token unless genesis names others in token_functions.",
    category = "Registries",
    see_also = "Tokens, Genesis",
)]
pub const STANDARD_TOKEN_FUNCTIONS: [&str; 6] = ["allowance", "approve", "balance_of", "total_supply", "transfer", "transfer_from"];

#[document(
    summary = "The optional functions a token may describe itself with. Those it exports are asked when it is deployed or upgraded.",
    category = "Registries",
    see_also = "Token",
)]
pub const METADATA_FUNCTIONS: [&str; 3] = ["name", "symbol", "decimals"];

#[document(summary = "The longest name a token may give, in bytes.", category = "Registries", see_also = "Token")]
pub const MAX_NAME: usize = 64;

#[document(summary = "The longest symbol a token may give, in bytes.", category = "Registries", see_also = "Token")]
pub const MAX_SYMBOL: usize = 16;

#[document(
    summary = "The most decimals a token may have: 10^77 is the largest power of ten a u256 holds.",
    category = "Registries",
    see_also = "Token",
)]
pub const MAX_DECIMALS: u8 = 77;

/// Key kinds: the token by address, and a listing entry by symbol that holds its address key.
const BY_ADDRESS: u8 = b'a';
const BY_SYMBOL: u8 = b's';

#[document(
    summary = "A registered token: its address, and what the optional name(), symbol() and decimals() answered when it was deployed or last upgraded, for those it exports.",
    category = "Registries",
    see_also = "Tokens, STANDARD_TOKEN_FUNCTIONS",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct Token {
    #[document(summary = "The token contract.")]
    pub address: Address,
    #[document(summary = "Its name, such as \"Example Token\": 1 to MAX_NAME bytes, no control characters. None when it exports no name().")]
    pub name: Option<String>,
    #[document(summary = "Its ticker, such as \"EXT\": 1 to MAX_SYMBOL bytes, no control characters. Symbols are not unique. None when it exports no symbol().")]
    pub symbol: Option<String>,
    #[document(summary = "How many decimals a display divides by, up to MAX_DECIMALS. None when it exports no decimals().")]
    pub decimals: Option<u8>,
}

impl Hashable for Token {}

impl Token {
    #[document(
        parent = "Token",
        summary = "The token a contract describes by what its name(), symbol() and decimals() returned, each None when it does not export it.",
        returns = "The token, or why an answer does not do: a name and symbol must be one string within their limits, decimals one whole number up to MAX_DECIMALS.",
    )]
    pub fn from_answers(
        address: Address,
        name: Option<&[TxCallArgumentType]>,
        symbol: Option<&[TxCallArgumentType]>,
        decimals: Option<&[TxCallArgumentType]>,
    ) -> Result<Token, String> {
        let text = |function: &str, answer: &[TxCallArgumentType], max: usize| match answer {
            [TxCallArgumentType::String(text)] if text.is_empty() || text.len() > max => {
                Err(format!("{function}() must give 1 to {max} bytes, not {}", text.len()))
            }
            [TxCallArgumentType::String(text)] if text.chars().any(char::is_control) => {
                Err(format!("{function}() gave control characters"))
            }
            [TxCallArgumentType::String(text)] => Ok(text.clone()),
            _ => Err(format!("{function}() must return one string")),
        };
        let whole_decimals = |answer: &[TxCallArgumentType]| match answer {
            [value] => whole(value)
                .filter(|decimals| *decimals <= u64::from(MAX_DECIMALS))
                .and_then(|decimals| u8::try_from(decimals).ok())
                .ok_or_else(|| format!("decimals() must return one whole number from 0 to {MAX_DECIMALS}")),
            _ => Err(format!("decimals() must return one whole number from 0 to {MAX_DECIMALS}")),
        };

        Ok(Token {
            address,
            name: name.map(|answer| text("name", answer, MAX_NAME)).transpose()?,
            symbol: symbol.map(|answer| text("symbol", answer, MAX_SYMBOL)).transpose()?,
            decimals: decimals.map(whole_decimals).transpose()?,
        })
    }
}

/// A value as a whole number, whichever numeric type carries it.
fn whole(value: &TxCallArgumentType) -> Option<u64> {
    match value {
        TxCallArgumentType::Number(number) if number.fract() == 0.0 && *number >= 0.0 && *number <= u64::MAX as f64 => {
            Some(*number as u64)
        }
        TxCallArgumentType::I64(number) => u64::try_from(*number).ok(),
        TxCallArgumentType::U64(number) => Some(*number),
        TxCallArgumentType::I128(number) => u64::try_from(*number).ok(),
        TxCallArgumentType::U128(number) => u64::try_from(*number).ok(),
        TxCallArgumentType::U256(number) => u64::try_from(*number).ok(),
        _ => None,
    }
}

#[document(
    summary = "The token registry: every contract that exports the token standard, by address and by symbol. Contracts keeps it as contracts are deployed and upgraded.",
    category = "Registries",
    see_also = "Token, Contracts, stored_tokens",
)]
pub struct Tokens<'a> {
    state: &'a State,
}

impl<'a> Tokens<'a> {
    #[document(parent = "Tokens", summary = "Reads and writes the registry in the given state layer.")]
    pub fn new(state: &'a State) -> Tokens<'a> {
        Tokens { state }
    }

    #[document(
        parent = "Tokens",
        summary = "A registered token.",
        returns = "The token, or None when the address is not one.",
    )]
    pub fn get(&self, address: &Address) -> Result<Option<Token>, DbError> {
        self.state.get_value(Column::Tokens, &by_address(address))
    }

    #[document(
        parent = "Tokens",
        summary = "Registers a token, replacing what an earlier deploy or upgrade registered for its address.",
    )]
    pub fn register(&self, token: &Token) -> Result<(), DbError> {
        self.unregister(&token.address)?;
        self.state.put(Column::Tokens, listing(token.symbol.as_deref(), &token.address), address_key(&token.address));
        self.state.put_value(Column::Tokens, by_address(&token.address), token);

        Ok(())
    }

    #[document(parent = "Tokens", summary = "Takes a contract off the registry, if it is on it.")]
    pub fn unregister(&self, address: &Address) -> Result<(), DbError> {
        if let Some(old) = self.get(address)? {
            self.state.delete(Column::Tokens, listing(old.symbol.as_deref(), address));
            self.state.delete(Column::Tokens, by_address(address));
        }

        Ok(())
    }
}

fn by_address(address: &Address) -> Vec<u8> {
    [&[BY_ADDRESS][..], &address_key(address)].concat()
}

#[document(
    summary = "Where the registry keeps a token's entry, in Column::Tokens: what a light client proves a token's name, symbol and decimals by.",
    category = "Registries",
    see_also = "Token",
)]
pub fn token_key(address: &Address) -> Vec<u8> {
    by_address(address)
}

/// Where symbols starting with this go: ASCII letters in lower case, so a search ignores their
/// case, and everything else as it is, so no node's Unicode tables decide where a key goes.
fn symbol_prefix(symbol: &str) -> Vec<u8> {
    [&[BY_SYMBOL][..], symbol.to_ascii_lowercase().as_bytes()].concat()
}

/// A token's listing key; one without a symbol lists as the empty one, ahead of every other.
fn listing(symbol: Option<&str>, address: &Address) -> Vec<u8> {
    [&symbol_prefix(symbol.unwrap_or(""))[..], &[0], &address_key(address)].concat()
}

#[document(
    summary = "A page of registered tokens by symbol, read straight from a database: alphabetically, ignoring the case of ASCII letters, and by address within a symbol. Tokens without a symbol come first.",
    category = "Registries",
    params = {
        db = "The database.",
        symbol = "Only symbols starting with this, ignoring ASCII case; an exact match comes first. None for every token.",
        cursor = "Where the last page stopped, as it returned; None for the first.",
        limit = "The most tokens to return.",
    },
    returns = "The tokens, and the cursor for the next page, or None after the last.",
    see_also = "Tokens",
)]
pub fn stored_tokens(
    db: &dyn Database,
    symbol: Option<&str>,
    cursor: Option<&[u8]>,
    limit: usize,
) -> Result<(Vec<Token>, Option<Vec<u8>>), DbError> {
    let prefix = symbol_prefix(symbol.unwrap_or(""));
    let mut found: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    db.scan(Column::Tokens, &prefix, cursor.unwrap_or(&prefix), &mut |key, value| {
        found.push((key.to_vec(), value.to_vec()));

        found.len() <= limit
    })?;
    let next = (found.len() > limit).then(|| found.remove(limit).0);
    let mut tokens = Vec::with_capacity(found.len());

    for (_, address) in found {
        let record = db
            .read(Column::Tokens, &[&[BY_ADDRESS][..], &address].concat())?
            .ok_or_else(|| DbError::Corrupt("a token listing points at nothing".to_string()))?;

        tokens.push(
            wincode::deserialize::<Token>(&record)
                .map_err(|err| DbError::Corrupt(format!("a registered token would not decode: {err}")))?,
        );
    }

    Ok((tokens, next))
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::db::MemoryDb,
        std::sync::Arc,
    };

    fn token(byte: u8, symbol: Option<&str>) -> Token {
        Token {
            address: Address::Contract([byte; 32]),
            name: None,
            symbol: symbol.map(str::to_string),
            decimals: None,
        }
    }

    fn symbols(db: &MemoryDb, symbol: Option<&str>) -> Vec<Option<String>> {
        stored_tokens(db, symbol, None, 100).unwrap().0.into_iter().map(|token| token.symbol).collect()
    }

    #[test]
    fn tokens_are_listed_by_symbol_ignoring_ascii_case() {
        let db = Arc::new(MemoryDb::default());
        let state = State::new(db.clone());
        let tokens = Tokens::new(&state);

        for (byte, symbol) in [(1, Some("ext2")), (2, Some("EXT")), (3, Some("ABC")), (4, None), (5, Some("Ex")), (6, Some("ÉX"))] {
            tokens.register(&token(byte, symbol)).unwrap();
        }

        db.commit(state.take_changes()).unwrap();
        let some = |symbols: &[&str]| symbols.iter().map(|symbol| Some(symbol.to_string())).collect::<Vec<_>>();

        // Those without a symbol first, then alphabetically; only ASCII letters fold.
        assert_eq!(symbols(&db, None), [vec![None], some(&["ABC", "Ex", "EXT", "ext2", "ÉX"])].concat());
        assert_eq!(symbols(&db, Some("ex")), some(&["Ex", "EXT", "ext2"]), "an exact match first");
        assert_eq!(symbols(&db, Some("EXT")), some(&["EXT", "ext2"]));
        assert!(symbols(&db, Some("x")).is_empty());

        // A page at a time.
        let (first, next) = stored_tokens(&*db, Some("ex"), None, 2).unwrap();
        let (rest, after) = stored_tokens(&*db, Some("ex"), next.as_deref(), 2).unwrap();

        assert_eq!((first.len(), rest.len(), after), (2, 1, None));
        assert_eq!(rest[0].symbol.as_deref(), Some("ext2"));

        // Registering again moves its listing; taking it off removes both its keys.
        let state = State::new(db.clone());
        let tokens = Tokens::new(&state);
        tokens.register(&token(2, Some("NEW"))).unwrap();
        tokens.unregister(&Address::Contract([3; 32])).unwrap();
        db.commit(state.take_changes()).unwrap();

        assert_eq!(symbols(&db, None), [vec![None], some(&["Ex", "ext2", "NEW", "ÉX"])].concat());
        assert_eq!(Tokens::new(&State::new(db.clone())).get(&Address::Contract([3; 32])).unwrap(), None);
    }

    #[test]
    fn answers_become_a_token_only_within_their_limits() {
        let address = Address::Contract([1; 32]);
        let text = |text: &str| vec![TxCallArgumentType::String(text.to_string())];

        for decimals in [TxCallArgumentType::Number(18.0), TxCallArgumentType::U64(18), TxCallArgumentType::U256(18u8.into())] {
            let token = Token::from_answers(address, Some(&text("Example")), Some(&text("EXT")), Some(&[decimals])).unwrap();

            assert_eq!((token.name.as_deref(), token.symbol.as_deref(), token.decimals), (Some("Example"), Some("EXT"), Some(18)));
        }

        assert_eq!(Token::from_answers(address, None, None, None).unwrap().symbol, None);

        let bad = [
            Token::from_answers(address, Some(&[]), None, None),
            Token::from_answers(address, None, Some(&text("SEVENTEEN-LETTERS")), None),
            Token::from_answers(address, None, Some(&text("A\u{7}")), None),
            Token::from_answers(address, None, None, Some(&[TxCallArgumentType::I64(-1)])),
            Token::from_answers(address, None, None, Some(&[TxCallArgumentType::U64(6), TxCallArgumentType::U64(6)])),
        ];

        for (index, result) in bad.into_iter().enumerate() {
            assert!(result.is_err(), "case {index}: {result:?}");
        }
    }
}
