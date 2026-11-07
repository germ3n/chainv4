use {
    crate::structs::hash::HashType,
    r#macro::document,
    std::fmt,
};

#[document(
    summary = "A value that can check it follows the chain's rules by itself, without looking at chain state. Run it on anything received from outside before acting on it.",
    category = "Crypto",
    see_also = "VerifyError, Tx",
    note = "Only what the value says about itself is checked. Balances, nonces and signatures need chain state and are checked elsewhere.",
)]
pub trait Verifiable {
    #[document(
        parent = "Verifiable",
        summary = "Checks the value and everything inside it.",
        returns = "The first rule the value breaks, if any.",
    )]
    fn verify(&self) -> Result<(), VerifyError>;
}

#[document(
    summary = "Why a value failed verification.",
    category = "Crypto",
    see_also = "Verifiable",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerifyError {
    #[document(summary = "An address whose bytes are not a usable Ed25519 public key: not a point on the curve, or a weak small-order point that would accept one signature for almost any message.")]
    InvalidPublicKey,
    #[document(summary = "A transfer, mint or burn of nothing.")]
    ZeroAmount,
    #[document(
        summary = "A contract call whose function hash is not XXH3-64.",
        params = { "0" = "The algorithm it used instead." },
    )]
    FunctionHash(HashType),
    #[document(summary = "A number argument that is NaN or infinite.")]
    NonFiniteNumber,
    #[document(summary = "A signature that does not match its key and message, or was made with a different scheme than the key.")]
    InvalidSignature,
    #[document(summary = "A transfer or burn taking from an address other than the signer's.")]
    NotSender,
    #[document(summary = "A vote whose target does not come after its source.")]
    BadVote,
    #[document(summary = "Slashing evidence that is not two different, conflicting votes by the same voter.")]
    NotConflicting,
    #[document(summary = "A name transaction whose name, or what it sets, could never be: see name_problem and NameAction::problem.")]
    BadName,
    #[document(summary = "An Ethereum-signed transaction that does not read, is for another chain, or whose nonce, fee offer or signer are not what its signed bytes say.")]
    BadEthereum,
    #[document(summary = "A multisig's transaction that is not one: from an address that is no multisig, carrying another multisig's, an approval or an Ethereum-signed transaction, or more signatures than members.")]
    BadMultisig,
    #[document(summary = "A claim with a Merkle path longer than any root's: more than 64 hashes.")]
    BadClaim,
    #[document(summary = "An anchor's proof longer than any Bitcoin transaction or Merkle path it could show.")]
    BadAnchor,
    #[document(summary = "A batch with no steps, or more than MAX_BATCH_STEPS.")]
    BatchSize,
    #[document(summary = "A batch holding what a batch may not: anything but a transfer, contract call, EVM call, deploy, upgrade, name, primary name, burn or claim.")]
    BatchStep,
    #[document(summary = "A multisig's batch signed as EIP-712 typed data, which could only show its signer an opaque hash.")]
    BatchNotTyped,
    #[document(summary = "Function names that are not the ones a typed signature's calls hold: too few, too many, one that hashes to another function, or any beside a signature of another kind.")]
    FunctionNames,
}

impl fmt::Display for VerifyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VerifyError::InvalidPublicKey => write!(f, "address is not a valid Ed25519 public key"),
            VerifyError::ZeroAmount => write!(f, "amount must be greater than zero"),
            VerifyError::FunctionHash(algorithm) => {
                write!(f, "function hash must be Xxh3_64, not {algorithm:?}")
            }
            VerifyError::NonFiniteNumber => write!(f, "number argument must be finite"),
            VerifyError::InvalidSignature => write!(f, "signature does not match"),
            VerifyError::NotSender => write!(f, "only the sender's own funds can move"),
            VerifyError::BadVote => write!(f, "a vote's target must come after its source"),
            VerifyError::NotConflicting => write!(f, "the votes are not conflicting votes by one voter"),
            VerifyError::BadName => write!(f, "not a name, or not what a name holds: labels of a to z, 0 to 9 and hyphens, the last at least 3 long"),
            VerifyError::BadEthereum => write!(f, "not an Ethereum transaction for this chain, as signed"),
            VerifyError::BadMultisig => write!(f, "not a multisig's transaction a multisig may send"),
            VerifyError::BadClaim => write!(f, "a claim's path is longer than any pool's root needs"),
            VerifyError::BadAnchor => write!(f, "an anchor's transaction, coinbase or path is longer than any Bitcoin block holds"),
            VerifyError::BatchSize => write!(f, "a batch holds 1 to {} steps", crate::structs::tx::MAX_BATCH_STEPS),
            VerifyError::BatchStep => write!(f, "a batch holds only transfers, contract calls, EVM calls, deploys, upgrades, names, primary names, burns and claims"),
            VerifyError::BatchNotTyped => write!(f, "a multisig's batch cannot be signed as EIP-712 typed data: its signer would see an opaque hash, not its steps"),
            VerifyError::FunctionNames => write!(f, "the function names are not the ones its typed signature's calls hold, or stand beside a signature of another kind"),
        }
    }
}

impl std::error::Error for VerifyError {}

#[cfg(test)]
mod tests {
    use {
        super::Verifiable,
        super::VerifyError,
        crate::structs::address::Address,
        crate::structs::amount::Amount,
        crate::structs::tx::Tx,
        crate::structs::u256::u256,
        ed25519_dalek::SigningKey,
        ed25519_dalek::VerifyingKey,
    };
    #[cfg(any(feature = "xxh3", feature = "sha256"))]
    use {
        crate::crypto::hasher::Hasher,
        crate::structs::hash::Hash,
        crate::structs::hash::HashType,
        crate::structs::tx::TxCallArgumentType,
    };

    /// A real public key, from a fixed secret.
    fn address(seed: u8) -> Address {
        Address::Ed25519(SigningKey::from_bytes(&[seed; 32]).verifying_key().to_bytes())
    }

    /// The identity point: on the curve, but of small order.
    fn weak_address() -> Address {
        let mut identity = [0; 32];
        identity[0] = 1;

        Address::Ed25519(identity)
    }

    fn transfer(amount: u128) -> Tx {
        Tx::Transfer {
            from: address(1),
            to: address(2),
            amount: Amount::U128(amount),
        }
    }

    /// Sends nothing, which a contract call is allowed to do.
    #[cfg(any(feature = "xxh3", feature = "sha256"))]
    fn call(func: Hash, data: Vec<TxCallArgumentType>) -> Tx {
        Tx::ContractCall {
            to: address(3),
            func,
            amount: Amount::U128(0),
            data,
        }
    }

    #[test]
    fn real_keys_pass() {
        assert_eq!(address(1).verify(), Ok(()));
        assert_eq!(transfer(5).verify(), Ok(()));
    }

    #[test]
    fn keys_off_the_curve_fail() {
        // About half of all 32-byte strings are not points on the curve.
        let off_curve = (0..=255u8)
            .map(|byte| [byte; 32])
            .find(|bytes| VerifyingKey::from_bytes(bytes).is_err())
            .unwrap();

        assert_eq!(
            Address::Ed25519(off_curve).verify(),
            Err(VerifyError::InvalidPublicKey)
        );
    }

    #[test]
    fn weak_keys_fail() {
        assert_eq!(weak_address().verify(), Err(VerifyError::InvalidPublicKey));

        let mint = Tx::Mint {
            to: weak_address(),
            amount: Amount::U128(1),
        };

        assert_eq!(mint.verify(), Err(VerifyError::InvalidPublicKey));
    }

    #[test]
    fn moving_zero_fails() {
        let mint = Tx::Mint {
            to: address(1),
            amount: Amount::U128(0),
        };
        let burn = Tx::Burn {
            from: address(1),
            amount: Amount::U128(0),
        };

        assert_eq!(transfer(0).verify(), Err(VerifyError::ZeroAmount));
        assert_eq!(mint.verify(), Err(VerifyError::ZeroAmount));
        assert_eq!(burn.verify(), Err(VerifyError::ZeroAmount));

        let wide = |value| Tx::Transfer {
            from: address(1),
            to: address(2),
            amount: Amount::U256(value),
        };

        assert_eq!(wide(u256::ZERO).verify(), Err(VerifyError::ZeroAmount));
        assert_eq!(wide(u256::MAX).verify(), Ok(()));
    }

    #[cfg(feature = "xxh3")]
    #[test]
    fn contract_calls_with_an_xxh3_function_hash_pass() {
        let func = Hasher::digest(HashType::Xxh3_64, b"transfer");
        let data = vec![
            TxCallArgumentType::Address(address(4)),
            TxCallArgumentType::String("memo".to_string()),
            TxCallArgumentType::Number(1.5),
        ];

        assert_eq!(call(func, data).verify(), Ok(()));
    }

    #[cfg(feature = "sha256")]
    #[test]
    fn contract_calls_with_another_function_hash_fail() {
        let func = Hasher::digest(HashType::Sha256, b"transfer");

        assert_eq!(
            call(func, Vec::new()).verify(),
            Err(VerifyError::FunctionHash(HashType::Sha256))
        );
    }

    #[cfg(feature = "xxh3")]
    #[test]
    fn contract_call_arguments_are_checked() {
        let func = Hasher::digest(HashType::Xxh3_64, b"scale");

        for number in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let data = vec![TxCallArgumentType::Number(number)];

            assert_eq!(call(func, data).verify(), Err(VerifyError::NonFiniteNumber));
        }

        let data = vec![
            TxCallArgumentType::U64(1),
            TxCallArgumentType::Address(weak_address()),
        ];

        assert_eq!(call(func, data).verify(), Err(VerifyError::InvalidPublicKey));
    }
}
