use {
    crate::crypto::hasher::Hasher,
    crate::structs::hash::Hash,
    crate::structs::hash::HashType,
    r#macro::document,
    wincode::SchemaWrite,
    wincode::config::DefaultConfig,
    wincode::io::WriteResult,
    wincode::io::Writer,
};

#[document(
    summary = "A value with a canonical byte form that can be hashed: its wincode encoding, the same bytes it has on the wire. hash leaves out the value's own variant tag and hash_with_tag keeps it.",
    category = "Crypto",
    see_also = "Hash, Hasher",
    note = "Without its tag, a value is just its fields, so two variants with the same fields hash the same: Tx::Mint and Tx::Burn moving the same amount to and from the same address, for example. Use hash_with_tag wherever the variant matters, such as transaction ids and signatures.",
)]
pub trait Hashable: SchemaWrite<DefaultConfig, Src = Self> {
    /// How many leading bytes of the encoding are the value's own variant tag: 1 for the chain's
    /// enums, which all use one-byte tags, and 0 for structs, which have none.
    const TAG_LEN: usize = 0;

    #[document(
        parent = "Hashable",
        summary = "Hashes the value's fields without its own variant tag, encoding them straight into the hasher. Nothing is buffered or allocated, however large the value is.",
        params = { algorithm = "The algorithm to hash with." },
        returns = "The digest, tagged with its algorithm.",
        note = "Xxh3_64 is not cryptographic. Use it for contract function selectors, never for transaction ids, signatures or anything else an attacker could gain from forging.",
        see_also = "Hashable::hash_with_tag, Hashable::hashable_bytes, Hasher::digest",
    )]
    fn hash(&self, algorithm: HashType) -> Hash {
        let mut hasher = Hasher::new(algorithm);
        encode(self, SkipTag::new(&mut hasher, Self::TAG_LEN));

        hasher.finalize()
    }

    #[document(
        parent = "Hashable",
        summary = "Hashes the whole encoding, variant tag included, so every variant hashes differently. Allocates nothing.",
        params = { algorithm = "The algorithm to hash with." },
        returns = "The digest, tagged with its algorithm.",
        see_also = "Hashable::hash, Hashable::hashable_bytes_with_tag",
    )]
    fn hash_with_tag(&self, algorithm: HashType) -> Hash {
        let mut hasher = Hasher::new(algorithm);
        encode(self, &mut hasher);

        hasher.finalize()
    }

    #[document(
        parent = "Hashable",
        summary = "The bytes hash reads, collected into a buffer. Only for when the bytes themselves are needed.",
        returns = "The value's wincode encoding without its own variant tag.",
    )]
    fn hashable_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        encode(self, SkipTag::new(&mut bytes, Self::TAG_LEN));

        bytes
    }

    #[document(
        parent = "Hashable",
        summary = "The bytes hash_with_tag reads: the full wincode encoding, which is also what decodes back into the value.",
        returns = "The value's wincode encoding.",
    )]
    fn hashable_bytes_with_tag(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        encode(self, &mut bytes);

        bytes
    }
}

fn encode<T: Hashable + ?Sized>(value: &T, writer: impl Writer) {
    // Neither a hasher nor a Vec ever fails a write, so this only fails if the type's own
    // encoding is broken.
    wincode::serialize_into(writer, value).expect("hashable values always encode");
}

/// Drops the first bytes written through it, which is how hash leaves out a value's tag
/// without encoding the value twice.
struct SkipTag<W> {
    inner: W,
    skip: usize,
}

impl<W> SkipTag<W> {
    fn new(inner: W, skip: usize) -> SkipTag<W> {
        SkipTag { inner, skip }
    }
}

impl<W: Writer> Writer for SkipTag<W> {
    fn write(&mut self, src: &[u8]) -> WriteResult<()> {
        let skipped = self.skip.min(src.len());
        self.skip -= skipped;

        self.inner.write(&src[skipped..])
    }
}

pub fn hash(value: impl Hashable, algorithm: HashType) -> Hash {
    value.hash(algorithm)
}
