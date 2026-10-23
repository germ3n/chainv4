use {
    crate::structs::hash::Hash,
    crate::structs::hash::HashType,
    r#macro::document,
    wincode::io::WriteResult,
    wincode::io::Writer,
};

#[document(
    summary = "A running hash. Bytes go in a piece at a time and the digest comes out at the end, so whatever is hashed never has to sit in one buffer.",
    category = "Crypto",
    see_also = "HashType, Hash, Hashable",
)]
// BLAKE3 and XXH3 keep large states. Boxing them would allocate on every hash, which is what this
// type exists to avoid.
#[allow(clippy::large_enum_variant)]
#[derive(Clone)]
pub enum Hasher {
    #[cfg(feature = "sha256")]
    Sha256(sha2::Sha256),
    #[cfg(feature = "blake3")]
    Blake3_256(blake3::Hasher),
    #[cfg(feature = "blake2s")]
    Blake2s256(blake2::Blake2s256),
    #[cfg(feature = "blake2b")]
    Blake2b256(blake2::Blake2b256),
    #[cfg(feature = "xxh3")]
    Xxh3_64(xxhash_rust::xxh3::Xxh3Default),
    #[cfg(feature = "keccak256")]
    Keccak256(sha3::Keccak256),
}

impl Hasher {
    #[document(
        parent = "Hasher",
        summary = "Starts an empty hash.",
        params = { algorithm = "The algorithm to hash with." },
    )]
    pub fn new(algorithm: HashType) -> Hasher {
        match algorithm {
            #[cfg(feature = "sha256")]
            HashType::Sha256 => Hasher::Sha256(sha2::Sha256::default()),
            #[cfg(feature = "blake3")]
            HashType::Blake3_256 => Hasher::Blake3_256(blake3::Hasher::new()),
            #[cfg(feature = "blake2s")]
            HashType::Blake2s256 => Hasher::Blake2s256(blake2::Blake2s256::default()),
            #[cfg(feature = "blake2b")]
            HashType::Blake2b256 => Hasher::Blake2b256(blake2::Blake2b256::default()),
            #[cfg(feature = "xxh3")]
            HashType::Xxh3_64 => Hasher::Xxh3_64(xxhash_rust::xxh3::Xxh3Default::new()),
            #[cfg(feature = "keccak256")]
            HashType::Keccak256 => Hasher::Keccak256(sha3::Keccak256::default()),
        }
    }

    #[document(
        parent = "Hasher",
        summary = "Hashes bytes exactly as given, without wincode's encoding. Use it when another program has to get the same hash from the same bytes, such as a contract function selector from the function's name.",
        params = {
            algorithm = "The algorithm to hash with.",
            bytes = "The whole input.",
        },
        returns = "The digest, tagged with its algorithm.",
        example = "let selector = Hasher::digest(HashType::Xxh3_64, b\"transfer\");",
        see_also = "Hashable::hash",
    )]
    pub fn digest(algorithm: HashType, bytes: &[u8]) -> Hash {
        let mut hasher = Hasher::new(algorithm);
        hasher.update(bytes);

        hasher.finalize()
    }

    #[document(
        parent = "Hasher",
        summary = "Feeds more bytes into the hash. Feeding a then b gives the same digest as feeding a and b joined.",
        params = { bytes = "The next piece of input." },
    )]
    pub fn update(&mut self, bytes: &[u8]) {
        match self {
            #[cfg(feature = "sha256")]
            Hasher::Sha256(state) => sha2::Digest::update(state, bytes),
            #[cfg(feature = "blake3")]
            Hasher::Blake3_256(state) => {
                state.update(bytes);
            }
            #[cfg(feature = "blake2s")]
            Hasher::Blake2s256(state) => blake2::Digest::update(state, bytes),
            #[cfg(feature = "blake2b")]
            Hasher::Blake2b256(state) => blake2::Digest::update(state, bytes),
            #[cfg(feature = "xxh3")]
            Hasher::Xxh3_64(state) => state.update(bytes),
            #[cfg(feature = "keccak256")]
            Hasher::Keccak256(state) => sha3::Digest::update(state, bytes),
        }
    }

    #[document(
        parent = "Hasher",
        summary = "Ends the hash.",
        returns = "The digest, tagged with its algorithm.",
    )]
    pub fn finalize(self) -> Hash {
        match self {
            #[cfg(feature = "sha256")]
            Hasher::Sha256(state) => Hash::Sha256(sha2::Digest::finalize(state).into()),
            #[cfg(feature = "blake3")]
            Hasher::Blake3_256(state) => Hash::Blake3_256(*state.finalize().as_bytes()),
            #[cfg(feature = "blake2s")]
            Hasher::Blake2s256(state) => Hash::Blake2s256(blake2::Digest::finalize(state).into()),
            #[cfg(feature = "blake2b")]
            Hasher::Blake2b256(state) => Hash::Blake2b256(blake2::Digest::finalize(state).into()),
            #[cfg(feature = "xxh3")]
            Hasher::Xxh3_64(state) => Hash::Xxh3_64(state.digest()),
            #[cfg(feature = "keccak256")]
            Hasher::Keccak256(state) => Hash::Keccak256(sha3::Digest::finalize(state).into()),
        }
    }
}

/// Lets wincode encode a value straight into the hash.
impl Writer for Hasher {
    fn write(&mut self, src: &[u8]) -> WriteResult<()> {
        self.update(src);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use {
        super::Hasher,
        crate::crypto::hashable::Hashable,
        crate::structs::address::Address,
        crate::structs::amount::Amount,
        crate::structs::hash::Hash,
        crate::structs::hash::HashType,
        crate::structs::tx::Tx,
        crate::structs::tx::TxCallArgumentType,
        std::alloc::{GlobalAlloc, Layout, System},
        std::cell::Cell,
    };

    /// Every algorithm this build has.
    const ALGORITHMS: &[HashType] = &[
        #[cfg(feature = "sha256")]
        HashType::Sha256,
        #[cfg(feature = "blake3")]
        HashType::Blake3_256,
        #[cfg(feature = "blake2s")]
        HashType::Blake2s256,
        #[cfg(feature = "blake2b")]
        HashType::Blake2b256,
        #[cfg(feature = "xxh3")]
        HashType::Xxh3_64,
        #[cfg(feature = "keccak256")]
        HashType::Keccak256,
    ];

    /// Counts this thread's allocations, so tests running in parallel do not disturb each other.
    struct Counting;

    thread_local! {
        static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
    }

    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let _ = ALLOCATIONS.try_with(|count| count.set(count.get() + 1));

            unsafe { System.alloc(layout) }
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            unsafe { System.dealloc(ptr, layout) }
        }
    }

    #[global_allocator]
    static ALLOCATOR: Counting = Counting;

    fn allocations() -> usize {
        ALLOCATIONS.with(Cell::get)
    }

    fn transfer() -> Tx {
        Tx::Transfer {
            from: Address::Ed25519([1; 32]),
            to: Address::Ed25519([2; 32]),
            amount: Amount::U128(500),
        }
    }

    /// A function selector: XXH3 when this build has it, otherwise the last algorithm it does.
    fn selector(name: &[u8]) -> Hash {
        Hasher::digest(ALGORITHMS[ALGORITHMS.len() - 1], name)
    }

    fn contract_call() -> Tx {
        Tx::ContractCall {
            to: Address::Ed25519([3; 32]),
            func: selector(b"transfer"),
            amount: Amount::U128(0),
            data: vec![
                TxCallArgumentType::String("hello".to_string()),
                TxCallArgumentType::Bytes(vec![1, 2, 3]),
                TxCallArgumentType::U64(9),
            ],
        }
    }

    #[test]
    fn matches_the_reference_implementations() {
        #[cfg(feature = "sha256")]
        {
            // FIPS 180-2, "abc".
            let abc = [
                0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d,
                0xae, 0x22, 0x23, 0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10,
                0xff, 0x61, 0xf2, 0x00, 0x15, 0xad,
            ];

            assert_eq!(Hasher::digest(HashType::Sha256, b"abc"), Hash::Sha256(abc));
        }

        #[cfg(feature = "blake3")]
        assert_eq!(
            Hasher::digest(HashType::Blake3_256, b"abc"),
            Hash::Blake3_256(*blake3::hash(b"abc").as_bytes())
        );

        #[cfg(feature = "blake2s")]
        assert_eq!(
            Hasher::digest(HashType::Blake2s256, b"abc"),
            Hash::Blake2s256(<blake2::Blake2s256 as blake2::Digest>::digest(b"abc").into())
        );

        #[cfg(feature = "blake2b")]
        assert_eq!(
            Hasher::digest(HashType::Blake2b256, b"abc"),
            Hash::Blake2b256(<blake2::Blake2b256 as blake2::Digest>::digest(b"abc").into())
        );

        #[cfg(feature = "xxh3")]
        {
            // The xxHash reference value for empty input.
            assert_eq!(
                Hasher::digest(HashType::Xxh3_64, b""),
                Hash::Xxh3_64(0x2D06_8005_38D3_94C2)
            );
            assert_eq!(
                Hasher::digest(HashType::Xxh3_64, b"transfer"),
                Hash::Xxh3_64(xxhash_rust::xxh3::xxh3_64(b"transfer"))
            );
        }

        #[cfg(feature = "keccak256")]
        {
            // Ethereum's hash of empty input, the code hash of every account without code.
            let empty = [
                0xc5, 0xd2, 0x46, 0x01, 0x86, 0xf7, 0x23, 0x3c, 0x92, 0x7e, 0x7d, 0xb2, 0xdc,
                0xc7, 0x03, 0xc0, 0xe5, 0x00, 0xb6, 0x53, 0xca, 0x82, 0x27, 0x3b, 0x7b, 0xfa,
                0xd8, 0x04, 0x5d, 0x85, 0xa4, 0x70,
            ];

            assert_eq!(Hasher::digest(HashType::Keccak256, b""), Hash::Keccak256(empty));

            // The ERC-20 transfer selector is the first four bytes of this hash.
            match Hasher::digest(HashType::Keccak256, b"transfer(address,uint256)") {
                Hash::Keccak256(digest) => assert_eq!(digest[..4], [0xa9, 0x05, 0x9c, 0xbb]),
                #[allow(unreachable_patterns)]
                other => panic!("{other:?}"),
            }

            // SHA3-256 pads differently, so the two never agree.
            let sha3: [u8; 32] = <sha3::Sha3_256 as sha3::Digest>::digest(b"").into();

            assert_ne!(sha3, empty);
        }
    }

    #[test]
    fn pieces_hash_the_same_as_the_whole() {
        // Long enough to cross every algorithm's block and buffer boundaries.
        let data: Vec<u8> = (0..2000u32).map(|index| (index * 31) as u8).collect();

        for &algorithm in ALGORITHMS {
            let mut pieces = Hasher::new(algorithm);

            for chunk in data.chunks(7) {
                pieces.update(chunk);
            }

            assert_eq!(pieces.finalize(), Hasher::digest(algorithm, &data), "{algorithm:?}");
        }

        #[cfg(feature = "xxh3")]
        assert_eq!(
            Hasher::digest(HashType::Xxh3_64, &data),
            Hash::Xxh3_64(xxhash_rust::xxh3::xxh3_64(&data))
        );
    }

    #[test]
    fn streaming_matches_hashing_the_encoded_bytes() {
        for tx in [transfer(), contract_call()] {
            for &algorithm in ALGORITHMS {
                let digest = tx.hash(algorithm);
                let tagged = tx.hash_with_tag(algorithm);

                assert_eq!(digest, Hasher::digest(algorithm, &tx.hashable_bytes()));
                assert_eq!(tagged, Hasher::digest(algorithm, &tx.hashable_bytes_with_tag()));
                assert_eq!(digest.algorithm(), algorithm);
            }
        }
    }

    #[test]
    fn hashing_allocates_nothing() {
        for tx in [transfer(), contract_call()] {
            for &algorithm in ALGORITHMS {
                let before = allocations();
                let digest = tx.hash(algorithm);
                let tagged = tx.hash_with_tag(algorithm);

                assert_eq!(allocations() - before, 0, "{algorithm:?}");
                std::hint::black_box((digest, tagged));
            }
        }

        // The counter does see allocations: collecting the bytes needs one.
        let before = allocations();
        std::hint::black_box(transfer().hashable_bytes());

        assert!(allocations() > before);
    }

    #[test]
    fn only_hash_with_tag_tells_variants_with_the_same_fields_apart() {
        let algorithm = ALGORITHMS[0];
        let mint = Tx::Mint {
            to: Address::Ed25519([7; 32]),
            amount: Amount::U128(100),
        };
        let burn = Tx::Burn {
            from: Address::Ed25519([7; 32]),
            amount: Amount::U128(100),
        };

        // Without the tag both are just an address and an amount.
        assert_eq!(mint.hash(algorithm), burn.hash(algorithm));
        assert_ne!(mint.hash_with_tag(algorithm), burn.hash_with_tag(algorithm));

        #[cfg(all(feature = "sha256", feature = "blake3"))]
        assert_ne!(
            Hash::Sha256([1; 32]).hash_with_tag(algorithm),
            Hash::Blake3_256([1; 32]).hash_with_tag(algorithm)
        );
    }

    /// hash drops only the value's own tag; fields keep their full encoding, tags included.
    #[test]
    fn hash_reads_the_fields_without_the_values_own_tag() {
        let to = Address::Ed25519([7; 32]);
        let amount = Amount::U128(100);
        let mut fields = to.hashable_bytes_with_tag();
        fields.extend_from_slice(&amount.hashable_bytes_with_tag());
        let mint = Tx::Mint { to, amount };

        assert_eq!(mint.hashable_bytes(), fields);
        assert_eq!(Address::Ed25519([7; 32]).hashable_bytes(), [7; 32]);
        assert_eq!(Amount::U128(100).hashable_bytes(), 100u128.to_le_bytes());
        assert_eq!(ALGORITHMS[0].hashable_bytes(), [0u8; 0]);

        #[cfg(feature = "xxh3")]
        assert_eq!(Hash::Xxh3_64(5).hashable_bytes(), 5u64.to_le_bytes());
    }

    /// The hashed bytes are consensus: changing this layout changes every hash on the chain.
    #[test]
    fn encoding_layout() {
        let mint = Tx::Mint {
            to: Address::Ed25519([7; 32]),
            amount: Amount::U128(100),
        };
        let mut expected = vec![1]; // Tx: Mint
        expected.push(0); // to: Ed25519
        expected.extend_from_slice(&[7; 32]);
        expected.push(0); // amount: U128
        expected.extend_from_slice(&100u128.to_le_bytes());

        assert_eq!(mint.hashable_bytes_with_tag(), expected);
    }

    #[cfg(feature = "xxh3")]
    #[test]
    fn contract_call_layout() {
        let call = Tx::ContractCall {
            to: Address::Ed25519([3; 32]),
            func: Hash::Xxh3_64(9),
            amount: Amount::U128(1),
            data: vec![TxCallArgumentType::Bool(true)],
        };
        let mut expected = vec![3]; // Tx: ContractCall
        expected.push(0); // to: Ed25519
        expected.extend_from_slice(&[3; 32]);
        expected.push(4); // func: Xxh3_64
        expected.extend_from_slice(&9u64.to_le_bytes());
        expected.push(0); // amount: U128
        expected.extend_from_slice(&1u128.to_le_bytes());
        expected.extend_from_slice(&1u64.to_le_bytes()); // data: one argument
        expected.extend_from_slice(&[3, 1]); // Bool, true

        assert_eq!(call.hashable_bytes_with_tag(), expected);
    }

    /// A tag never depends on which other algorithms are compiled in.
    #[test]
    fn hash_tags_are_pinned() {
        #[cfg(feature = "sha256")]
        assert_eq!(HashType::Sha256.hashable_bytes_with_tag(), [0]);
        #[cfg(feature = "blake3")]
        assert_eq!(HashType::Blake3_256.hashable_bytes_with_tag(), [1]);
        #[cfg(feature = "blake2s")]
        assert_eq!(HashType::Blake2s256.hashable_bytes_with_tag(), [2]);
        #[cfg(feature = "blake2b")]
        assert_eq!(HashType::Blake2b256.hashable_bytes_with_tag(), [3]);

        #[cfg(feature = "xxh3")]
        {
            assert_eq!(HashType::Xxh3_64.hashable_bytes_with_tag(), [4]);
            assert_eq!(
                Hash::Xxh3_64(5).hashable_bytes_with_tag(),
                [4, 5, 0, 0, 0, 0, 0, 0, 0]
            );
        }

        #[cfg(feature = "keccak256")]
        assert_eq!(HashType::Keccak256.hashable_bytes_with_tag(), [5]);
    }

    #[test]
    fn encoding_round_trips() {
        for tx in [transfer(), contract_call()] {
            let bytes = tx.hashable_bytes_with_tag();
            let decoded: Tx = wincode::deserialize(&bytes).unwrap();

            assert_eq!(decoded.hashable_bytes_with_tag(), bytes);
        }
    }
}
