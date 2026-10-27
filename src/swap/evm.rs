//! A swap's leg on an EVM chain, such as Ethereum, Sepolia or Base: contracts/evm/Swap.sol, the
//! same lock, claim, refund and get as chainv4's swap contract, by block.timestamp in seconds. Here
//! are its calls as calldata, what get answers, and the transactions that carry them; reaching the
//! chain, by its JSON-RPC, is the apps'.
//!
//! Swap.sol is deployed through the deterministic deployment proxy most EVM chains have, with no
//! salt, so it sits at one address on each of them: [deployed_address]. Terms still name the contract, so a
//! chain without the proxy works with one deployed anywhere.

use {
    super::{EvmLeg, Lock, LockState, Party},
    crate::crypto::ethereum::{EthereumKind, EthereumTx},
    crate::crypto::secp256k1::keccak,
    crate::structs::hex::{from_hex, to_hex},
    crate::structs::u256::u256,
    r#macro::document,
};

#[document(summary = "Swap.sol's init code, as solc wrote it for Paris: what deploys it.", category = "Swaps")]
pub const SWAP_CODE: &str = include_str!("../../contracts/evm/Swap.bin");

#[document(
    summary = "The deterministic deployment proxy, at one address on most EVM chains: sent a salt and init code, it creates the contract at an address both decide alone, CREATE2's.",
    category = "Swaps",
)]
pub const DEPLOYMENT_PROXY: [u8; 20] = [0x4e, 0x59, 0xb4, 0x48, 0x47, 0xb3, 0x79, 0x57, 0x85, 0x88, 0x92, 0x0c, 0xa7, 0x8f, 0xbf, 0x26, 0xc0, 0xb4, 0x95, 0x6c];

#[document(
    summary = "EVM chains the apps name, by chain id: their names and coins' symbols. Any other works by its id.",
    category = "Swaps",
)]
pub const CHAINS: &[(u64, &str, &str)] = &[
    (1, "Ethereum", "ETH"),
    (11155111, "Sepolia", "ETH"),
    (560048, "Hoodi", "ETH"),
    (8453, "Base", "ETH"),
    (84532, "Base Sepolia", "ETH"),
    (42161, "Arbitrum One", "ETH"),
    (421614, "Arbitrum Sepolia", "ETH"),
    (10, "OP Mainnet", "ETH"),
    (137, "Polygon", "POL"),
    (56, "BNB Smart Chain", "BNB"),
    (43114, "Avalanche", "AVAX"),
    (100, "Gnosis", "xDAI"),
];

#[document(summary = "An EVM chain's name, from CHAINS, or chain and its id.", category = "Swaps")]
pub fn chain_name(chain_id: u64) -> String {
    CHAINS.iter().find(|(id, ..)| *id == chain_id).map_or_else(|| format!("chain {chain_id}"), |(_, name, _)| name.to_string())
}

#[document(summary = "An EVM chain's coin's symbol, from CHAINS, or ETH.", category = "Swaps")]
pub fn symbol(chain_id: u64) -> &'static str {
    CHAINS.iter().find(|(id, ..)| *id == chain_id).map_or("ETH", |(_, _, symbol)| symbol)
}

#[document(summary = "Swap.sol's init code, as bytes.", category = "Swaps")]
pub fn init_code() -> Vec<u8> {
    from_hex(&format!("0x{}", SWAP_CODE.trim())).expect("Swap.bin is hex")
}

#[document(
    summary = "Where Swap.sol sits on every chain it was deployed to through the proxy: CREATE2's address for the proxy, no salt, and its init code.",
    category = "Swaps",
)]
pub fn deployed_address() -> [u8; 20] {
    create2(&DEPLOYMENT_PROXY, &[0; 32], &init_code())
}

#[document(summary = "Where CREATE2 puts a contract, as EIP-1014 says: from its deployer, a salt and its init code.", category = "Swaps")]
pub fn create2(deployer: &[u8; 20], salt: &[u8; 32], init: &[u8]) -> [u8; 20] {
    let hashed = keccak(&[&[0xff][..], deployer, salt, &keccak(init)].concat());

    hashed[12..].try_into().expect("20 bytes")
}

#[document(summary = "What deploys Swap.sol through the proxy: the call's data, no salt then its init code.", category = "Swaps")]
pub fn proxy_deployment() -> Vec<u8> {
    [vec![0; 32], init_code()].concat()
}

#[document(summary = "An EVM address from text: 0x and 40 hex digits.", category = "Swaps")]
pub fn parse_address(text: &str) -> Result<[u8; 20], String> {
    let text = text.trim();

    match text.len() == 42 && text.starts_with("0x") {
        true => from_hex(text).and_then(|bytes| bytes.try_into().ok()).ok_or_else(|| format!("{text} is not hex")),
        false => Err(format!("{text} is not an EVM address: 0x and 40 hex digits")),
    }
}

#[document(summary = "An EVM address as text: 0x and 40 hex digits, lowercase.", category = "Swaps")]
pub fn show_address(address: &[u8; 20]) -> String {
    to_hex(address)
}

#[document(
    summary = "A call to make on an EVM chain: to whom, the coin it sends, its calldata, and what it does, to show before signing.",
    category = "Swaps",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Call {
    pub to: [u8; 20],
    pub value: u256,
    pub input: Vec<u8>,
    pub what: &'static str,
}

impl Call {
    #[document(
        parent = "Call",
        summary = "The call as an EIP-1559 transaction, unsigned: its nonce, gas limit and fees as the chain's RPC suggests them.",
    )]
    pub fn transaction(&self, chain_id: u64, nonce: u64, gas_limit: u64, max_price: u256, tip: u256) -> EthereumTx {
        EthereumTx {
            kind: EthereumKind::DynamicFee,
            chain_id,
            nonce,
            max_price,
            tip,
            gas_limit,
            to: Some(self.to),
            value: self.value,
            input: self.input.clone(),
            access_list: Vec::new(),
            signature: [0; 64],
            parity: 0,
        }
    }
}

fn selector(signature: &str) -> [u8; 4] {
    keccak(signature.as_bytes())[..4].try_into().expect("4 bytes")
}

fn address_word(address: &[u8; 20]) -> [u8; 32] {
    let mut word = [0; 32];
    word[12..].copy_from_slice(address);

    word
}

fn number_word(number: u256) -> [u8; 32] {
    number.to_be_bytes()
}

fn calldata(signature: &str, words: &[[u8; 32]]) -> Vec<u8> {
    [selector(signature).to_vec(), words.concat()].concat()
}

impl EvmLeg {
    #[document(
        parent = "EvmLeg",
        summary = "What locking it takes, from its payer: the coin sent with lock, or for a token, approve for Swap.sol to move it, then lockToken. The deadline is in milliseconds, Swap.sol's in seconds.",
    )]
    pub fn lock_calls(&self, hash: &[u8; 32], deadline: u64) -> Result<Vec<Call>, String> {
        let receiver = self.receiver.ok_or("its receiver is not in yet")?;
        let deadline = number_word(u256::from(deadline / 1_000));

        Ok(match self.token {
            None => vec![Call {
                to: self.contract,
                value: self.amount,
                input: calldata("lock(bytes32,address,uint64)", &[*hash, address_word(&receiver), deadline]),
                what: "Lock a swap",
            }],
            Some(token) => vec![
                Call {
                    to: token,
                    value: u256::ZERO,
                    input: calldata("approve(address,uint256)", &[address_word(&self.contract), number_word(self.amount)]),
                    what: "Let the swap contract move the token",
                },
                Call {
                    to: self.contract,
                    value: u256::ZERO,
                    input: calldata("lockToken(bytes32,address,uint64,address,uint256)", &[*hash, address_word(&receiver), deadline, address_word(&token), number_word(self.amount)]),
                    what: "Lock a swap's token",
                },
            ],
        })
    }

    #[document(parent = "EvmLeg", summary = "Claiming it with the secret: anyone may send it, and it pays the receiver.")]
    pub fn claim_call(&self, secret: &[u8; 32]) -> Call {
        Call { to: self.contract, value: u256::ZERO, input: calldata("claim(bytes32)", &[*secret]), what: "Claim a swap" }
    }

    #[document(parent = "EvmLeg", summary = "Refunding it after its deadline: anyone may send it, and it pays the payer back.")]
    pub fn refund_call(&self, hash: &[u8; 32]) -> Call {
        Call { to: self.contract, value: u256::ZERO, input: calldata("refund(bytes32)", &[*hash]), what: "Refund a swap" }
    }
}

#[document(summary = "Calldata asking Swap.sol where the swap under a hash stands, for eth_call.", category = "Swaps", see_also = "read_lock")]
pub fn get_input(hash: &[u8; 32]) -> Vec<u8> {
    calldata("get(bytes32)", &[*hash])
}

#[document(summary = "What Swap.sol's get answered, as a lock.", category = "Swaps", see_also = "get_input")]
pub fn read_lock(output: &[u8]) -> Result<Lock, String> {
    if output.len() != 7 * 32 {
        return Err("an answer Swap.sol does not give: is the contract Swap.sol?".to_string());
    }

    let word = |index: usize| -> &[u8] { &output[index * 32..(index + 1) * 32] };
    let address = |index: usize| -> [u8; 20] { word(index)[12..].try_into().expect("20 bytes") };
    let small = |index: usize| -> Result<u64, String> {
        let bytes = word(index);
        match bytes[..24].iter().all(|byte| *byte == 0) {
            true => Ok(u64::from_be_bytes(bytes[24..].try_into().expect("8 bytes"))),
            false => Err("a number Swap.sol does not give".to_string()),
        }
    };
    let state = match small(0)? {
        0 => LockState::None,
        1 => LockState::Locked,
        2 => LockState::Claimed,
        3 => LockState::Refunded,
        _ => return Err("a state Swap.sol does not give".to_string()),
    };
    let secret: [u8; 32] = word(6).try_into().expect("32 bytes");

    if state == LockState::None {
        return Ok(Lock { state, recipient: None, token: None, amount: u256::ZERO, deadline: 0, secret: None });
    }

    Ok(Lock {
        state,
        recipient: Some(Party::Evm(address(2))),
        token: Some(address(3)).filter(|token| *token != [0; 20]).map(Party::Evm),
        amount: u256::from_be_bytes(word(4).try_into().expect("32 bytes")),
        deadline: small(5)?.saturating_mul(1_000),
        secret: (state == LockState::Claimed).then_some(secret),
    })
}

#[cfg(test)]
mod tests;
