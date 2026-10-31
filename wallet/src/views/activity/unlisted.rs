//! What an EVM contract the registry does not list says of itself, for the Activity page's lines
//! and for nothing else. Without it, a token moved by such a contract shows as a plain whole number
//! of a short address; with it, as 1.5 "USDC" (unverified).
//!
//! A contract can say anything, and the node answers for it unproven. So what comes back is kept
//! here, private to this page: no screen someone signs on, no balance and nothing sent can reach it,
//! and no line that shows it says the move happened. It is cut short and escaped before it is kept,
//! the quotes and (unverified) always show around it, and a symbol the same as a listed token's or
//! the coin's looks like any other. Anything that does not read is as if nothing was said: the plain
//! whole number stays, and nothing is said of why.
//!
//! It is held for the session, by chain and contract, and never stored: settings that do not decode
//! lose everything in them. Questions go one at a time, for what is on screen, well under what the
//! node's nginx lets one address ask of /rpc.

use {
    crate::format,
    crate::node::{Ask, Failure},
    chainv4::abi::known::clean_text,
    chainv4::abi::{AbiType, AbiValue, decode_values, parse_signature},
    chainv4::structs::address::Address,
    chainv4::structs::hex::{from_hex, to_hex},
    serde_json::{Value, json},
    std::collections::HashMap,
};

/// What every amount named this way ends with, however long its symbol.
pub const UNVERIFIED: &str = "(unverified)";

/// A symbol as long as it may be, in the contract's characters; and once escaped, which can make one
/// character ten, in what shows: a row must not grow much for what a contract chose.
const MAX_SYMBOL: usize = 16;
const MAX_SHOWN: usize = 32;
/// The most decimals taken as meant: more are a contract playing with how small an amount looks.
const MAX_DECIMALS: u8 = 36;
/// The longest answer read, in bytes: a symbol and a number take far less.
const MAX_ANSWER: usize = 4 * 1024;

/// How long between questions, in milliseconds. The node's nginx lets an address make 5 contract
/// calls a second, with a burst of 20, and the Trade page and a site's requests go by /rpc too: 2.5
/// a second leaves them room, however long the list.
const PACE: u64 = 400;
/// How long nothing is asked after the node says too many, in milliseconds.
const LIMITED_FOR: u64 = 30_000;
/// How long before asking again what did not read, in milliseconds; and after the node failed to
/// answer at all, how long before asking anything.
const RETRY_AFTER: u64 = 5 * 60 * 1_000;
/// How long a question stays out before it is taken for lost, in milliseconds: its answer never
/// comes if the wallet connects to another node meanwhile. Requests time out well before this.
const LOST_AFTER: u64 = 2 * 60 * 1_000;

/// Which of the two a question asks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Question {
    Symbol,
    Decimals,
}

impl Question {
    fn signature(self) -> &'static str {
        match self {
            Question::Symbol => "symbol()",
            Question::Decimals => "decimals()",
        }
    }
}

/// How one of the two stands.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum Asked<T> {
    /// Not asked yet, or to ask again, as the node said too many.
    #[default]
    Not,
    /// On its way, since then.
    Out(u64),
    /// It did not read, at that time.
    Failed(u64),
    Said(T),
}

impl<T> Asked<T> {
    /// Whether to ask it now.
    fn due(&self, now: u64) -> bool {
        match self {
            Asked::Not => true,
            Asked::Out(at) => now.saturating_sub(*at) >= LOST_AFTER,
            Asked::Failed(at) => now.saturating_sub(*at) >= RETRY_AFTER,
            Asked::Said(_) => false,
        }
    }
}

/// What a contract said of itself so far.
#[derive(Clone, Debug, Default)]
struct Contract {
    symbol: Asked<String>,
    decimals: Asked<u8>,
}

impl Contract {
    /// What to ask it next: its symbol, since with none there is nothing to show its amounts in, so
    /// its decimals are not worth asking.
    fn question(&self, now: u64) -> Option<Question> {
        match &self.symbol {
            Asked::Said(_) if self.decimals.due(now) => Some(Question::Decimals),
            Asked::Said(_) => None,
            symbol if symbol.due(now) => Some(Question::Symbol),
            _ => None,
        }
    }
}

/// What unlisted contracts said, for this page, for as long as the wallet stays open.
#[derive(Default)]
pub struct Unlisted {
    /// By the chain's genesis id, then the contract as the chain writes it.
    chains: HashMap<String, HashMap<String, Contract>>,
    /// What is out, by the body it went as: the chain, the contract and the question it answers.
    out: HashMap<String, (String, String, Question)>,
    /// When the next question may go, in milliseconds since the Unix epoch.
    next: u64,
}

impl Unlisted {
    /// The question to send now about these contracts, the ones on screen in order, if one is due:
    /// one at most, however often a frame comes.
    pub fn next(&mut self, genesis: &str, wanted: &[String], now: u64) -> Option<Ask> {
        if now < self.next {
            return None;
        }

        for contract in wanted {
            let question = self.chains.get(genesis).and_then(|contracts| contracts.get(contract)).map_or(Some(Question::Symbol), |said| said.question(now));
            let Some(question) = question else {
                continue;
            };
            let (Ok(Address::Evm(to)), Ok((Some(selector), _))) = (contract.parse::<Address>(), parse_signature(question.signature())) else {
                continue;
            };
            let body = json!({ "jsonrpc": "2.0", "id": "unlisted", "method": "eth_call", "params": [{ "to": to_hex(&to), "data": to_hex(&selector) }, "latest"] }).to_string();
            let said = self.chains.entry(genesis.to_string()).or_default().entry(contract.clone()).or_default();

            match question {
                Question::Symbol => said.symbol = Asked::Out(now),
                Question::Decimals => said.decimals = Asked::Out(now),
            }

            self.out.insert(body.clone(), (genesis.to_string(), contract.clone(), question));
            self.next = now + PACE;

            return Some(Ask::Rpc(body));
        }

        None
    }

    /// Takes an answer to a question of this page's: false for anyone else's. Nothing it says goes
    /// on a screen as a failure: a line simply stays as it was.
    pub fn answered(&mut self, body: &str, result: &Result<Value, Failure>, now: u64) -> bool {
        let Some((genesis, contract, question)) = self.out.remove(body) else {
            return false;
        };
        let said = self.chains.entry(genesis).or_default().entry(contract).or_default();

        match result {
            // Too many for now: nothing is wrong with what was asked, and it is asked again later.
            Err(Failure { status: 429, .. }) => {
                match question {
                    Question::Symbol => said.symbol = Asked::Not,
                    Question::Decimals => said.decimals = Asked::Not,
                }

                self.next = self.next.max(now + LIMITED_FOR);
            }
            Err(_) => {
                match question {
                    Question::Symbol => said.symbol = Asked::Failed(now),
                    Question::Decimals => said.decimals = Asked::Failed(now),
                }

                // A node that is away, or that has no /rpc, is not asked the rest in the meantime.
                self.next = self.next.max(now + RETRY_AFTER);
            }
            Ok(answer) => match question {
                Question::Symbol => said.symbol = output(answer).and_then(|output| symbol_in(&output)).map_or(Asked::Failed(now), Asked::Said),
                Question::Decimals => said.decimals = output(answer).and_then(|output| decimals_in(&output)).map_or(Asked::Failed(now), Asked::Said),
            },
        }

        true
    }
}

/// What a JSON-RPC answer holds, unless it is an error or too long to be what was asked.
fn output(answer: &Value) -> Option<Vec<u8>> {
    answer["result"].as_str().filter(|hex| hex.len() <= 2 * MAX_ANSWER + 2).and_then(from_hex)
}

/// A symbol, cut short and escaped: ready to show, and none if there is nothing to see of it. A
/// token from before the standard, whose symbol is 32 bytes rather than a string, has none here.
fn symbol_in(output: &[u8]) -> Option<String> {
    let values = decode_values(&[AbiType::String], output).ok()?;
    let [AbiValue::String(symbol)] = values.as_slice() else {
        return None;
    };

    if symbol.trim().is_empty() {
        return None;
    }

    let clean = clean_text(symbol, MAX_SYMBOL);

    Some(match clean.chars().count() > MAX_SHOWN {
        true => format!("{}…", clean.chars().take(MAX_SHOWN).collect::<String>()),
        false => clean,
    })
}

/// A contract's decimals, if they are ones a token could mean.
fn decimals_in(output: &[u8]) -> Option<u8> {
    let values = decode_values(&[AbiType::Uint(8)], output).ok()?;
    let [AbiValue::Uint(decimals)] = values.as_slice() else {
        return None;
    };

    u8::try_from(*decimals).ok().filter(|decimals| *decimals <= MAX_DECIMALS)
}

/// What one chain's contracts said, as the lines drawn may read it. The default says nothing, which
/// is what every line outside the Activity page has: the Overview's sit under its balances.
#[derive(Clone, Copy, Default)]
pub struct Said<'a> {
    unlisted: Option<&'a Unlisted>,
    genesis: &'a str,
}

impl<'a> Said<'a> {
    pub fn on(unlisted: &'a Unlisted, genesis: &'a str) -> Said<'a> {
        Said { unlisted: Some(unlisted), genesis }
    }

    /// What this contract says it is called and how many decimals it says it has, once both read.
    fn of(&self, contract: &str) -> Option<(&'a str, u8)> {
        let said = self.unlisted?.chains.get(self.genesis)?.get(contract)?;

        match (&said.symbol, &said.decimals) {
            (Asked::Said(symbol), Asked::Said(decimals)) => Some((symbol, *decimals)),
            _ => None,
        }
    }

    /// An amount of this contract's in what it says of itself, cut short and in full: None until it
    /// has said both, and for one that never will.
    pub fn amount(&self, contract: &str, raw: &str) -> Option<(String, String)> {
        let (symbol, decimals) = self.of(contract)?;
        let label = format!("\"{symbol}\" {UNVERIFIED}");

        Some((format!("{} {label}", format::compact(raw, decimals, 4)), format::amount(raw, decimals, &label)))
    }
}

#[cfg(test)]
pub mod tests {
    use {super::*, chainv4::abi::encode_values, chainv4::structs::u256::u256};

    pub const GENESIS: &str = "0x9a7f";
    const TOKEN: &str = "evm:0x0505050505050505050505050505050505050505";

    pub fn says_symbol(symbol: &str) -> Value {
        json!({ "jsonrpc": "2.0", "id": "unlisted", "result": to_hex(&encode_values(&[AbiType::String], &[AbiValue::String(symbol.to_string())]).unwrap()) })
    }

    pub fn says_number(number: u64) -> Value {
        json!({ "jsonrpc": "2.0", "id": "unlisted", "result": to_hex(&u256::new(u128::from(number)).to_be_bytes()) })
    }

    fn body(ask: Option<Ask>) -> String {
        match ask {
            Some(Ask::Rpc(body)) => body,
            other => panic!("asked {other:?}"),
        }
    }

    /// A contract's two answers taken, as they come: what it then is, on the chain followed.
    pub fn answering(unlisted: &mut Unlisted, genesis: &str, contract: &str, symbol: &Value, decimals: &Value, now: u64) {
        let wanted = [contract.to_string()];

        for (at, answer) in [(now, symbol), (now + PACE, decimals)] {
            if let Some(ask) = unlisted.next(genesis, &wanted, at) {
                assert!(unlisted.answered(&body(Some(ask)), &Ok(answer.clone()), at));
            }
        }
    }

    fn named(symbol: &Value, decimals: &Value, raw: &str) -> Option<(String, String)> {
        let mut unlisted = Unlisted::default();
        answering(&mut unlisted, GENESIS, TOKEN, symbol, decimals, 0);

        Said::on(&unlisted, GENESIS).amount(TOKEN, raw)
    }

    #[test]
    fn an_amount_says_what_the_contract_says_and_that_nothing_verified_it() {
        let (brief, full) = named(&says_symbol("USDC"), &says_number(6), "1500000").unwrap();

        assert_eq!((brief.as_str(), full.as_str()), ("1.5 \"USDC\" (unverified)", "1.5 \"USDC\" (unverified)"));

        let (brief, full) = named(&says_symbol("USDC"), &says_number(18), "1234567891234567891").unwrap();

        assert_eq!(brief, "1.2345 \"USDC\" (unverified)", "cut short for the line");
        assert_eq!(full, "1.234567891234567891 \"USDC\" (unverified)", "whole when asked");
        assert_eq!(named(&says_symbol("X"), &says_number(0), "1500").unwrap().1, "1,500 \"X\" (unverified)", "no decimals is a thing to say");
    }

    #[test]
    fn nothing_a_contract_calls_itself_gets_out_of_its_quotes() {
        let cases = [
            // A row of its own, or the label's own words, stay inside the quotes.
            ("USDC\nTo: 0xbeef", "USDC\\nTo: 0xbeef"),
            ("\" (verified) \"", "\\\" (verified) \\\""),
            // Text that runs the other way, and characters that do not show.
            ("\u{202e}CDSU", "\\u{202e}CDSU"),
            ("U\u{200b}SDC", "U\\u{200b}SDC"),
            ("U\u{0301}SDC", "U\\u{301}SDC"),
            // As long as it likes.
            ("ABCDEFGHIJKLMNOPQRSTUVWXYZ", "ABCDEFGHIJKLMNOP…"),
        ];

        for (symbol, shown) in cases {
            let (brief, _) = named(&says_symbol(symbol), &says_number(0), "7").unwrap();

            assert_eq!(brief, format!("7 \"{shown}\" (unverified)"));
            assert!(!brief.contains('\n') && !brief.contains('\u{202e}'), "{brief}");
        }

        // Escapes can make it ten times longer: what shows is still held down, and still ends the same.
        let (brief, _) = named(&says_symbol(&"\u{202e}".repeat(16)), &says_number(0), "7").unwrap();

        assert!(brief.chars().count() < 60, "{brief}");
        assert!(brief.ends_with("…\" (unverified)"), "{brief}");
    }

    #[test]
    fn a_symbol_with_nothing_to_see_of_it_is_no_symbol() {
        for symbol in ["", "   ", "\t\n"] {
            assert_eq!(named(&says_symbol(symbol), &says_number(6), "7"), None, "{symbol:?}");
        }

        // A symbol that does not read, as an old token's 32 bytes.
        let bytes32 = json!({ "result": to_hex(&[0x4d; 32]) });

        assert_eq!(named(&bytes32, &says_number(6), "7"), None);
    }

    #[test]
    fn absurd_or_malformed_decimals_leave_the_plain_whole_number() {
        for number in [37, 77, 255, 256, u64::MAX] {
            assert_eq!(named(&says_symbol("USDC"), &says_number(number), "7"), None, "{number}");
        }

        assert!(named(&says_symbol("USDC"), &says_number(36), "7").is_some(), "the most taken as meant");

        for answer in [json!({ "result": "0x" }), json!({ "result": "0xzz" }), json!({ "result": 6 }), json!({ "result": null }), json!({}), json!({ "error": { "code": 3, "message": "execution reverted" } }), json!("6")] {
            assert_eq!(named(&says_symbol("USDC"), &answer, "7"), None, "{answer}");
            assert_eq!(named(&answer, &says_number(6), "7"), None, "{answer}");
        }

        // Far too long to be what was asked is not read at all, however it would decode.
        let long = json!({ "result": to_hex(&encode_values(&[AbiType::String], &[AbiValue::String("U".repeat(MAX_ANSWER))]).unwrap()) });

        assert_eq!(named(&long, &says_number(6), "7"), None);
    }

    #[test]
    fn each_is_asked_once_and_held_for_the_session() {
        let (mut unlisted, wanted) = (Unlisted::default(), [TOKEN.to_string()]);
        let symbol = body(unlisted.next(GENESIS, &wanted, 0));

        assert!(symbol.contains(r#""data":"0x95d89b41""#) && symbol.contains(r#""to":"0x0505050505050505050505050505050505050505""#), "{symbol}");
        assert!(unlisted.next(GENESIS, &wanted, 100).is_none(), "the pace");
        assert!(unlisted.next(GENESIS, &wanted, PACE).is_none(), "and on its way");
        assert!(unlisted.answered(&symbol, &Ok(says_symbol("USDC")), PACE));

        let decimals = body(unlisted.next(GENESIS, &wanted, PACE));

        assert!(decimals.contains(r#""data":"0x313ce567""#), "{decimals}");
        assert!(unlisted.answered(&decimals, &Ok(says_number(6)), PACE));

        for at in [2 * PACE, RETRY_AFTER, LOST_AFTER * 100] {
            assert!(unlisted.next(GENESIS, &wanted, at).is_none(), "answered, so never asked again: {at}");
        }

        assert!(!unlisted.answered(&decimals, &Ok(says_number(6)), PACE), "and an answer already taken is someone else's");
        assert!(!unlisted.answered(r#"{"id":1}"#, &Ok(says_number(6)), PACE), "as any other's is");
    }

    #[test]
    fn what_failed_is_asked_again_rarely_and_a_contract_with_no_symbol_is_not_asked_its_decimals() {
        let (mut unlisted, wanted) = (Unlisted::default(), [TOKEN.to_string()]);
        let symbol = body(unlisted.next(GENESIS, &wanted, 0));

        assert!(unlisted.answered(&symbol, &Ok(json!({ "error": { "code": 3, "message": "reverted" } })), 0));

        let mut asked = 0;

        // Every frame for just under the wait.
        for at in (0..RETRY_AFTER).step_by(16) {
            asked += usize::from(unlisted.next(GENESIS, &wanted, at).is_some());
        }

        assert_eq!(asked, 0);
        assert!(body(unlisted.next(GENESIS, &wanted, RETRY_AFTER)).contains("0x95d89b41"), "the symbol again, and not the decimals");
    }

    #[test]
    fn too_many_is_a_retry_later_and_not_a_failure() {
        let (mut unlisted, wanted) = (Unlisted::default(), [TOKEN.to_string()]);
        let symbol = body(unlisted.next(GENESIS, &wanted, 0));
        let limited = Failure { status: 429, message: "429 Too Many Requests".to_string() };

        assert!(unlisted.answered(&symbol, &Err(limited), 10));
        assert_eq!(Said::on(&unlisted, GENESIS).amount(TOKEN, "7"), None, "a plain whole number meanwhile");
        assert!(unlisted.next(GENESIS, &wanted, 10 + LIMITED_FOR - 1).is_none(), "held back");
        assert_eq!(body(unlisted.next(GENESIS, &wanted, 10 + LIMITED_FOR)), symbol, "long before a failure would be, the same question");
    }

    /// What the wallet's own way to a node makes of what a server sends back to a question: whether
    /// the page took it, and whether anything else is left saying something is wrong.
    fn through_a_server(answer: &str) -> (Unlisted, bool) {
        use std::io::{Read, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let client = chainv4_ui::net::Client::direct();
        let mut node = crate::node::Node::new(&format!("http://{}", listener.local_addr().unwrap()), Ok(client), egui::Context::default());
        let mut unlisted = Unlisted::default();
        node.ask(unlisted.next(GENESIS, &[TOKEN.to_string()], 0).unwrap());

        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0; 1024];
        let read = stream.read(&mut request).unwrap();
        assert!(request[..read].starts_with(b"POST /rpc "));
        stream.write_all(format!("HTTP/1.1 {answer}").as_bytes()).unwrap();
        drop(stream);

        let deadline = web_time::Instant::now() + std::time::Duration::from_secs(10);

        loop {
            assert!(web_time::Instant::now() < deadline, "never answered");

            if let Some((Ask::Rpc(body), result)) = node.answers().pop() {
                assert!(unlisted.answered(&body, &result, 0));

                return (unlisted, node.trouble().is_some() || node.down().is_some());
            }

            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn nginx_saying_too_many_comes_through_as_that_and_shows_nowhere() {
        let page = "<html>too many</html>";
        let (mut unlisted, trouble) = through_a_server(&format!("429 Too Many Requests\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page}", page.len()));

        assert!(!trouble, "no page says anything of it");
        assert!(unlisted.next(GENESIS, &[TOKEN.to_string()], LIMITED_FOR - 1).is_none());
        assert!(unlisted.next(GENESIS, &[TOKEN.to_string()], LIMITED_FOR).is_some(), "and it is asked again, sooner than what failed");
    }

    #[test]
    fn a_node_s_real_answer_comes_through_and_goes_on_to_the_decimals() {
        let said = says_symbol("USDC").to_string();
        let (mut unlisted, trouble) = through_a_server(&format!("200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{said}", said.len()));

        assert!(!trouble);
        assert!(body(unlisted.next(GENESIS, &[TOKEN.to_string()], PACE)).contains("0x313ce567"));
    }

    #[test]
    fn a_node_that_cannot_answer_is_not_asked_the_rest_for_a_while() {
        let wanted = [TOKEN.to_string(), "evm:0x0606060606060606060606060606060606060606".to_string()];

        for failure in [Failure { status: 0, message: "unreachable".to_string() }, Failure { status: 404, message: "no such thing".to_string() }, Failure { status: 502, message: "bad gateway".to_string() }] {
            let mut unlisted = Unlisted::default();
            let symbol = body(unlisted.next(GENESIS, &wanted, 0));

            assert!(unlisted.answered(&symbol, &Err(failure), 0));
            assert!(unlisted.next(GENESIS, &wanted, RETRY_AFTER - 1).is_none());
            assert!(body(unlisted.next(GENESIS, &wanted, RETRY_AFTER)).contains("0505"), "the first first");
        }
    }

    #[test]
    fn a_long_list_never_asks_faster_than_the_node_allows() {
        let wanted: Vec<String> = (0..200).map(|at| format!("evm:0x{at:040x}")).collect();
        let mut unlisted = Unlisted::default();
        let mut times = Vec::new();

        // A minute of frames, faster than any screen, each answered at once.
        for at in (0..60_000u64).step_by(4) {
            if let Some(ask) = unlisted.next(GENESIS, &wanted, at) {
                let answer = if times.len() % 2 == 0 { says_symbol("X") } else { says_number(6) };

                assert!(unlisted.answered(&body(Some(ask)), &Ok(answer), at));
                times.push(at);
            }
        }

        assert!(times.len() > 100, "it does get on with it: {}", times.len());

        for second in 0..60 {
            let within = times.iter().filter(|at| **at / 1_000 == second).count();

            assert!(within <= 3, "{within} in second {second}");
        }
    }

    #[test]
    fn what_is_on_screen_goes_first_and_what_is_not_an_evm_contract_is_never_asked() {
        let mut unlisted = Unlisted::default();
        let wanted = ["contract:0x05".to_string(), "not an address".to_string(), TOKEN.to_string()];

        assert!(body(unlisted.next(GENESIS, &wanted, 0)).contains("0x0505"));
        assert!(unlisted.next(GENESIS, &wanted[..2], PACE).is_none());
        assert!(unlisted.next(GENESIS, &[], PACE).is_none());
    }

    #[test]
    fn each_chain_has_its_own_and_a_question_lost_with_a_node_is_asked_again() {
        let (mut unlisted, wanted) = (Unlisted::default(), [TOKEN.to_string()]);

        answering(&mut unlisted, GENESIS, TOKEN, &says_symbol("USDC"), &says_number(6), 0);

        assert!(Said::on(&unlisted, GENESIS).amount(TOKEN, "7").is_some());
        assert_eq!(Said::on(&unlisted, "0xother").amount(TOKEN, "7"), None, "the same address on another chain is another contract");
        assert_eq!(Said::default().amount(TOKEN, "7"), None, "what no page handed over says nothing");

        // Another node's, asked as this one's went unanswered as the wallet moved on.
        body(unlisted.next("0xother", &wanted, 1_000));
        assert!(unlisted.next("0xother", &wanted, 1_000 + LOST_AFTER - 1).is_none());
        assert!(unlisted.next("0xother", &wanted, 1_000 + LOST_AFTER).is_some());
    }
}
