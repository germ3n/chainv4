use {
    crate::crypto::hashable::Hashable,
    crate::crypto::keypair::{Keypair, PublicKey, Signature},
    crate::crypto::verifiable::Verifiable,
    crate::crypto::verifiable::VerifyError,
    crate::structs::address::Address,
    crate::structs::amount::Amount,
    crate::registry::directory::Directory,
    crate::registry::multisig::MultisigMember,
    crate::registry::signers::Signer,
    crate::governance::{Change, ChangeVote},
    crate::registry::names::{NameAction, name_problem},
    crate::structs::code::Code,
    crate::structs::hash::Hash,
    crate::structs::u256::{I256Le, U256Le, i256},
    crate::structs::u256::u256,
    crate::structs::vote::SignedVote,
    r#macro::document,
    wincode::SchemaRead,
    wincode::SchemaWrite,
};

#[document(
    summary = "A state change submitted to the chain.",
    category = "Transactions",
    see_also = "Address, Amount",
)]
#[derive(Clone, Debug, PartialEq, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
pub enum Tx {
    #[document(
        summary = "Moves an amount from one address to another.",
        params = {
            from = "The address the amount is taken from.",
            to = "The address the amount is credited to.",
            amount = "How much to move.",
        },
    )]
    #[wincode(tag = 0)]
    Transfer {
        from: Address,
        to: Address,
        amount: Amount,
    },
    #[document(
        summary = "Creates new supply and credits it to an address.",
        params = {
            to = "The address that receives the new supply.",
            amount = "How much to create.",
        },
        see_also = "Tx::Burn",
    )]
    #[wincode(tag = 1)]
    Mint {
        to: Address,
        amount: Amount,
    },
    #[document(
        summary = "Destroys supply held by an address.",
        params = {
            from = "The address the supply is taken from.",
            amount = "How much to destroy.",
        },
    )]
    #[wincode(tag = 2)]
    Burn {
        from: Address,
        amount: Amount,
    },
    #[document(
        summary = "Calls a function on a contract.",
        params = {
            to = "The address of the contract to call.",
            func = "Which function to call: the XXH3-64 of its name, from Hasher::digest.",
            amount = "How much to send to the contract.",
            data = "The data to send to the contract.",
        },
        see_also = "Hasher::digest, TxCallArgumentType",
    )]
    #[wincode(tag = 3)]
    ContractCall {
        to: Address,
        func: Hash,
        amount: Amount,
        data: Vec<TxCallArgumentType>,
    },
    #[document(
        summary = "Deploys a contract. Nodes compile or validate the code themselves; the contract lands at an address derived from the sender and this transaction's nonce.",
        params = { code = "The contract, tagged with the engine that runs it." },
        see_also = "Contracts::deploy, contract_address, Code",
    )]
    #[wincode(tag = 4)]
    Deploy { code: Code },
    #[document(
        summary = "Replaces a contract's code, keeping its address, storage and balance. The contract decides: its check_perms must return true for \"upgrade\" and the sender. Then the new code's migrate runs, if it has one.",
        params = {
            contract = "The contract to upgrade.",
            code = "The new code, for either engine.",
        },
        see_also = "Contracts::upgrade, CHECK_PERMS",
    )]
    #[wincode(tag = 5)]
    Upgrade { contract: Address, code: Code },
    #[document(
        summary = "Bonds funds from the sender to become a finality validator. The funds are locked, the voting key starts voting a few epochs later, and only the sender can unbond and withdraw.",
        params = {
            voter = "The key that will sign the validator's votes. It can stay on the validating node while the sender's key stays offline.",
            amount = "How much to bond: at least the chain's min_bond. Votes weigh by bond.",
        },
        see_also = "Tx::Unbond, Tx::Withdraw, FinalityConfig",
    )]
    #[wincode(tag = 6)]
    Bond { voter: PublicKey, amount: Amount },
    #[document(
        summary = "Stops a validator voting, a few epochs later. Its bond stays locked, and slashable, for withdrawal_delay epochs more.",
        params = { voter = "The validator's voting key. Only the account that bonded may unbond it." },
        see_also = "Tx::Withdraw",
    )]
    #[wincode(tag = 7)]
    Unbond { voter: PublicKey },
    #[document(
        summary = "Pays an unbonded validator's bond back to the account that bonded it, once withdrawal_delay has passed, and forgets the validator.",
        params = { voter = "The validator's voting key." },
    )]
    #[wincode(tag = 8)]
    Withdraw { voter: PublicKey },
    #[document(
        summary = "Reports a voter that signed two conflicting votes. Under FFG its whole bond is taken: a share goes to the sender, the rest is burned. Under authority finality the signer is thrown out of the decided set and the sets already fixed two epochs on.",
        params = {
            first = "One of the votes.",
            second = "The other, by the same voter, conflicting with the first.",
        },
        see_also = "Vote::conflicts",
    )]
    #[wincode(tag = 9)]
    Slash { first: Box<SignedVote>, second: Box<SignedVote> },
    #[document(
        summary = "Approves a new set of signers for authority finality. Only one of the decided signers may send it, from its account. Once change_threshold of them approve the same set for the same version, it is decided, and the checkpoints that follow fix it for epochs activation_delay on.",
        params = {
            version = "The version of the decided set this changes, so an approval never counts toward a later change.",
            signers = "Every signer of the new set.",
            threshold = "How many of them justify a checkpoint: more than half, and at most all.",
            change_threshold = "How many of them change the set after that: more than half, and at most all.",
        },
        see_also = "Signers::approve, AuthorityConfig",
    )]
    #[wincode(tag = 10)]
    ApproveSigners {
        version: u64,
        signers: Vec<Signer>,
        threshold: u32,
        change_threshold: u32,
    },
    #[document(
        summary = "Takes back the sender's approval of a change of signers, so it backs no change. Only one of the decided signers may send it, while its approval still counts.",
        params = { version = "The version of the decided set its approval was for." },
        see_also = "Signers::withdraw, Tx::ApproveSigners",
    )]
    #[wincode(tag = 11)]
    WithdrawApproval { version: u64 },
    #[document(
        summary = "Votes on a change to what genesis set, under the chain's governance, proposing it if nobody has: Yes, No or Abstain, replacing the sender's old vote on it. Only an account governance lets vote may send it: a validator's or signer's account under the electorate, a member's under a council. Once Yes weighs enough and the quorum has voted, the change is decided; once what has not voted No could no longer weigh enough, it is rejected.",
        params = {
            version = "The version of the change's kind this changes: how many changes of that kind were decided before, as GET /governance reports in versions.",
            change = "The change it votes on.",
            vote = "Yes, No or Abstain.",
        },
        see_also = "Changes::vote, GovernanceConfig, Change, ChangeVote",
    )]
    #[wincode(tag = 12)]
    VoteChange { version: u64, change: Change, vote: ChangeVote },
    #[document(
        summary = "Clears the sender's vote on a change, so it has no say on it until it votes again. Only an account governance lets vote may send it, while its vote still counts.",
        params = {
            version = "The kind's version its vote was for.",
            change = "The change it voted on.",
        },
        see_also = "Changes::clear, Tx::VoteChange",
    )]
    #[wincode(tag = 13)]
    ClearVote { version: u64, change: Change },
    #[document(
        summary = "Calls an EVM contract with raw calldata, as Ethereum does, or sends the coin to any address through the EVM. The sender takes part in the EVM at its 20-byte address: a secp256k1 account as itself, any other under its alias.",
        params = {
            to = "The EVM contract, or any account, which then simply receives value.",
            value = "What the call sends, in the coin's smallest units.",
            input = "The calldata: a function selector and ABI-encoded arguments, as Solidity expects.",
        },
        see_also = "Code::Evm, evm_alias",
    )]
    #[wincode(tag = 14)]
    EvmCall { to: Address, value: Amount, input: Vec<u8> },
    #[document(
        summary = "Registers, renews, transfers or changes a name, such as alice.dev, in the chain's name registry: on a chain whose genesis has names.",
        params = {
            name = "The name, without the chain's suffix: alice, or pay.alice for a subname.",
            action = "What to do with it.",
        },
        see_also = "NameAction, Names, NamesConfig",
    )]
    #[wincode(tag = 15)]
    Name { name: String, action: NameAction },
    #[document(
        summary = "Sets the sender's primary name, the one reverse lookups give for its address, or clears it. The name must resolve to the sender, and stops being its primary name once it no longer does.",
        params = { name = "The name, without the chain's suffix, or None to clear it." },
        see_also = "Names::primary, Tx::Name",
    )]
    #[wincode(tag = 16)]
    PrimaryName { name: Option<String> },
    #[document(
        summary = "An Ethereum transaction, as MetaMask, Foundry or ethers signed it: its signed bytes as they are, which the EVM runs as Ethereum would, a call or a create. Its SignedTx carries Signature::Ethereum, the key it recovers as the signer, and the nonce and fee offer its bytes say; verify_on checks them against the bytes, and its chain id against this chain's.",
        params = { raw = "The signed bytes, as eth_sendRawTransaction takes them: a legacy transaction under EIP-155, or one of type 1 or 2." },
        see_also = "EthereumTx, SignedTx::from_ethereum",
    )]
    #[wincode(tag = 17)]
    Ethereum { raw: Vec<u8> },
    #[document(
        summary = "A multisig's own transaction, carried by whoever submits it: its SignedTx's nonce and fee offer are the multisig's, so the multisig is the sender, pays the fee from its balance and takes the nonce; the carrier's key only signs the envelope. It needs threshold of the members: signatures in it over the body (MultisigBody::signing_bytes), and approvals made on chain for the same body, count together. One that falls short is refused before it runs, so nobody but the quorum moves the multisig's nonce.",
        params = {
            account = "The multisig.",
            tx = "What it does, as the multisig: any transaction a key's account sends, but another multisig's, an approval of one, or an Ethereum-signed one.",
            signatures = "Members' signatures over the body, collected off chain; empty when approvals on chain are enough.",
        },
        see_also = "MultisigBody, Tx::ApproveMultisig, Multisigs",
    )]
    #[wincode(tag = 18)]
    Multisig {
        account: Address,
        #[wincode(with = "Nested")]
        tx: Box<Tx>,
        signatures: Vec<MemberSignature>,
    },
    #[document(
        summary = "Makes a new multisig, at multisig_address of the sender and this nonce. The sender pays for it and has no say in it after, unless it is a member.",
        params = {
            members = "Its members, 1 to 20: each a key here, and a Bitcoin xpub and EVM address for the other chains, every member naming those or none.",
            threshold = "How many members each of its transactions needs.",
            testnet = "Whether the members' Bitcoin xpubs are for Bitcoin's test networks rather than Bitcoin.",
        },
        see_also = "MultisigPolicy, multisig_address",
    )]
    #[wincode(tag = 19)]
    CreateMultisig { members: Vec<MultisigMember>, threshold: u32, testnet: bool },
    #[document(
        summary = "Replaces the sending multisig's members and threshold; the address stays and the open proposals go. Only a multisig sends it, through Tx::Multisig, so its quorum decides.",
        params = {
            members = "The new members.",
            threshold = "The new threshold.",
            testnet = "Which network the new members' Bitcoin xpubs are for.",
        },
        see_also = "Tx::Multisig, Multisigs::change",
    )]
    #[wincode(tag = 20)]
    ChangeMultisig { members: Vec<MultisigMember>, threshold: u32, testnet: bool },
    #[document(
        summary = "A member approves a multisig's transaction on chain, from its own account and at its own cost, for the multisig's next nonce: what a Tx::Multisig counts with the signatures it carries. A member backs one proposal at a time; approving another moves its approval. Once any of the multisig's transactions runs, every open proposal is void.",
        params = {
            account = "The multisig.",
            nonce = "The multisig's next nonce, which the transaction is for.",
            fee = "The fee offer the multisig's transaction will run with.",
            tx = "What it does, as the multisig.",
        },
        see_also = "Tx::Multisig, Multisigs::approve",
    )]
    #[wincode(tag = 21)]
    ApproveMultisig {
        account: Address,
        nonce: u64,
        fee: FeeOffer,
        #[wincode(with = "Nested")]
        tx: Box<Tx>,
    },
    #[document(
        summary = "A member takes back its approval of an open proposal of a multisig.",
        params = {
            account = "The multisig.",
            proposal = "The proposal's id, MultisigBody::id.",
        },
        see_also = "Tx::ApproveMultisig",
    )]
    #[wincode(tag = 22)]
    WithdrawMultisig { account: Address, proposal: Hash },
    #[document(
        summary = "Claims a miner's earnings from a mining pool: what a leaf under a root the pool's block committed to says the miner earned, less what it claimed before, paid from the pool's escrow. Anyone may send it, the pool for its miners among them: the coins go to the miner whoever pays the fee.",
        params = {
            height = "The height of a block of the pool's whose root holds the leaf: any one, the newest paying the most.",
            to = "The miner.",
            earned = "All the miner has earned from the pool, as the leaf says.",
            index = "The leaf's place among the root's claims.",
            path = "The Merkle path from the leaf to the root, at most 64 hashes.",
        },
        see_also = "PoolCommitment, Pools::claim, claim_leaf",
    )]
    #[wincode(tag = 23)]
    Claim {
        height: u64,
        to: Address,
        #[wincode(with = "U256Le")]
        earned: u256,
        index: u64,
        path: Vec<Hash>,
    },
    #[document(
        summary = "The directory's admin sets the directory, whole, in force at once: on a chain without governance whose genesis names a directory_admin.",
        params = { directory = "The new directory, checked as genesis's is." },
        see_also = "Genesis::directory_admin, Tx::HandDirectory, Change::Directory",
    )]
    #[wincode(tag = 24)]
    SetDirectory { directory: Directory },
    #[document(
        summary = "The directory's admin hands the role to another address, or with none renounces it, leaving the directory as it is for good.",
        params = { to = "The new admin, any address a multisig's too, or None to renounce." },
        see_also = "Genesis::directory_admin, Tx::SetDirectory",
    )]
    #[wincode(tag = 25)]
    HandDirectory { to: Option<Address> },
    #[document(
        summary = "Several steps under one signature, one nonce and one fee offer, all or nothing: any step failing undoes every step, and the fee is charged as for any failed transaction. The steps share one fuel limit, each drawing on what those before it left. It holds what a user does, a transfer, contract call, EVM call, deploy, upgrade, name, primary name, burn or claim, each sent by the batch's sender. A multisig may send one, through Tx::Multisig.",
        params = { steps = "What it does, in order: 1 to MAX_BATCH_STEPS of them." },
        see_also = "MAX_BATCH_STEPS, batch_deploy_nonce, StepOutcome, Tx::Multisig",
    )]
    #[wincode(tag = 26)]
    Batch {
        #[wincode(with = "BatchSteps")]
        steps: Vec<Tx>,
    },
    #[document(
        summary = "Shows that another chain holds an anchor of one of this chain's checkpoints: in Bitcoin, the transaction whose OP_RETURN carries it and the proof that a block the relay holds, deep enough, includes it; in Ethereum, a transaction carrying it in a block the Ethereum relay holds as finalized, with the signed finality update that finalized it. The chain records the checkpoint in its anchor list and pays the sender the reward when one is due; whoever wrote the anchor, anyone may show it. See docs/anchoring.md.",
        params = { proof = "The anchor's transaction and the proof that the other chain holds it." },
        see_also = "AnchorProof, Payload, AnchorSettings, Anchors",
    )]
    #[wincode(tag = 27)]
    Anchor { proof: crate::structs::tx::AnchorProof },
    #[document(
        summary = "Removes a Lua or WebAssembly contract for good. The contract decides: its check_perms must return true for \"remove\", the sender and the beneficiary. Its balance goes to the beneficiary, its code and every entry of its storage are deleted, each entry for a fee in fuel, it leaves the token and collection registries, and its address is kept as removed, so nothing is ever deployed or cloned there again. EVM contracts are removed by their own code, as Ethereum's are.",
        params = {
            contract = "The contract to remove.",
            beneficiary = "Who gets its balance: any address, a contract's too, which is credited and not called.",
        },
        see_also = "Contracts::remove, Removal, CHECK_PERMS",
    )]
    #[wincode(tag = 28)]
    Remove { contract: Address, beneficiary: Address },
}

#[document(
    summary = "What Tx::Anchor carries, and the chain keeps with each entry so any node can show it to a peer: the proof that Bitcoin, or Ethereum, holds an anchor.",
    category = "Transactions",
    see_also = "Tx::Anchor, BtcAnchorProof, EthAnchorProof",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
pub enum AnchorProof {
    #[document(summary = "An anchor in Bitcoin, proven against the Bitcoin relay.")]
    #[wincode(tag = 0)]
    Bitcoin(BtcAnchorProof),
    #[document(summary = "An anchor in Ethereum, proven against the Ethereum relay's finalized headers and the sync committee that signed their finality.")]
    #[wincode(tag = 1)]
    Ethereum(EthAnchorProof),
}

impl crate::crypto::hashable::Hashable for AnchorProof {
    const TAG_LEN: usize = 1;
}

impl AnchorProof {
    #[document(parent = "AnchorProof", summary = "The chain it proves an anchor in.")]
    pub fn chain(&self) -> crate::params::anchoring::AnchorChain {
        match self {
            AnchorProof::Bitcoin(_) => crate::params::anchoring::AnchorChain::Bitcoin,
            AnchorProof::Ethereum(_) => crate::params::anchoring::AnchorChain::Ethereum,
        }
    }

    #[document(parent = "AnchorProof", summary = "Whether its parts fit the limits a transaction checks before it runs: MAX_ANCHOR_TX bytes for each transaction, MAX_ANCHOR_HEADER for an Ethereum header and MAX_ANCHOR_UPDATE for its finality update, at most MAX_ANCHOR_PATH nodes a path.")]
    pub fn within_limits(&self) -> bool {
        match self {
            AnchorProof::Bitcoin(proof) => proof.raw.len() <= MAX_ANCHOR_TX && proof.coinbase.len() <= MAX_ANCHOR_TX && proof.path.len() <= MAX_ANCHOR_PATH && proof.coinbase_path.len() <= MAX_ANCHOR_PATH,
            AnchorProof::Ethereum(proof) => {
                proof.header.len() <= MAX_ANCHOR_HEADER
                    && proof.tx.len() <= MAX_ANCHOR_TX
                    && proof.update.len() <= MAX_ANCHOR_UPDATE
                    && proof.branch.len() <= MAX_ANCHOR_PATH
                    && proof.ancestry.as_ref().is_none_or(|branch| branch.len() <= MAX_ANCHOR_PATH)
            }
        }
    }
}

#[document(
    summary = "A Bitcoin transaction and the proof a relay block includes it: what AnchorProof::Bitcoin carries.",
    category = "Transactions",
    see_also = "AnchorProof, Tx::Anchor",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct BtcAnchorProof {
    #[document(summary = "The Bitcoin block, as the relay keys it: its header's SHA-256 twice, unreversed.")]
    pub block: [u8; 32],
    #[document(summary = "The transaction, without its witnesses.")]
    pub raw: Vec<u8>,
    #[document(summary = "Its place in the block.")]
    pub index: u64,
    #[document(summary = "Its Merkle path, siblings from its own row up.")]
    pub path: Vec<[u8; 32]>,
    #[document(summary = "The block's coinbase, without its witness.")]
    pub coinbase: Vec<u8>,
    #[document(summary = "The coinbase's Merkle path, as long as the transaction's.")]
    pub coinbase_path: Vec<[u8; 32]>,
}

impl crate::crypto::hashable::Hashable for BtcAnchorProof {}

#[document(
    summary = "An Ethereum transaction carrying an anchor, and the proof that a finalized block holds it: what AnchorProof::Ethereum carries. The header is the block's light client header; the transaction is proven against its execution payload's transactions root, and the header against a finalized header the Ethereum relay holds: the header itself, or, with an ancestry branch, a later one whose state's block_roots hold it. The finality update that finalized that held header comes with it, so a node syncing on its own checks the sync committee's signature itself.",
    category = "Transactions",
    see_also = "AnchorProof, Tx::Anchor",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct EthAnchorProof {
    #[document(summary = "The light client header of the block holding the transaction, SSZ-encoded as beacon nodes serve it.")]
    pub header: Vec<u8>,
    #[document(summary = "The beacon block root of the finalized header the relay holds that the proof rests on: the header's own, or, with ancestry, a later one's.")]
    pub root: [u8; 32],
    #[document(summary = "The transaction, its bytes as the execution payload holds them.")]
    pub tx: Vec<u8>,
    #[document(summary = "Its place in the payload.")]
    pub index: u64,
    #[document(summary = "Its SSZ branch to the payload's transactions root: 21 nodes, the last the count.")]
    pub branch: Vec<[u8; 32]>,
    #[document(summary = "The branch from the header's root to the state root of the held header at root, through its block_roots, when the header is an ancestor of it rather than held itself.")]
    pub ancestry: Option<Vec<[u8; 32]>>,
    #[document(summary = "The light client update, SSZ-encoded as the relay takes one, whose finalized header is the one at root, signed by two thirds of its period's sync committee: a finality update reads as one bringing no next committee.")]
    pub update: Vec<u8>,
}

impl crate::crypto::hashable::Hashable for EthAnchorProof {}

#[document(
    summary = "The most bytes an anchor's Bitcoin transaction, or the coinbase with it, or its Ethereum transaction, may take: 100,000, Bitcoin's standard limit for one transaction.",
    category = "Transactions",
    see_also = "Tx::Anchor",
)]
pub const MAX_ANCHOR_TX: usize = 100_000;

#[document(
    summary = "The longest Merkle path an anchor's proof may carry: 32 levels, more than any Bitcoin block has, or any SSZ branch an Ethereum anchor needs.",
    category = "Transactions",
    see_also = "Tx::Anchor",
)]
pub const MAX_ANCHOR_PATH: usize = 32;

#[document(
    summary = "The most bytes an Ethereum anchor's light client header may take: 4,096, several times what one is.",
    category = "Transactions",
    see_also = "Tx::Anchor, EthAnchorProof",
)]
pub const MAX_ANCHOR_HEADER: usize = 4_096;

#[document(
    summary = "The most bytes an Ethereum anchor's finality update may take: 65,536, room for one bringing a next sync committee, about 27 KiB.",
    category = "Transactions",
    see_also = "Tx::Anchor, EthAnchorProof",
)]
pub const MAX_ANCHOR_UPDATE: usize = 65_536;

#[document(
    summary = "The longest Merkle path a claim may carry, 64 hashes: enough for a pool root over every claim there could be.",
    category = "Transactions",
    see_also = "Tx::Claim",
)]
pub const MAX_CLAIM_PATH: usize = 64;

#[document(
    summary = "The most steps a batch may hold, 32. More does not decode, and a transaction holding more never verifies.",
    category = "Transactions",
    see_also = "Tx::Batch",
)]
pub const MAX_BATCH_STEPS: usize = 32;

#[document(
    summary = "The nonce a deploy in a batch is made at, which is what its address is worked out from: the batch's own for its first deploy, so one deploy lands where it would alone, and for each later one a number no account's nonce ever reaches, with the top bit set, then the batch's nonce, then how many deploys came before it. Every engine's address comes from it as from any nonce: contract_address for a native contract, create_address for an EVM one.",
    params = {
        nonce = "The batch's nonce.",
        before = "How many of the batch's steps before this one are deploys, of either engine.",
    },
    category = "Transactions",
    see_also = "Tx::Batch, contract_address, create_address",
)]
pub const fn batch_deploy_nonce(nonce: u64, before: u32) -> u64 {
    match before {
        0 => nonce,
        before => (1 << 63) | (nonce << 5) | (before as u64 & 31),
    }
}

// How deep the transactions being decoded on this thread are: bytes choose how deep they nest, and
// each level is a stack frame, so it is the decoder that must stop them.
thread_local! {
    static NESTING: std::cell::Cell<u8> = const { std::cell::Cell::new(0) };
}

/// The deepest a transaction nests when it decodes: a multisig's batch's steps, two below the
/// outermost. The deepest any valid one goes, and what verifying refuses of them it refuses with
/// the transaction in hand, its depth bounded.
const MAX_NESTING: u8 = 2;

/// One level of nesting being decoded, given back however decoding it ends.
struct Nesting;

impl Nesting {
    fn enter() -> wincode::ReadResult<Nesting> {
        NESTING.with(|depth| match depth.get() < MAX_NESTING {
            true => {
                depth.set(depth.get() + 1);

                Ok(Nesting)
            }
            false => Err(wincode::error::ReadError::Custom("a transaction nested too deep")),
        })
    }
}

impl Drop for Nesting {
    fn drop(&mut self) {
        NESTING.with(|depth| depth.set(depth.get().saturating_sub(1)));
    }
}

/// Encodes a transaction inside another, as a multisig's: its own encoding, with no size known
/// ahead, which a type holding itself could not tell without asking itself.
pub struct Nested;

// SAFETY: size_of is exactly what write writes: the transaction's own encoding.
unsafe impl<C: wincode::config::Config> SchemaWrite<C> for Nested {
    type Src = Box<Tx>;

    fn size_of(src: &Box<Tx>) -> wincode::WriteResult<usize> {
        <Tx as SchemaWrite<C>>::size_of(src)
    }

    fn write(writer: impl wincode::io::Writer, src: &Box<Tx>) -> wincode::WriteResult<()> {
        <Tx as SchemaWrite<C>>::write(writer, src)
    }
}

// SAFETY: read initializes dst before it returns Ok, from the transaction's own encoding.
unsafe impl<'de, C: wincode::config::Config> SchemaRead<'de, C> for Nested {
    type Dst = Box<Tx>;

    fn read(reader: impl wincode::io::Reader<'de>, dst: &mut std::mem::MaybeUninit<Box<Tx>>) -> wincode::ReadResult<()> {
        let _nesting = Nesting::enter()?;

        dst.write(Box::new(<Tx as SchemaRead<'de, C>>::get(reader)?));

        Ok(())
    }
}

/// Encodes a batch's steps as a list of transactions, each as Nested does one. A list longer than
/// a batch holds does not decode, whatever its length says, and nothing is set aside for it first.
pub struct BatchSteps;

// SAFETY: size_of is exactly what write writes: the list's length, then each step's own encoding.
unsafe impl<C: wincode::config::Config> SchemaWrite<C> for BatchSteps {
    type Src = Vec<Tx>;

    fn size_of(src: &Vec<Tx>) -> wincode::WriteResult<usize> {
        let mut size = <C::LengthEncoding as wincode::len::SeqLen<C>>::write_bytes_needed(src.len())?;

        for step in src {
            size += <Tx as SchemaWrite<C>>::size_of(step)?;
        }

        Ok(size)
    }

    fn write(mut writer: impl wincode::io::Writer, src: &Vec<Tx>) -> wincode::WriteResult<()> {
        <C::LengthEncoding as wincode::len::SeqLen<C>>::write(writer.by_ref(), src.len())?;

        for step in src {
            <Tx as SchemaWrite<C>>::write(writer.by_ref(), step)?;
        }

        Ok(())
    }
}

// SAFETY: read initializes dst before it returns Ok, from the steps' own encodings.
unsafe impl<'de, C: wincode::config::Config> SchemaRead<'de, C> for BatchSteps {
    type Dst = Vec<Tx>;

    fn read(mut reader: impl wincode::io::Reader<'de>, dst: &mut std::mem::MaybeUninit<Vec<Tx>>) -> wincode::ReadResult<()> {
        let _nesting = Nesting::enter()?;
        let count = <C::LengthEncoding as wincode::len::SeqLen<C>>::read(reader.by_ref())?;

        if count > MAX_BATCH_STEPS {
            return Err(wincode::error::ReadError::Custom("a batch of more steps than one holds"));
        }

        let mut steps = Vec::with_capacity(count);

        for _ in 0..count {
            steps.push(<Tx as SchemaRead<'de, C>>::get(reader.by_ref())?);
        }

        dst.write(steps);

        Ok(())
    }
}

#[document(
    summary = "A member's signature over a multisig's transaction, and the key that made it.",
    category = "Multisig",
    see_also = "Tx::Multisig, MultisigBody::signing_bytes",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct MemberSignature {
    #[document(summary = "The member's key.")]
    pub key: PublicKey,
    #[document(summary = "Its signature over the body's signing bytes.")]
    pub signature: Signature,
}

impl Hashable for MemberSignature {}

impl MemberSignature {
    #[document(
        parent = "MemberSignature",
        summary = "A member signs a multisig's transaction.",
        params = {
            body = "The transaction, with the multisig's nonce and fee offer.",
            keypair = "The member's key.",
            genesis = "The chain's genesis id, so the signature holds on this chain only.",
        },
    )]
    pub fn sign(body: &crate::registry::multisig::MultisigBody, keypair: &Keypair, genesis: &Hash) -> MemberSignature {
        MemberSignature {
            key: keypair.public(),
            signature: keypair.sign(&body.signing_bytes(genesis)),
        }
    }
}

#[document(
    summary = "The type of an argument to a contract call.",
    category = "Transactions",
    see_also = "Tx::ContractCall",
)]
#[derive(Clone, Debug, PartialEq, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
pub enum TxCallArgumentType {
    #[document(summary = "An address.")]
    #[wincode(tag = 0)]
    Address(Address),
    #[document(summary = "An amount.")]
    #[wincode(tag = 1)]
    Amount(Amount),
    #[document(summary = "A vector of bytes.")]
    #[wincode(tag = 2)]
    Bytes(Vec<u8>),
    #[document(summary = "A boolean.")]
    #[wincode(tag = 3)]
    Bool(bool),
    #[document(summary = "A string.")]
    #[wincode(tag = 4)]
    String(String),
    #[document(summary = "A number.")]
    #[wincode(tag = 5)]
    Number(f64),
    #[document(summary = "A signed 64-bit integer.")]
    #[wincode(tag = 6)]
    I64(i64),
    #[document(summary = "An unsigned 64-bit integer.")]
    #[wincode(tag = 7)]
    U64(u64),
    #[document(summary = "A signed 128-bit integer.")]
    #[wincode(tag = 8)]
    I128(i128),
    #[document(summary = "An unsigned 128-bit integer.")]
    #[wincode(tag = 9)]
    U128(u128),
    #[document(summary = "An unsigned 256-bit integer.")]
    #[wincode(tag = 10)]
    U256(#[wincode(with = "U256Le")] u256),
    #[document(summary = "No value: Lua's nil, JSON's null. A function that returns nil returns it, and one that is given it sees nil.")]
    #[wincode(tag = 11)]
    Nil,
    #[document(summary = "A signed 256-bit integer.")]
    #[wincode(tag = 12)]
    I256(#[wincode(with = "I256Le")] i256),
}

impl Verifiable for Tx {
    #[document(
        parent = "Tx",
        summary = "Checks every address and argument inside, that a transfer, mint or burn moves more than zero, that a contract call names its function by XXH3-64, that a name transaction's name and what it sets could be a name's, and that a batch holds 1 to MAX_BATCH_STEPS steps, each of a kind a batch may hold and each checked likewise. A contract call may send zero.",
        see_also = "VerifyError, Hasher::digest",
    )]
    fn verify(&self) -> Result<(), VerifyError> {
        match self {
            Tx::Transfer { from, to, amount } => {
                from.verify()?;
                to.verify()?;
                amount.verify()?;

                nonzero(amount)
            }
            Tx::Mint { to, amount } => {
                to.verify()?;
                amount.verify()?;

                nonzero(amount)
            }
            Tx::EvmCall { to, value, .. } => {
                to.verify()?;
                value.verify()
            }
            Tx::Name { name, action } => {
                if name_problem(name).or_else(|| action.problem()).is_some() {
                    return Err(VerifyError::BadName);
                }

                match action {
                    NameAction::Transfer { to } | NameAction::Subname { owner: to, .. } => to.verify(),
                    NameAction::Target { target: Some(target) } => target.verify(),
                    _ => Ok(()),
                }
            }
            Tx::PrimaryName { name: Some(name) } if name_problem(name).is_some() => Err(VerifyError::BadName),
            Tx::PrimaryName { .. } => Ok(()),
            // Its signed bytes are read and checked with the chain in hand, by SignedTx::verify_on.
            Tx::Ethereum { .. } => Ok(()),
            Tx::Burn { from, amount } => {
                from.verify()?;
                amount.verify()?;

                nonzero(amount)
            }
            Tx::ContractCall {
                to,
                func,
                amount,
                data,
            } => {
                to.verify()?;
                func.verify()?;
                amount.verify()?;
                function_hash(func)?;

                for argument in data {
                    argument.verify()?;
                }

                Ok(())
            }
            // Whether the source compiles is decided when the transaction runs.
            Tx::Deploy { .. } | Tx::Upgrade { .. } => Ok(()),
            Tx::Remove { contract, beneficiary } => {
                contract.verify()?;
                beneficiary.verify()
            }
            Tx::Bond { voter, amount } => {
                voter.verify()?;
                amount.verify()?;

                nonzero(amount)
            }
            Tx::Unbond { voter } | Tx::Withdraw { voter } => voter.verify(),
            Tx::ApproveSigners { signers, .. } => signers.iter().try_for_each(|signer| signer.key.verify()),
            Tx::WithdrawApproval { .. } | Tx::ClearVote { .. } | Tx::SetDirectory { .. } => Ok(()),
            Tx::HandDirectory { to } => to.iter().try_for_each(Verifiable::verify),
            // Whether the consensus fits the chain is decided when the transaction runs; an
            // authority's key is checked here.
            Tx::VoteChange { change: Change::Consensus(consensus), .. } => match consensus {
                crate::consensus::ConsensusConfig::Authority { key, .. } => key.verify(),
                _ => Ok(()),
            },
            // Whether the list is one a chain takes is decided when the transaction runs, and so are
            // name prices and collection standards.
            Tx::VoteChange { change: Change::Bootnodes(_) | Change::Names(_) | Change::TokenFunctions(_) | Change::EvmSpec { .. } | Change::Dex(_) | Change::Anchoring(_) | Change::Collections(_) | Change::Federation(_) | Change::Directory(_) | Change::Fees(_) | Change::Reward(_) | Change::Limits(_) | Change::Finality(_) | Change::SwitchFinality { .. } | Change::Upgrade(_) | Change::Name(_) | Change::Governance(_) | Change::Locks(_) | Change::Engines(_), .. } => Ok(()),
            // The members' signatures are checked when it runs, against the chain's genesis id and
            // the policy in state.
            Tx::Multisig { account, tx, signatures } => {
                multisig_inner(account, tx)?;

                if signatures.len() > crate::registry::multisig::MAX_MEMBERS {
                    return Err(VerifyError::BadMultisig);
                }

                signatures.iter().try_for_each(|signed| signed.key.verify())
            }
            Tx::ApproveMultisig { account, tx, .. } => multisig_inner(account, tx),
            Tx::CreateMultisig { members, .. } | Tx::ChangeMultisig { members, .. } => members.iter().try_for_each(|member| member.key.verify()),
            Tx::WithdrawMultisig { .. } => Ok(()),
            Tx::Anchor { proof } => match proof.within_limits() {
                true => Ok(()),
                false => Err(VerifyError::BadAnchor),
            },
            Tx::Claim { to, path, .. } => {
                to.verify()?;

                match path.len() <= MAX_CLAIM_PATH {
                    true => Ok(()),
                    false => Err(VerifyError::BadClaim),
                }
            }
            Tx::Batch { steps } => {
                if steps.is_empty() || steps.len() > MAX_BATCH_STEPS {
                    return Err(VerifyError::BatchSize);
                }

                if !steps.iter().all(Tx::fits_a_batch) {
                    return Err(VerifyError::BatchStep);
                }

                steps.iter().try_for_each(Verifiable::verify)
            }
            // The signatures are checked when it runs, against the chain's genesis id.
            Tx::Slash { first, second } => {
                if !first.vote.conflicts(&second.vote) {
                    return Err(VerifyError::NotConflicting);
                }

                Ok(())
            }
        }
    }
}

/// A multisig's transaction comes from a multisig, and does what a key's account could: nothing
/// another multisig's, no approval of one, nothing one key signs for Ethereum. A batch of its own
/// takes from nobody else, whether it is sent or only approved.
fn multisig_inner(account: &Address, tx: &Tx) -> Result<(), VerifyError> {
    if !matches!(account, Address::Multisig(_)) || matches!(tx, Tx::Multisig { .. } | Tx::ApproveMultisig { .. } | Tx::WithdrawMultisig { .. } | Tx::Ethereum { .. }) {
        return Err(VerifyError::BadMultisig);
    }

    tx.verify()?;

    match tx {
        Tx::Batch { steps } => steps_from(steps, account),
        _ => Ok(()),
    }
}

/// Whether every step that says whom it takes from takes from this sender.
fn steps_from(steps: &[Tx], sender: &Address) -> Result<(), VerifyError> {
    match steps.iter().all(|step| !matches!(step, Tx::Transfer { from, .. } | Tx::Burn { from, .. } if from != sender)) {
        true => Ok(()),
        false => Err(VerifyError::NotSender),
    }
}

fn nonzero(amount: &Amount) -> Result<(), VerifyError> {
    match amount {
        Amount::U128(0) => Err(VerifyError::ZeroAmount),
        Amount::U256(value) if *value == u256::ZERO => Err(VerifyError::ZeroAmount),
        Amount::U128(_) | Amount::U256(_) => Ok(()),
    }
}

/// Functions are named by the XXH3-64 of their name, so every client computes the same hash. A
/// build without the xxh3 feature cannot check that and rejects every contract call.
fn function_hash(func: &Hash) -> Result<(), VerifyError> {
    match func {
        #[cfg(feature = "xxh3")]
        Hash::Xxh3_64(_) => Ok(()),
        #[allow(unreachable_patterns)]
        other => Err(VerifyError::FunctionHash(other.algorithm())),
    }
}

impl Verifiable for TxCallArgumentType {
    #[document(
        parent = "TxCallArgumentType",
        summary = "Checks address arguments, and that number arguments are finite: NaN has many encodings, which would let the same call go out under different hashes.",
        see_also = "VerifyError::NonFiniteNumber",
    )]
    fn verify(&self) -> Result<(), VerifyError> {
        // Every variant is listed, so a new one has to decide its own rule.
        match self {
            TxCallArgumentType::Address(address) => address.verify(),
            TxCallArgumentType::Amount(amount) => amount.verify(),
            TxCallArgumentType::Number(number) if !number.is_finite() => {
                Err(VerifyError::NonFiniteNumber)
            }
            TxCallArgumentType::Number(_)
            | TxCallArgumentType::Bytes(_)
            | TxCallArgumentType::Bool(_)
            | TxCallArgumentType::String(_)
            | TxCallArgumentType::I64(_)
            | TxCallArgumentType::U64(_)
            | TxCallArgumentType::I128(_)
            | TxCallArgumentType::U128(_)
            | TxCallArgumentType::U256(_)
            | TxCallArgumentType::I256(_)
            | TxCallArgumentType::Nil => Ok(()),
        }
    }
}

impl Hashable for Tx {
    const TAG_LEN: usize = 1;
}

impl Tx {
    /// Whether a batch may hold this kind: what a user does, and nothing with a signature or a
    /// nonce of its own, nothing that changes who validates or governs, and no batch. Every kind is
    /// listed, so a new one has to decide.
    fn fits_a_batch(&self) -> bool {
        match self {
            Tx::Transfer { .. }
            | Tx::ContractCall { .. }
            | Tx::EvmCall { .. }
            | Tx::Deploy { .. }
            | Tx::Upgrade { .. }
            | Tx::Remove { .. }
            | Tx::Name { .. }
            | Tx::PrimaryName { .. }
            | Tx::Burn { .. }
            | Tx::Claim { .. } => true,
            Tx::Mint { .. }
            | Tx::Bond { .. }
            | Tx::Unbond { .. }
            | Tx::Withdraw { .. }
            | Tx::Slash { .. }
            | Tx::ApproveSigners { .. }
            | Tx::WithdrawApproval { .. }
            | Tx::VoteChange { .. }
            | Tx::ClearVote { .. }
            | Tx::Ethereum { .. }
            | Tx::Multisig { .. }
            | Tx::CreateMultisig { .. }
            | Tx::ChangeMultisig { .. }
            | Tx::ApproveMultisig { .. }
            | Tx::WithdrawMultisig { .. }
            | Tx::SetDirectory { .. }
            | Tx::HandDirectory { .. }
            | Tx::Anchor { .. }
            | Tx::Batch { .. } => false,
        }
    }

    #[document(
        parent = "Tx",
        summary = "The steps of the batch this runs or asks to run: its own, a multisig's it carries, or one a member's approval names. None for anything else.",
    )]
    pub fn batch_steps(&self) -> Option<&[Tx]> {
        match self {
            Tx::Batch { steps } => Some(steps),
            Tx::Multisig { tx, .. } | Tx::ApproveMultisig { tx, .. } => tx.batch_steps(),
            _ => None,
        }
    }

    #[document(
        parent = "Tx",
        summary = "What it does, one transaction at a time: a batch's steps, a multisig's transaction's, or the transaction itself.",
    )]
    pub fn what_it_runs(&self) -> &[Tx] {
        match self {
            Tx::Batch { steps } => steps,
            Tx::Multisig { tx, .. } => tx.what_it_runs(),
            tx => std::slice::from_ref(tx),
        }
    }
}

impl TxCallArgumentType {
    #[document(
        parent = "TxCallArgumentType",
        summary = "An argument as written for a person: type:value, such as i64:-5, string:hi, u256:1_000, bytes:0x00ff or address:ed25519:0x…, or nil alone, a number's digits grouped with _ if wanted. What chainv4 send call and the apps take.",
        returns = "The argument, or why the text is not one.",
    )]
    pub fn parse_typed(text: &str) -> Result<Self, String> {
        let whole = |value: &str| u256::from_str_radix(value, 10).map_err(|_| format!("{value} is not a whole amount"));

        if text == "nil" {
            return Ok(Self::Nil);
        }

        let (kind, value) = text
            .split_once(':')
            .ok_or_else(|| format!("{text} needs a type, such as i64:{text}"))?;
        // Numbers may group their digits with `_`, as 1_000_000; strings and bytes keep theirs.
        let grouped = value;
        let value = match kind {
            "amount" | "number" | "i64" | "u64" | "i128" | "u128" | "u256" | "i256" => {
                crate::structs::digits::ungrouped(grouped).ok_or_else(|| format!("{grouped} is not a valid {kind}: a _ goes between two digits"))?
            }
            _ => std::borrow::Cow::Borrowed(grouped),
        };
        let value = value.as_ref();
        let bad = |_| format!("{grouped} is not a valid {kind}");
        let argument = match kind {
            "address" => Self::Address(value.parse().map_err(|err: crate::structs::address::ParseAddressError| err.to_string())?),
            "amount" => {
                let amount = whole(value)?;

                Self::Amount(match u128::try_from(amount) {
                    Ok(narrow) => Amount::U128(narrow),
                    Err(_) => Amount::U256(amount),
                })
            }
            "bytes" => Self::Bytes(crate::structs::hex::from_hex(value).ok_or_else(|| format!("{value} is not hex"))?),
            "bool" => Self::Bool(value.parse().map_err(|_| format!("{value} is not true or false"))?),
            "string" => Self::String(value.to_string()),
            "number" => Self::Number(value.parse().map_err(|_| format!("{value} is not a number"))?),
            "i64" => Self::I64(value.parse().map_err(bad)?),
            "u64" => Self::U64(value.parse().map_err(bad)?),
            "i128" => Self::I128(value.parse().map_err(bad)?),
            "u128" => Self::U128(value.parse().map_err(bad)?),
            "u256" => Self::U256(whole(value)?),
            "i256" => Self::I256(i256::from_str_radix(value, 10).map_err(bad)?),
            other => {
                return Err(format!(
                    "unknown type {other}; use address, amount, bytes, bool, string, number, i64, u64, i128, u128, i256, u256, or nil alone"
                ));
            }
        };

        Ok(argument)
    }
}

impl Hashable for TxCallArgumentType {
    const TAG_LEN: usize = 1;
}

/// Separates transaction signatures from anything else a key might sign.
const SIGNING_DOMAIN: &[u8] = b"chainv4 transaction";

#[document(
    summary = "A transaction as it travels and lands in blocks: the transaction, the signer's next nonce, the signer's public key and their signature over all three and the chain's genesis id, so it holds on one chain alone. The sender is the signer's address.",
    category = "Transactions",
    see_also = "Tx, Keypair::sign, SignedTx::verify_on",
    example = "let signed = SignedTx::sign(tx, accounts.get(&keypair.public().address())?.nonce, FeeOffer::NONE, &keypair, &chain.genesis());",
)]
#[derive(Clone, Debug, PartialEq, SchemaWrite, SchemaRead)]
pub struct SignedTx {
    #[document(summary = "What to do.")]
    pub tx: Tx,
    #[document(summary = "The sender's nonce at the time; each one is usable once, so the transaction cannot be replayed.")]
    pub nonce: u64,
    #[document(summary = "What the sender offers to pay for it. Ignored on a chain without fees.")]
    pub fee: FeeOffer,
    #[document(summary = "Who signed it. Their address is the sender.")]
    pub signer: PublicKey,
    #[document(summary = "The signer's signature over the chain's genesis id, the transaction, nonce, fee offer and signer.")]
    pub signature: Signature,
}

#[document(
    summary = "What a sender offers to pay for a transaction: the most gas it may use, and the most it pays for each. It pays for the gas it uses, and gets the rest back.",
    category = "Transactions",
    see_also = "SignedTx, FeeConfig",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct FeeOffer {
    #[document(summary = "The most gas the transaction may use. Its fee is reserved from the sender up front, at max_price, and what it does not use is refunded.")]
    pub gas_limit: u64,
    #[document(summary = "The most the sender pays per gas: the whole price when senders bid, the cap on base price plus tip under a base fee.")]
    #[wincode(with = "U256Le")]
    pub max_price: u256,
    #[document(summary = "Under a base fee, what the sender adds per gas for the producer, as far as max_price allows. Ignored when senders bid.")]
    #[wincode(with = "U256Le")]
    pub tip: u256,
}

impl Hashable for FeeOffer {}

impl FeeOffer {
    #[document(parent = "FeeOffer", summary = "No offer, for a chain without fees.")]
    pub const NONE: FeeOffer = FeeOffer {
        gas_limit: 0,
        max_price: u256::ZERO,
        tip: u256::ZERO,
    };

    #[document(
        parent = "FeeOffer",
        summary = "The most the transaction can cost: its gas limit at its max price. None when that passes the largest u256.",
    )]
    pub fn reserve(&self) -> Option<u256> {
        u256::from(self.gas_limit).checked_mul(self.max_price)
    }
}

impl SignedTx {
    #[document(
        parent = "SignedTx",
        summary = "Signs a transaction.",
        params = {
            tx = "The transaction.",
            nonce = "The sender's current nonce.",
            fee = "What the sender offers to pay; FeeOffer::NONE on a chain without fees.",
            keypair = "The sender's keys.",
            genesis = "The chain's genesis id, so the signature holds on this chain only.",
        },
    )]
    pub fn sign(tx: Tx, nonce: u64, fee: FeeOffer, keypair: &Keypair, genesis: &Hash) -> SignedTx {
        let signer = keypair.public();
        let signature = keypair.sign(&signing_bytes(&tx, nonce, &fee, &signer, genesis));

        SignedTx {
            tx,
            nonce,
            fee,
            signer,
            signature,
        }
    }

    #[document(
        parent = "SignedTx",
        summary = "The address sending the transaction: the signer's, or for a multisig's own transaction the multisig, whose nonce and fee offer it carries.",
    )]
    pub fn sender(&self) -> Address {
        match &self.tx {
            Tx::Multisig { account, .. } => *account,
            _ => self.signer.address(),
        }
    }

    #[document(
        parent = "SignedTx",
        summary = "The transaction's id: its whole encoding, signature included, hashed with the chain's hash. A signature names its chain, so the same keys sending the same thing on another chain make another id.",
        params = { hash = "The chain's hash, from genesis." },
    )]
    pub fn id(&self, hash: crate::structs::hash::HashType) -> Hash {
        self.hash_with_tag(hash)
    }

    #[document(
        parent = "SignedTx",
        summary = "Its leaf in a block's tx_root: its whole encoding, hashed as a Merkle leaf.",
        params = { hash = "The chain's hash, from genesis." },
        see_also = "Block::tx_root, leaf",
    )]
    pub fn leaf(&self, hash: crate::structs::hash::HashType) -> Hash {
        crate::crypto::merkle::leaf(hash, &self.hashable_bytes_with_tag())
    }
}

fn signing_bytes(tx: &Tx, nonce: u64, fee: &FeeOffer, signer: &PublicKey, genesis: &Hash) -> Vec<u8> {
    let mut bytes = SIGNING_DOMAIN.to_vec();
    bytes.extend_from_slice(&genesis.hashable_bytes_with_tag());
    bytes.extend_from_slice(&tx.hashable_bytes_with_tag());
    bytes.extend_from_slice(&nonce.to_le_bytes());
    bytes.extend_from_slice(&fee.hashable_bytes_with_tag());
    bytes.extend_from_slice(&signer.hashable_bytes_with_tag());

    bytes
}

impl SignedTx {
    #[document(
        parent = "SignedTx",
        summary = "Checks the transaction itself, the signer's key, the signature, and that a transfer or burn, or any batch's step that is one, only takes from the sender. Every signature names its chain, so one signed for another chain does not hold here: chainv4's own over the genesis id and the transaction, an EIP-712 one over the transaction's typed hash under this chain's domain (crypto::eip712), and an Ethereum one by its chain id. Nothing that holds a batch holds under EIP-712, which would show its signer an opaque hash. Whether the nonce is the next one depends on chain state, so the executor checks that. Every node's check of a transaction is this one, and there is no check of a signature without a chain.",
        params = { genesis = "The chain's genesis id, which the signature is bound to." },
        see_also = "Tx::verify, VerifyError::NotSender",
    )]
    pub fn verify_on(&self, genesis: &crate::structs::hash::Hash) -> Result<(), VerifyError> {
        #[cfg(feature = "secp256k1")]
        if let (Signature::Ethereum, Tx::Ethereum { raw }) = (&self.signature, &self.tx) {
            // What the bytes say, signer and all, must be what the transaction says, on this chain.
            let held = SignedTx::from_ethereum(raw.clone()).map_err(|_| VerifyError::BadEthereum)?;
            let ethereum = crate::crypto::ethereum::decode(raw).map_err(|_| VerifyError::BadEthereum)?;

            if held != *self || ethereum.chain_id != crate::crypto::ethereum::chain_id(genesis) {
                return Err(VerifyError::BadEthereum);
            }

            return self.signer.verify();
        }

        #[cfg(feature = "secp256k1")]
        if let Signature::Secp256k1Eip712(signature) = &self.signature {
            if self.tx.batch_steps().is_some() {
                return Err(VerifyError::BatchNotTyped);
            }

            self.tx.verify()?;
            self.signer.verify()?;
            let key = match &self.signer {
                PublicKey::Secp256k1(key) => key,
                #[allow(unreachable_patterns)]
                _ => return Err(VerifyError::InvalidSignature),
            };
            let digest = crate::crypto::eip712::transaction_digest(&self.tx, self.nonce, &self.fee, genesis);

            if !crate::crypto::secp256k1::verify_hash(key, &digest, signature) {
                return Err(VerifyError::InvalidSignature);
            }

            return self.check_sender();
        }

        // An Ethereum-signed transaction holds only as the first check above takes it.
        if matches!(self.tx, Tx::Ethereum { .. }) {
            return Err(VerifyError::BadEthereum);
        }

        self.tx.verify()?;
        self.signer.verify()?;
        self.signer.verify_signature(&signing_bytes(&self.tx, self.nonce, &self.fee, &self.signer, genesis), &self.signature)?;

        self.check_sender()
    }

    #[document(
        parent = "SignedTx",
        summary = "Signs a transaction in its EIP-712 typed form for this chain, as MetaMask or a hardware wallet signs it: what chainv4 send --eip712 does with a key of its own.",
        params = {
            tx = "What to do.",
            nonce = "The sender's next nonce.",
            fee = "What the sender offers to pay.",
            keypair = "The sender's keys: secp256k1, Ethereum's, alone sign typed data.",
            genesis = "The chain's genesis id, which the signature is bound to.",
        },
        returns = "The signed transaction, or None for a key of another scheme, or for a batch, which holds under chainv4's own signature alone.",
    )]
    #[cfg(feature = "secp256k1")]
    pub fn sign_eip712(tx: Tx, nonce: u64, fee: FeeOffer, keypair: &Keypair, genesis: &crate::structs::hash::Hash) -> Option<SignedTx> {
        if tx.batch_steps().is_some() {
            return None;
        }

        let secret = match keypair.private() {
            crate::crypto::keypair::PrivateKey::Secp256k1(secret) => secret,
            #[allow(unreachable_patterns)]
            _ => return None,
        };
        let digest = crate::crypto::eip712::transaction_digest(&tx, nonce, &fee, genesis);

        Some(SignedTx {
            tx,
            nonce,
            fee,
            signer: keypair.public(),
            signature: Signature::Secp256k1Eip712(crate::crypto::secp256k1::sign_hash(secret, &digest)),
        })
    }

    #[document(
        parent = "SignedTx",
        summary = "An Ethereum transaction as chainv4 carries it, from its signed bytes: Tx::Ethereum with them, the nonce and fee offer they say, the key their signature recovers as the signer, and Signature::Ethereum. An EIP-1559 one offers its max fee per gas and its priority fee as the tip; the others, their gas price as both.",
        returns = "The transaction, or why the bytes are not one: they do not read, or their signature recovers no key.",
        see_also = "Tx::Ethereum, EthereumTx",
    )]
    #[cfg(feature = "secp256k1")]
    pub fn from_ethereum(raw: Vec<u8>) -> Result<SignedTx, String> {
        let ethereum = crate::crypto::ethereum::decode(&raw)?;
        let key = ethereum.signer().ok_or("its signature recovers no key, or has a high s")?;

        Ok(SignedTx {
            tx: Tx::Ethereum { raw },
            nonce: ethereum.nonce,
            fee: FeeOffer {
                gas_limit: ethereum.gas_limit,
                max_price: ethereum.max_price,
                tip: ethereum.tip,
            },
            signer: PublicKey::Secp256k1(key),
            signature: Signature::Ethereum,
        })
    }

    /// Whether a transfer or burn takes from its sender alone, a batch's steps as much as any.
    fn check_sender(&self) -> Result<(), VerifyError> {
        let tx = match &self.tx {
            Tx::Multisig { tx, .. } => tx,
            tx => tx,
        };

        match tx {
            Tx::Transfer { from, .. } | Tx::Burn { from, .. } if *from != self.sender() => Err(VerifyError::NotSender),
            Tx::Batch { steps } => steps_from(steps, &self.sender()),
            _ => Ok(()),
        }
    }

    #[document(
        parent = "SignedTx",
        summary = "Why a batch, its own or the one a multisig's transaction carries, could not run as its sender's: what verify refuses of it, signature aside, for what checks one before any is made, as a dry run and a signer do.",
        returns = "The rule it breaks; None for one that breaks none, or for anything but a batch.",
    )]
    pub fn batch_problem(&self) -> Option<VerifyError> {
        let batch = match &self.tx {
            Tx::Batch { .. } => &self.tx,
            Tx::Multisig { tx, .. } if matches!(**tx, Tx::Batch { .. }) => tx,
            _ => return None,
        };

        batch.verify().err().or_else(|| self.check_sender().err())
    }
}

impl Hashable for SignedTx {}

#[cfg(test)]
mod tests;
