//! contracts/lua/gateway.lua, the bridges' gateway here: the signature board, wrapped assets made
//! and burned, native ones held and released, with digests as chainv4::bridge makes them.

use {
    super::*,
    crate::bridge::{AssetKind, Transfer},
    crate::registry::federation::Federation,
};

/// This chain's id, and an EVM chain's, as the gateways name them.
const HERE: u64 = 0x12_3456;
const THERE: u64 = 11_155_111;
/// The gateway on the EVM chain, its 20 bytes.
const PEER: [u8; 20] = [0xe1; 20];
/// When a gateway is set up.
const SET_UP: u64 = 1_800_000_000_000;

fn bytes(value: &[u8]) -> TxCallArgumentType {
    TxCallArgumentType::Bytes(value.to_vec())
}

fn number(value: u64) -> TxCallArgumentType {
    TxCallArgumentType::U64(value)
}

fn secret(signer: u8) -> [u8; 32] {
    [signer; 32]
}

struct Bridge {
    world: World,
    gateway: Address,
    here: u64,
}

impl Bridge {
    /// A gateway with its templates and its EVM peer, under a federation of three, two to sign.
    fn new() -> Bridge {
        Bridge::set_up_at(SET_UP)
    }

    /// The same, set up at a time: a chain restarted from the same genesis is the same but for this.
    fn set_up_at(setup_time: u64) -> Bridge {
        Bridge::on_chain(HERE, setup_time)
    }

    fn on_chain(here: u64, setup_time: u64) -> Bridge {
        let federation = Federation {
            signers: (1..=3).map(|signer| crate::crypto::secp256k1::public_key(&secret(signer)).unwrap()).collect(),
            threshold: 2,
            chain_depth: 20,
            evm_depth: 12,
            bitcoin_depth: 6,
        };
        let mut world = World {
            vm: Arc::new(Vm::new(&[Engine::Lua, Engine::Wasm], HashType::default_chain()).unwrap().with_federation(Some(federation)).with_chain_id(here)),
            ..World::new()
        };
        let token = world.lua_code(include_str!("../../../contracts/lua/wrapped.lua"));
        let nft = world.lua_code(include_str!("../../../contracts/lua/nft.lua"));
        let multi = world.lua_code(include_str!("../../../contracts/lua/multi.lua"));
        let gateway = world.lua_code(include_str!("../../../contracts/lua/gateway.lua"));
        world.call_at(DEPLOYER, gateway, "setup", &[address(token), address(nft), address(multi)], 0, setup_time).result.unwrap();
        world.call(gateway, "set_peer", &[number(THERE), bytes(&PEER)]).result.unwrap();
        world.call(gateway, "set_coin", &[TxCallArgumentType::String("Dev".into()), TxCallArgumentType::String("DEV".into())]).result.unwrap();

        Bridge { world, gateway, here }
    }

    /// The gateway as transfers write it: its address and when it was set up.
    fn identity(&self) -> Vec<u8> {
        match &self.read(self.gateway, "identity", &[])[..] {
            [TxCallArgumentType::Bytes(identity)] => identity.clone(),
            [TxCallArgumentType::String(identity)] => identity.clone().into_bytes(),
            other => panic!("{other:?}"),
        }
    }

    /// A transfer from the EVM chain to here.
    fn arriving(&self, nonce: u64, kind: AssetKind, origin: u64, asset: &[u8], amount: u64, recipient: Address) -> Transfer {
        Transfer {
            from: PEER.to_vec(),
            to: self.identity(),
            source: THERE,
            target: HERE,
            nonce,
            kind,
            origin,
            asset: asset.to_vec(),
            id: u256::from(if kind == AssetKind::Nft { 42u64 } else { 0 }),
            amount: u256::from(amount),
            recipient: recipient.to_abi().to_vec(),
            name: "Tether".to_string(),
            symbol: "USDT".to_string(),
            decimals: 6,
            uri: if kind == AssetKind::Nft { "ipfs://ape/42".to_string() } else { String::new() },
        }
    }

    fn attest(&self, transfer: &Transfer, signer: u8) -> Result<Vec<TxCallArgumentType>, String> {
        let signature = transfer.sign(&secret(signer));

        self.world.call(self.gateway, "attest", &transfer.attest_args(&signature)).result.map_err(|err| err.to_string())
    }

    fn read(&self, contract: Address, function: &str, args: &[TxCallArgumentType]) -> Vec<TxCallArgumentType> {
        self.world.call(contract, function, args).result.unwrap()
    }

    /// The transfer the gateway says left at a nonce, as chainv4::bridge writes it.
    fn left(&self, nonce: u64) -> Transfer {
        let values = self.read(self.gateway, "sent", &[number(nonce)]);
        let whole = |value: &TxCallArgumentType| match value {
            TxCallArgumentType::I64(number) => *number as u64,
            other => panic!("{other:?}"),
        };
        let wide = |value: &TxCallArgumentType| match value {
            TxCallArgumentType::U256(number) => *number,
            other => panic!("{other:?}"),
        };
        let raw = |value: &TxCallArgumentType| match value {
            TxCallArgumentType::Bytes(bytes) => bytes.clone(),
            TxCallArgumentType::String(text) => text.as_bytes().to_vec(),
            TxCallArgumentType::Nil => Vec::new(),
            other => panic!("{other:?}"),
        };
        let text = |value: &TxCallArgumentType| String::from_utf8(raw(value)).unwrap();

        Transfer {
            from: self.identity(),
            to: PEER.to_vec(),
            source: self.here,
            target: whole(&values[0]),
            nonce,
            kind: AssetKind::from_number(whole(&values[1])).unwrap(),
            origin: whole(&values[2]),
            asset: raw(&values[3]),
            id: wide(&values[4]),
            amount: wide(&values[5]),
            recipient: raw(&values[6]),
            name: text(&values[7]),
            symbol: text(&values[8]),
            decimals: whole(&values[9]) as u8,
            uri: text(&values[10]),
        }
    }
}

#[test]
fn a_deposit_there_mints_its_wrapped_token_here_once_enough_signed() {
    let bridge = Bridge::new();
    let alice = Address::Ed25519([0xa; 32]);
    // Not UTF-8, so it comes back as bytes, not text.
    let usdt = [0x9e; 20];
    let deposit = bridge.arriving(1, AssetKind::Token, THERE, &usdt, 500, alice);
    let wrapped = |bridge: &Bridge| bridge.read(bridge.gateway, "wrapped_of", &[number(THERE), number(1), bytes(&usdt)]);

    // One signer is not enough; the same one twice is still one; a stranger counts for nothing.
    bridge.attest(&deposit, 1).unwrap();
    bridge.attest(&deposit, 1).unwrap();

    assert_eq!(wrapped(&bridge), [TxCallArgumentType::Nil]);
    assert!(bridge.attest(&deposit, 9).unwrap_err().contains("not signed by a signer"));

    // Signed over another gateway than this one's peer, it is someone else's transfer.
    let forged = Transfer { from: vec![0xbb; 20], ..deposit.clone() };

    assert!(bridge.attest(&forged, 2).unwrap_err().contains("not signed by a signer"));

    // A second signer, and it is minted: wUSDT, as Tether's six decimals.
    bridge.attest(&deposit, 2).unwrap();
    let [TxCallArgumentType::Address(token)] = wrapped(&bridge)[..] else { panic!("made") };

    assert_eq!(bridge.read(token, "balance_of", &[address(alice)]), [TxCallArgumentType::U256(u256::from(500u64))]);
    assert_eq!(bridge.read(token, "symbol", &[]), [TxCallArgumentType::String("wUSDT".into())]);
    assert_eq!(bridge.read(token, "name", &[]), [TxCallArgumentType::String("Wrapped Tether".into())]);
    assert_eq!(bridge.read(bridge.gateway, "received", &[number(THERE), number(1)]), [TxCallArgumentType::Bool(true)]);
    assert_eq!(bridge.read(bridge.gateway, "origin_of", &[address(token)]), [TxCallArgumentType::I64(THERE as i64), TxCallArgumentType::I64(1), bytes(&usdt)]);

    // The third signature is kept, and mints nothing more.
    bridge.attest(&deposit, 3).unwrap();

    assert_eq!(bridge.read(token, "total_supply", &[]), [TxCallArgumentType::U256(u256::from(500u64))]);

    // Only the gateway mints or burns it.
    assert!(bridge.world.call_as(alice, token, "mint", &[address(alice), TxCallArgumentType::U256(u256::from(1u64))]).result.is_err());

    // Sent back, it is burned here, and the transfer names Tether on its own chain: Lua's
    // digest is chainv4::bridge's.
    let back = bridge.world.call_as(alice, bridge.gateway, "send", &[number(THERE), bytes(&[0xb0; 20]), number(1), address(token), number(0), TxCallArgumentType::U256(u256::from(200u64))]);

    assert_eq!(back.result.unwrap(), [TxCallArgumentType::I64(1)]);
    assert_eq!(bridge.read(token, "balance_of", &[address(alice)]), [TxCallArgumentType::U256(u256::from(300u64))]);
    let left = bridge.left(1);

    assert_eq!((left.kind, left.origin, left.asset.as_slice(), left.name.as_str(), left.decimals), (AssetKind::Token, THERE, usdt.as_slice(), "Tether", 6));
    assert_eq!(bridge.read(bridge.gateway, "sent", &[number(1)])[11..], [bytes(&left.digest()), address(alice), TxCallArgumentType::I64(0)], "who sent it, for wallets and refunds, and the height it left at");
    assert_eq!(back.events.last().map(|event| event.data.clone()), Some(vec![TxCallArgumentType::I64(1), TxCallArgumentType::I64(THERE as i64), bytes(&left.digest()), address(alice)]));

    // Signatures over it are kept for a relayer, in the signers' order; one over a transfer that
    // never left is refused.
    bridge.attest(&left, 3).unwrap();
    bridge.attest(&left, 1).unwrap();

    assert_eq!(
        bridge.read(bridge.gateway, "signatures", &[bytes(&left.digest())]),
        [TxCallArgumentType::I64(2), bytes(&left.sign(&secret(1))), bytes(&left.sign(&secret(3)))]
    );
    assert!(bridge.attest(&Transfer { nonce: 2, ..left }, 1).unwrap_err().contains("no such transfer"));
}

#[test]
fn the_coin_and_nfts_leave_held_and_come_back_released() {
    let mut bridge = Bridge::new();
    let (alice, bob) = (Address::Ed25519([0xa; 32]), Address::Ed25519([0xb; 32]));
    bridge.world.fund(alice, 5_000);

    // The coin leaves with the call, held by the gateway, named as the owner set it.
    let sent = bridge.world.call_at(alice, bridge.gateway, "send", &[number(THERE), bytes(&[0xb0; 20]), number(0), TxCallArgumentType::Nil, number(0), TxCallArgumentType::U256(u256::from(1_000u64))], 1_000, 0);

    assert_eq!(sent.result.unwrap(), [TxCallArgumentType::I64(1)]);
    assert_eq!(bridge.world.balance(bridge.gateway), u256::from(1_000u64));
    assert_eq!((bridge.left(1).kind, bridge.left(1).symbol.as_str(), bridge.left(1).asset.len()), (AssetKind::Coin, "DEV", 0));
    let wrong = bridge.world.call_at(alice, bridge.gateway, "send", &[number(THERE), bytes(&[0xb0; 20]), number(0), TxCallArgumentType::Nil, number(0), TxCallArgumentType::U256(u256::from(5u64))], 0, 0);

    assert!(wrong.result.unwrap_err().to_string().contains("as much as amount says"));

    // It comes back as the coin, released to Bob.
    let back = bridge.arriving(1, AssetKind::Coin, HERE, &[], 400, bob);
    bridge.attest(&back, 2).unwrap();
    bridge.attest(&back, 3).unwrap();

    assert_eq!((bridge.world.balance(bob), bridge.world.balance(bridge.gateway)), (u256::from(400u64), u256::from(600u64)));

    // An NFT of a collection here leaves held, with its URI, and comes back to Bob.
    let art = bridge.world.lua_code(include_str!("../../../contracts/lua/nft.lua"));
    bridge.world.call(art, "mint", &[address(alice), TxCallArgumentType::String("ipfs://art/1".into())]).result.unwrap();
    bridge.world.call_as(alice, art, "approve", &[address(bridge.gateway), TxCallArgumentType::U256(u256::from(1u64))]).result.unwrap();
    let sent = bridge.world.call_as(alice, bridge.gateway, "send", &[number(THERE), bytes(&[0xb0; 20]), number(2), address(art), TxCallArgumentType::U256(u256::from(1u64)), number(0)]);

    assert_eq!(sent.result.unwrap(), [TxCallArgumentType::I64(2)]);
    assert_eq!(bridge.read(art, "owner_of", &[TxCallArgumentType::U256(u256::from(1u64))]), [address(bridge.gateway)]);
    assert_eq!((bridge.left(2).uri.as_str(), bridge.left(2).amount), ("ipfs://art/1", u256::from(1u64)));

    let returned = Transfer { id: u256::from(1u64), uri: "ipfs://art/1".into(), ..bridge.arriving(2, AssetKind::Nft, HERE, &art.to_abi(), 1, bob) };
    bridge.attest(&returned, 1).unwrap();
    bridge.attest(&returned, 2).unwrap();

    assert_eq!(bridge.read(art, "owner_of", &[TxCallArgumentType::U256(u256::from(1u64))]), [address(bob)]);
}

#[test]
fn an_nft_from_there_is_minted_here_in_a_wrapped_collection_and_goes_back_burned() {
    let bridge = Bridge::new();
    let alice = Address::Ed25519([0xa; 32]);
    let apes = [0xa9; 20];
    let deposit = bridge.arriving(1, AssetKind::Nft, THERE, &apes, 1, alice);
    bridge.attest(&deposit, 1).unwrap();
    bridge.attest(&deposit, 3).unwrap();
    let [TxCallArgumentType::Address(collection)] = bridge.read(bridge.gateway, "wrapped_of", &[number(THERE), number(2), bytes(&apes)])[..] else { panic!("made") };
    let id = TxCallArgumentType::U256(u256::from(42u64));

    assert_eq!(bridge.read(collection, "owner_of", std::slice::from_ref(&id)), [address(alice)]);
    assert_eq!(bridge.read(collection, "token_uri", std::slice::from_ref(&id)), [TxCallArgumentType::String("ipfs://ape/42".into())]);

    // Back: the gateway takes it, as it may, and burns it.
    bridge.world.call_as(alice, collection, "approve", &[address(bridge.gateway), id.clone()]).result.unwrap();
    bridge.world.call_as(alice, bridge.gateway, "send", &[number(THERE), bytes(&[0xb0; 20]), number(2), address(collection), id.clone(), number(0)]).result.unwrap();

    assert!(bridge.world.call(collection, "owner_of", std::slice::from_ref(&id)).result.is_err(), "burned");
    let left = bridge.left(1);

    assert_eq!((left.kind, left.origin, left.asset.as_slice(), left.uri.as_str()), (AssetKind::Nft, THERE, apes.as_slice(), "ipfs://ape/42"));
}

#[test]
fn the_gateway_goes_by_the_chain_s_own_id_which_no_owner_sets() {
    // Its owner gives it no id at setup, so a chain's id is what it signs under, whichever chain.
    for here in [HERE, HERE + 9] {
        let bridge = Bridge::on_chain(here, SET_UP);
        let sender = Address::Ed25519([0xb; 32]);
        bridge.world.fund(sender, 5);
        bridge.world.call_at(sender, bridge.gateway, "send", &[number(THERE), bytes(&[0xb0; 20]), number(0), TxCallArgumentType::Nil, number(0), TxCallArgumentType::U256(u256::from(5u64))], 5, 0).result.unwrap();
        let left = bridge.left(1);

        assert_eq!((left.source, left.origin), (here, here));
        assert_eq!(bridge.read(bridge.gateway, "sent", &[number(1)])[2], TxCallArgumentType::I64(here as i64));
        assert_eq!(bridge.read(bridge.gateway, "sent", &[number(1)])[11], bytes(&left.digest()), "chain.id() in what it signs");
    }

    let bridge = Bridge::new();
    // A transfer for any other chain than the one it runs on is another pair's.
    let alice = Address::Ed25519([0xa; 32]);
    let wrong = Transfer { target: HERE + 1, ..bridge.arriving(1, AssetKind::Coin, THERE, &[], 5, alice) };

    assert!(bridge.attest(&wrong, 1).unwrap_err().contains("another pair of chains"));
}

#[test]
fn a_restart_from_the_identical_genesis_cannot_mint_again_what_the_old_chain_minted() {
    let alice = Address::Ed25519([0xa; 32]);
    let usdt = [0x9e; 20];
    let old = Bridge::set_up_at(SET_UP);
    let deposit = old.arriving(1, AssetKind::Token, THERE, &usdt, 500, alice);
    old.attest(&deposit, 1).unwrap();
    old.attest(&deposit, 2).unwrap();
    assert_eq!(old.read(old.gateway, "received", &[number(THERE), number(1)]), [TxCallArgumentType::Bool(true)]);

    // The same genesis: the same id, the same gateway address, nothing done yet. All that differs
    // is when it was set up, which no owner chooses.
    let new = Bridge::set_up_at(SET_UP + 86_400_000);
    assert_eq!(new.gateway, old.gateway);
    assert_ne!(new.identity(), old.identity());
    assert_eq!(new.read(new.gateway, "received", &[number(THERE), number(1)]), [TxCallArgumentType::Bool(false)]);

    // The old chain's signatures are public, and Gateway.sol's logs still name the old gateway:
    // anyone posting them finds them over another digest.
    assert!(new.attest(&deposit, 1).unwrap_err().contains("not signed by a signer"));
    assert!(new.attest(&deposit, 2).unwrap_err().contains("not signed by a signer"));
    assert_eq!(new.read(new.gateway, "wrapped_of", &[number(THERE), number(1), bytes(&usdt)]), [TxCallArgumentType::Nil]);
    assert_eq!(new.read(new.gateway, "received", &[number(THERE), number(1)]), [TxCallArgumentType::Bool(false)]);

    // And a signer never gets that far: it is not for this gateway. What left once Gateway.sol was
    // joined to the new one is.
    assert!(!deposit.arrives_at(HERE, &new.identity(), &PEER));
    let since = new.arriving(2, AssetKind::Token, THERE, &usdt, 70, alice);
    assert!(since.arrives_at(HERE, &new.identity(), &PEER));
    assert!(!since.arrives_at(HERE + 1, &new.identity(), &PEER), "another chain's");
    assert!(!since.arrives_at(HERE, &new.identity(), &[0xbb; 20]), "another peer's");
    new.attest(&since, 1).unwrap();
    new.attest(&since, 2).unwrap();
    assert_eq!(new.read(new.gateway, "received", &[number(THERE), number(2)]), [TxCallArgumentType::Bool(true)]);

    // Its transfers out count from 1 again, under a Gateway.sol key the old chain's never used.
    assert_ne!(Transfer { from: new.identity(), ..deposit.clone() }.delivered_key(), Transfer { from: old.identity(), ..deposit }.delivered_key());

    // The owner sets peers, not what the gateway is, once it is set up.
    let token = address(new.gateway);
    assert!(new.world.call_at(DEPLOYER, new.gateway, "setup", &[token.clone(), token.clone(), token], 0, SET_UP).result.unwrap_err().to_string().contains("set up already"));
}

#[test]
fn a_recipient_that_could_never_arrive_is_refused_as_it_is_sent() {
    let bridge = Bridge::new();
    let alice = Address::Ed25519([0xa; 32]);
    bridge.world.fund(alice, 5_000);
    let send = |bridge: &Bridge, recipient: TxCallArgumentType| bridge.world.call_at(alice, bridge.gateway, "send", &[number(THERE), recipient, number(0), TxCallArgumentType::Nil, number(0), TxCallArgumentType::U256(u256::from(1u64))], 1, 0).result;

    for wrong in [&[][..], &[0xb0; 19], &[0xb0; 21], &[0xb0; 33], &[0; 20]] {
        assert!(send(&bridge, bytes(wrong)).unwrap_err().to_string().contains("no address on that chain"), "{wrong:?}");
    }

    assert!(send(&bridge, TxCallArgumentType::Nil).unwrap_err().to_string().contains("no address on that chain"));
    assert_eq!(bridge.read(bridge.gateway, "sent_count", &[]), [TxCallArgumentType::I64(0)], "nothing left");
    send(&bridge, bytes(&[0xb0; 20])).unwrap();

    // A peer that is another chainv4 takes that chain's addresses instead.
    bridge.world.call(bridge.gateway, "set_peer", &[number(77), bytes(&[0xe2; 41])]).result.unwrap();
    let there = |bridge: &Bridge, recipient: &[u8]| bridge.world.call_at(alice, bridge.gateway, "send", &[number(77), bytes(recipient), number(0), TxCallArgumentType::Nil, number(0), TxCallArgumentType::U256(u256::from(1u64))], 1, 0).result;

    assert!(there(&bridge, &[0xb0; 20]).is_err());
    assert!(there(&bridge, &[9; 33]).is_err(), "an unknown kind");
    assert!(there(&bridge, &[0; 33]).is_err(), "nobody");
    there(&bridge, &alice.to_abi()).unwrap();
}

/// A token here whose name and symbol are not text, and whose id's URI is not either.
const UNREADABLE: &str = r#"
local M = {}

function M.name() return "\255\254 bad" end
function M.symbol() return "\237\160\128" end
function M.decimals() return 8 end
function M.transfer_from() return true end

return M
"#;

#[test]
fn a_token_whose_name_is_not_text_crosses_without_it_so_its_transfer_can_be_signed() {
    let mut bridge = Bridge::new();
    let token = bridge.world.lua_code(UNREADABLE);
    let alice = Address::Ed25519([0xa; 32]);
    bridge.world.call_as(alice, bridge.gateway, "send", &[number(THERE), bytes(&[0xb0; 20]), number(1), address(token), number(0), TxCallArgumentType::U256(u256::from(5u64))]).result.unwrap();
    // A signer's tools hold names as text: this reads back, hashes alike, and is taken.
    let left = bridge.left(1);

    assert_eq!((left.name.as_str(), left.symbol.as_str(), left.decimals), ("", "", 8));
    bridge.attest(&left, 1).unwrap();
}

/// A wide number, as the gateway takes and gives amounts and ids.
fn wide(value: u64) -> TxCallArgumentType {
    TxCallArgumentType::U256(u256::from(value))
}

impl Bridge {
    fn attest_void(&self, transfer: &Transfer, signer: u8) -> Result<Vec<TxCallArgumentType>, String> {
        let signature = crate::bridge::sign_digest(&secret(signer), &transfer.void_digest());

        self.world.call(self.gateway, "attest_void", &transfer.attest_void_args(&signature)).result.map_err(|err| err.to_string())
    }

    fn attest_refund(&self, transfer: &Transfer, sender: &[u8], signer: u8) -> Result<Vec<TxCallArgumentType>, String> {
        let signature = crate::bridge::sign_digest(&secret(signer), &transfer.refund_digest(sender));

        self.world.call(self.gateway, "attest_refund", &transfer.attest_refund_args(sender, &signature)).result.map_err(|err| err.to_string())
    }

    /// Who a refund of what left at a nonce pays, and whether it was paid.
    fn refund_of(&self, nonce: u64) -> (TxCallArgumentType, bool) {
        match &self.read(self.gateway, "refund_of", &[number(nonce)])[..] {
            [who, TxCallArgumentType::Bool(paid)] => (who.clone(), *paid),
            other => panic!("{other:?}"),
        }
    }

    /// Enough of the federation's signatures to refund what left at a nonce.
    fn refund(&self, nonce: u64, sender: Address) -> Result<(), String> {
        let (left, sender) = (self.left(nonce), sender.to_abi());
        self.attest_refund(&left, &sender, 1)?;
        self.attest_refund(&left, &sender, 2)?;

        Ok(())
    }
}

#[test]
fn a_void_a_refund_and_their_digests_are_the_gateway_s_as_they_are_rust_s_and_pass_for_nothing_else() {
    let bridge = Bridge::new();
    let alice = Address::Ed25519([0xa; 32]);
    let deposit = bridge.arriving(1, AssetKind::Token, THERE, &[0x9e; 20], 500, alice);
    let digest = bytes(&deposit.digest());
    let sender = [0xc0; 20];

    assert_eq!(bridge.read(bridge.gateway, "void_digest", &[number(THERE), number(HERE), number(1), digest.clone()]), [bytes(&deposit.void_digest())]);
    assert_eq!(bridge.read(bridge.gateway, "refund_digest", &[number(THERE), number(HERE), number(1), digest, bytes(&sender)]), [bytes(&deposit.refund_digest(&sender))]);

    // What was signed for a delivery is no void, and what was signed for a void delivers nothing.
    let delivery = deposit.sign(&secret(1));
    assert!(bridge.world.call(bridge.gateway, "attest_void", &deposit.attest_void_args(&delivery)).result.unwrap_err().to_string().contains("not signed by a signer"));
    let void = crate::bridge::sign_digest(&secret(1), &deposit.void_digest());
    assert!(bridge.world.call(bridge.gateway, "attest", &deposit.attest_args(&void)).result.unwrap_err().to_string().contains("not signed by a signer"));
    assert!(bridge.world.call(bridge.gateway, "attest_refund", &deposit.attest_refund_args(&sender, &void)).result.unwrap_err().to_string().contains("not signed by a signer"));
}

#[test]
fn a_transfer_voided_here_can_never_be_delivered_however_public_its_signatures() {
    let bridge = Bridge::new();
    let alice = Address::Ed25519([0xa; 32]);
    let usdt = [0x9e; 20];
    let deposit = bridge.arriving(1, AssetKind::Token, THERE, &usdt, 500, alice);

    // One signer's word for its delivery is public already, and one's for its void changes nothing.
    bridge.attest(&deposit, 1).unwrap();
    bridge.attest_void(&deposit, 1).unwrap();
    assert_eq!(bridge.read(bridge.gateway, "voided", &[number(THERE), number(1)]), [TxCallArgumentType::Nil]);
    assert!(bridge.attest_void(&deposit, 9).unwrap_err().contains("not signed by a signer"));

    // The threshold, and its nonce is taken. The other delivery signature now does nothing but fail.
    bridge.attest_void(&deposit, 2).unwrap();
    assert_eq!(bridge.read(bridge.gateway, "voided", &[number(THERE), number(1)])[0], bytes(&deposit.digest()));
    assert!(bridge.attest(&deposit, 2).unwrap_err().contains("voided"));
    assert!(bridge.attest(&deposit, 3).unwrap_err().contains("voided"));
    assert_eq!(bridge.read(bridge.gateway, "wrapped_of", &[number(THERE), number(1), bytes(&usdt)]), [TxCallArgumentType::Nil], "nothing was minted");
    assert_eq!(bridge.read(bridge.gateway, "received", &[number(THERE), number(1)]), [TxCallArgumentType::Bool(false)], "and it says it did not arrive");

    // What arrived cannot be voided, and a void that lost the race goes no further.
    let arrived = bridge.arriving(2, AssetKind::Token, THERE, &usdt, 500, alice);
    bridge.attest_void(&arrived, 1).unwrap();
    bridge.attest(&arrived, 1).unwrap();
    bridge.attest(&arrived, 2).unwrap();
    assert!(bridge.attest_void(&arrived, 2).unwrap_err().contains("delivered already"));
    assert_eq!(bridge.read(bridge.gateway, "voided", &[number(THERE), number(2)]), [TxCallArgumentType::Nil]);

    // Coming from over there, what refunds it waits on the board for a relayer, and pays nothing here.
    let sender = [0xc0; 20];
    bridge.attest_refund(&deposit, &sender, 1).unwrap();
    bridge.attest_refund(&deposit, &sender, 2).unwrap();
    assert_eq!(
        bridge.read(bridge.gateway, "signatures", &[bytes(&deposit.refund_digest(&sender))]),
        [TxCallArgumentType::I64(2), bytes(&crate::bridge::sign_digest(&secret(1), &deposit.refund_digest(&sender))), bytes(&crate::bridge::sign_digest(&secret(2), &deposit.refund_digest(&sender)))]
    );
}

#[test]
fn what_left_here_held_is_released_back_to_who_sent_it_once() {
    let mut bridge = Bridge::new();
    let alice = Address::Ed25519([0xa; 32]);
    let there = bytes(&[0xb0; 20]);
    bridge.world.fund(alice, 5_000);
    let balance = |bridge: &Bridge, contract: Address, args: &[TxCallArgumentType]| bridge.read(contract, "balance_of", args);

    // The coin.
    bridge.world.call_at(alice, bridge.gateway, "send", &[number(THERE), there.clone(), number(0), TxCallArgumentType::Nil, number(0), wide(1_000)], 1_000, 0).result.unwrap();
    let before = bridge.world.balance(alice);
    let left = bridge.left(1);

    // One signature pays nothing, nor does one over what it did not record.
    bridge.attest_refund(&left, &alice.to_abi(), 1).unwrap();
    assert_eq!(bridge.world.balance(alice), before);
    assert!(!bridge.refund_of(1).1);
    let mallory = Address::Ed25519([0xd; 32]);
    assert!(bridge.attest_refund(&left, &mallory.to_abi(), 2).unwrap_err().contains("not who sent it"));
    assert!(bridge.attest_refund(&Transfer { amount: u256::from(4_000u64), ..left.clone() }, &alice.to_abi(), 2).unwrap_err().contains("no such transfer"));
    assert!(bridge.attest_refund(&Transfer { nonce: 9, ..left.clone() }, &alice.to_abi(), 2).unwrap_err().contains("no such transfer"));
    assert!(bridge.attest_void(&Transfer { nonce: 9, ..left.clone() }, 2).unwrap_err().contains("no such transfer"));

    // The threshold pays it, and never again.
    bridge.attest_refund(&left, &alice.to_abi(), 2).unwrap();
    assert_eq!((bridge.world.balance(alice), bridge.world.balance(bridge.gateway)), (before + u256::from(1_000u64), u256::ZERO));
    assert_eq!(bridge.refund_of(1), (address(alice), true));
    assert!(bridge.attest_refund(&left, &alice.to_abi(), 3).unwrap_err().contains("refunded already"));
    assert_eq!(bridge.world.balance(alice), before + u256::from(1_000u64));

    // A token.
    let token = bridge.world.lua_code(include_str!("../../../contracts/lua/token.lua"));
    bridge.world.call(token, "transfer", &[address(alice), wide(700)]).result.unwrap();
    bridge.world.call_as(alice, token, "approve", &[address(bridge.gateway), wide(300)]).result.unwrap();
    bridge.world.call_as(alice, bridge.gateway, "send", &[number(THERE), there.clone(), number(1), address(token), number(0), wide(300)]).result.unwrap();
    assert_eq!(balance(&bridge, token, &[address(alice)]), [wide(400)]);
    bridge.refund(2, alice).unwrap();
    assert_eq!(balance(&bridge, token, &[address(alice)]), [wide(700)]);

    // An NFT.
    let art = bridge.world.lua_code(include_str!("../../../contracts/lua/nft.lua"));
    bridge.world.call(art, "mint", &[address(alice), TxCallArgumentType::String("ipfs://art/1".into())]).result.unwrap();
    bridge.world.call_as(alice, art, "approve", &[address(bridge.gateway), wide(1)]).result.unwrap();
    bridge.world.call_as(alice, bridge.gateway, "send", &[number(THERE), there.clone(), number(2), address(art), wide(1), number(0)]).result.unwrap();
    assert_eq!(bridge.read(art, "owner_of", &[wide(1)]), [address(bridge.gateway)]);
    bridge.refund(3, alice).unwrap();
    assert_eq!(bridge.read(art, "owner_of", &[wide(1)]), [address(alice)]);

    // An amount of a multi-token id.
    let items = bridge.world.lua_code(include_str!("../../../contracts/lua/multi.lua"));
    bridge.world.call(items, "mint", &[address(alice), wide(9), wide(50)]).result.unwrap();
    bridge.world.call_as(alice, items, "set_approval_for_all", &[address(bridge.gateway), TxCallArgumentType::Bool(true)]).result.unwrap();
    bridge.world.call_as(alice, bridge.gateway, "send", &[number(THERE), there, number(3), address(items), wide(9), wide(20)]).result.unwrap();
    assert_eq!(balance(&bridge, items, &[address(alice), wide(9)]), [wide(30)]);
    bridge.refund(4, alice).unwrap();
    assert_eq!(balance(&bridge, items, &[address(alice), wide(9)]), [wide(50)]);
}

#[test]
fn what_left_here_burned_is_minted_back_to_who_sent_it_once() {
    let bridge = Bridge::new();
    let alice = Address::Ed25519([0xa; 32]);
    let there = bytes(&[0xb0; 20]);
    let made = |bridge: &Bridge, kind: u64, asset: &[u8]| match bridge.read(bridge.gateway, "wrapped_of", &[number(THERE), number(kind), bytes(asset)])[..] {
        [TxCallArgumentType::Address(made)] => made,
        ref other => panic!("{other:?}"),
    };
    let arrive = |transfer: &Transfer| {
        bridge.attest(transfer, 1).unwrap();
        bridge.attest(transfer, 2).unwrap();
    };

    // A wrapped token, burned as it left, comes back.
    let usdt = [0x9e; 20];
    arrive(&bridge.arriving(1, AssetKind::Token, THERE, &usdt, 500, alice));
    let token = made(&bridge, 1, &usdt);
    bridge.world.call_as(alice, bridge.gateway, "send", &[number(THERE), there.clone(), number(1), address(token), number(0), wide(200)]).result.unwrap();
    assert_eq!(bridge.read(token, "balance_of", &[address(alice)]), [wide(300)]);
    bridge.refund(1, alice).unwrap();
    assert_eq!(bridge.read(token, "balance_of", &[address(alice)]), [wide(500)]);
    assert_eq!(bridge.read(token, "total_supply", &[]), [wide(500)]);

    // A wrapped coin, which left saying kind 0.
    arrive(&bridge.arriving(2, AssetKind::Coin, THERE, &[], 90, alice));
    let coin = made(&bridge, 0, &[]);
    bridge.world.call_as(alice, bridge.gateway, "send", &[number(THERE), there.clone(), number(1), address(coin), number(0), wide(90)]).result.unwrap();
    assert_eq!(bridge.left(2).kind, AssetKind::Coin);
    bridge.refund(2, alice).unwrap();
    assert_eq!(bridge.read(coin, "balance_of", &[address(alice)]), [wide(90)]);

    // A wrapped NFT comes back with its URI, the same id.
    let apes = [0xa9; 20];
    arrive(&bridge.arriving(3, AssetKind::Nft, THERE, &apes, 1, alice));
    let collection = made(&bridge, 2, &apes);
    bridge.world.call_as(alice, collection, "approve", &[address(bridge.gateway), wide(42)]).result.unwrap();
    bridge.world.call_as(alice, bridge.gateway, "send", &[number(THERE), there.clone(), number(2), address(collection), wide(42), number(0)]).result.unwrap();
    assert!(bridge.world.call(collection, "owner_of", &[wide(42)]).result.is_err(), "burned");
    bridge.refund(3, alice).unwrap();
    assert_eq!(bridge.read(collection, "owner_of", &[wide(42)]), [address(alice)]);
    assert_eq!(bridge.read(collection, "token_uri", &[wide(42)]), [TxCallArgumentType::String("ipfs://ape/42".into())]);

    // An amount of a wrapped multi-token id.
    let swords = [0xa7; 20];
    arrive(&Transfer { id: u256::from(5u64), uri: "ipfs://sword".to_string(), ..bridge.arriving(4, AssetKind::Multi, THERE, &swords, 30, alice) });
    let items = made(&bridge, 3, &swords);
    bridge.world.call_as(alice, items, "set_approval_for_all", &[address(bridge.gateway), TxCallArgumentType::Bool(true)]).result.unwrap();
    bridge.world.call_as(alice, bridge.gateway, "send", &[number(THERE), there, number(3), address(items), wide(5), wide(10)]).result.unwrap();
    assert_eq!(bridge.read(items, "balance_of", &[address(alice), wide(5)]), [wide(20)]);
    bridge.refund(4, alice).unwrap();
    assert_eq!(bridge.read(items, "balance_of", &[address(alice), wide(5)]), [wide(30)]);
}

/// A token that will not move what it holds until told, as one paused might.
const STUBBORN: &str = r#"
    local M = {}
    function M.name() return "Stubborn" end
    function M.symbol() return "STB" end
    function M.decimals() return 0 end
    function M.transfer_from(from, to, amount) return true end
    function M.allow() storage.set("allowed", "1") end
    function M.paid() return storage.get("paid") or "nothing" end

    function M.transfer(to, amount)
        if storage.get("allowed") == nil then
            error("it will not move")
        end

        storage.set("paid", to:text() .. ":" .. tostring(amount) .. ":" .. (storage.get("paid") or ""))
        return true
    end

    return M
"#;

#[test]
fn a_refund_that_cannot_be_paid_stays_to_try_again_and_its_sender_alone_says_where_else_it_goes() {
    let mut bridge = Bridge::new();
    let (alice, bob, mallory) = (Address::Ed25519([0xa; 32]), Address::Ed25519([0xb; 32]), Address::Ed25519([0xd; 32]));
    let token = bridge.world.lua_code(STUBBORN);
    bridge.world.call_as(alice, bridge.gateway, "send", &[number(THERE), bytes(&[0xb0; 20]), number(1), address(token), number(0), wide(5)]).result.unwrap();
    let left = bridge.left(1);

    // What takes it to the threshold fails with the payment, and nothing is marked.
    bridge.attest_refund(&left, &alice.to_abi(), 1).unwrap();
    assert!(bridge.attest_refund(&left, &alice.to_abi(), 2).unwrap_err().contains("will not move"));
    assert_eq!(bridge.refund_of(1), (address(alice), false));

    // Only who sent it names another, and what it signs is still who sent it.
    assert!(bridge.world.call_as(mallory, bridge.gateway, "set_refund_to", &[number(1), address(mallory)]).result.unwrap_err().to_string().contains("only who sent it"));
    bridge.world.call_as(alice, bridge.gateway, "set_refund_to", &[number(1), address(bob)]).result.unwrap();
    assert_eq!(bridge.refund_of(1), (address(bob), false));

    // Tried again, it goes, once.
    bridge.world.call(token, "allow", &[]).result.unwrap();
    bridge.attest_refund(&left, &alice.to_abi(), 2).unwrap();
    assert_eq!(bridge.refund_of(1), (address(bob), true));
    assert_eq!(bridge.read(token, "paid", &[]), [TxCallArgumentType::String(format!("{bob}:5:"))]);
    assert!(bridge.attest_refund(&left, &alice.to_abi(), 3).unwrap_err().contains("refunded already"));
    assert_eq!(bridge.read(token, "paid", &[]), [TxCallArgumentType::String(format!("{bob}:5:"))]);
}

#[test]
fn a_refund_begun_under_one_federation_takes_the_threshold_of_the_next() {
    let mut bridge = Bridge::new();
    let alice = Address::Ed25519([0xa; 32]);
    bridge.world.fund(alice, 5_000);
    bridge.world.call_at(alice, bridge.gateway, "send", &[number(THERE), bytes(&[0xb0; 20]), number(0), TxCallArgumentType::Nil, number(0), wide(1_000)], 1_000, 0).result.unwrap();
    let left = bridge.left(1);
    let before = bridge.world.balance(alice);
    bridge.attest_refund(&left, &alice.to_abi(), 1).unwrap();

    // The set rotates, signer 3 alone staying: the old signature stops counting, and so does its signer.
    let next = Federation {
        signers: [3, 4, 5].map(|signer| crate::crypto::secp256k1::public_key(&secret(signer)).unwrap()).to_vec(),
        threshold: 2,
        chain_depth: 20,
        evm_depth: 12,
        bitcoin_depth: 6,
    };
    bridge.world.vm = Arc::new(Vm::new(&[Engine::Lua, Engine::Wasm], HashType::default_chain()).unwrap().with_federation(Some(next)).with_chain_id(HERE));
    assert!(bridge.attest_refund(&left, &alice.to_abi(), 2).unwrap_err().contains("not signed by a signer"));
    bridge.attest_refund(&left, &alice.to_abi(), 3).unwrap();
    assert_eq!(bridge.world.balance(alice), before, "one of the new set");
    bridge.attest_refund(&left, &alice.to_abi(), 4).unwrap();
    assert_eq!(bridge.world.balance(alice), before + u256::from(1_000u64));
}
