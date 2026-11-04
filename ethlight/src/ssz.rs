//! SSZ's Merkle side, as the consensus specs define it: chunks, merkleize with a limit,
//! mix_in_length, Merkle branches by generalized index, and a reader for SSZ's encoding.

use {
    crate::{Crypto, Error, Root},
    alloc::vec::Vec,
};

/// Bytes in a chunk, a leaf of an SSZ tree.
pub const BYTES_PER_CHUNK: usize = 32;

/// SHA-256 of two nodes side by side: their parent.
pub fn hash_pair<C: Crypto + ?Sized>(crypto: &C, left: &Root, right: &Root) -> Root {
    let mut pair = [0u8; 64];
    pair[..32].copy_from_slice(left);
    pair[32..].copy_from_slice(right);

    crypto.sha256(&pair)
}

/// The root of a tree of a depth whose leaves are all zero chunks.
pub fn zero_hash<C: Crypto + ?Sized>(crypto: &C, depth: u32) -> Root {
    (0..depth).fold([0u8; 32], |zero, _| hash_pair(crypto, &zero, &zero))
}

/// A whole number as its chunk: little-endian, padded with zeros.
pub fn chunk_u64(value: u64) -> Root {
    let mut chunk = [0u8; 32];
    chunk[..8].copy_from_slice(&value.to_le_bytes());

    chunk
}

/// Bytes as chunks, the last padded with zeros; none for no bytes.
pub fn pack(bytes: &[u8]) -> Vec<Root> {
    bytes
        .chunks(BYTES_PER_CHUNK)
        .map(|piece| {
            let mut chunk = [0u8; 32];
            chunk[..piece.len()].copy_from_slice(piece);
            chunk
        })
        .collect()
}

/// The least depth a tree of so many leaves needs: 0 for one or none.
pub const fn ceil_log2(count: u64) -> u32 {
    match count {
        0 | 1 => 0,
        _ => 64 - (count - 1).leading_zeros(),
    }
}

/// The depth of a generalized index: how many steps it is below the root.
pub const fn floorlog2(gindex: u64) -> u32 {
    63 - gindex.leading_zeros()
}

/// A generalized index's place among the nodes at its depth, from the left.
pub const fn subtree_index(gindex: u64) -> u64 {
    gindex % (1 << floorlog2(gindex))
}

/// The generalized index of inner, a gindex within the subtree at outer, from the root of all.
pub const fn concat_gindices(outer: u64, inner: u64) -> u64 {
    let depth = floorlog2(inner);

    (outer << depth) | subtree_index(inner)
}

/// merkleize as the specs define it: the chunks padded with zero chunks to the limit's next power
/// of two, hashed up to one root. The limit is in chunks; one smaller than the chunks counts as
/// the chunks.
pub fn merkleize<C: Crypto + ?Sized>(crypto: &C, chunks: &[Root], limit: u64) -> Root {
    let depth = ceil_log2(limit.max(chunks.len() as u64));
    let mut layer: Vec<Root> = chunks.to_vec();
    let mut zero = [0u8; 32];

    for _ in 0..depth {
        if layer.len() % 2 == 1 {
            layer.push(zero);
        }

        hash_layer(crypto, &mut layer);
        zero = hash_pair(crypto, &zero, &zero);
    }

    layer.first().copied().unwrap_or(zero)
}

/// A layer of an even count of nodes hashed in place, pair by pair, into the half as many above:
/// each pair hashed where it lies, without copying it out first.
fn hash_layer<C: Crypto + ?Sized>(crypto: &C, layer: &mut Vec<Root>) {
    for at in 0..layer.len() / 2 {
        layer[at] = crypto.sha256(layer[2 * at..2 * at + 2].as_flattened());
    }

    layer.truncate(layer.len() / 2);
}

/// Whether two byte strings are the same, compared 8 bytes at a time: in a contract, a byte-wise
/// compare of a committee's 24 KiB costs more fuel than hashing it.
pub fn same_bytes(a: &[u8], b: &[u8]) -> bool {
    let ((a_words, a_rest), (b_words, b_rest)) = (a.as_chunks::<8>(), b.as_chunks::<8>());

    a.len() == b.len() && a_rest == b_rest && a_words.iter().zip(b_words).all(|(a, b)| u64::from_ne_bytes(*a) == u64::from_ne_bytes(*b))
}

/// A list's root: its contents' root beside its length, as a chunk.
pub fn mix_in_length<C: Crypto + ?Sized>(crypto: &C, root: &Root, length: u64) -> Root {
    hash_pair(crypto, root, &chunk_u64(length))
}

/// The siblings of the leaf at an index, from the leaf up, in a tree of a depth over these leaves,
/// padded with zero chunks: what is_valid_merkle_branch checks.
pub fn merkle_branch<C: Crypto + ?Sized>(crypto: &C, leaves: &[Root], depth: u32, index: u64) -> Vec<Root> {
    let mut layer: Vec<Root> = leaves.to_vec();
    let mut zero = [0u8; 32];
    let mut index = index;
    let mut branch = Vec::with_capacity(depth as usize);

    for _ in 0..depth {
        branch.push(usize::try_from(index ^ 1).ok().and_then(|sibling| layer.get(sibling).copied()).unwrap_or(zero));

        if layer.len() % 2 == 1 {
            layer.push(zero);
        }

        hash_layer(crypto, &mut layer);
        zero = hash_pair(crypto, &zero, &zero);
        index >>= 1;
    }

    branch
}

/// is_valid_merkle_branch: whether a leaf at an index, a depth down, comes to a root by its
/// siblings. False for a branch shorter than the depth.
pub fn is_valid_merkle_branch<C: Crypto + ?Sized>(crypto: &C, leaf: &Root, branch: &[Root], depth: u32, index: u64, root: &Root) -> bool {
    let Some(branch) = branch.get(..depth as usize) else {
        return false;
    };
    let mut value = *leaf;

    for (level, sibling) in branch.iter().enumerate() {
        value = match (index >> level) & 1 {
            1 => hash_pair(crypto, sibling, &value),
            _ => hash_pair(crypto, &value, sibling),
        };
    }

    value == *root
}

/// is_valid_normalized_merkle_branch: as is_valid_merkle_branch at a generalized index, for a
/// branch that may be longer than its depth, as a later fork's container holds an earlier fork's
/// branch, whose extra siblings, first, must be zero.
pub fn is_valid_normalized_merkle_branch<C: Crypto + ?Sized>(crypto: &C, leaf: &Root, branch: &[Root], gindex: u64, root: &Root) -> bool {
    let depth = floorlog2(gindex) as usize;
    let Some(extra) = branch.len().checked_sub(depth) else {
        return false;
    };

    branch[..extra].iter().all(|sibling| *sibling == [0u8; 32]) && is_valid_merkle_branch(crypto, leaf, &branch[extra..], depth as u32, subtree_index(gindex), root)
}

/// Reads SSZ's fixed parts in order.
pub(crate) struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
    what: &'static str,
}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8], what: &'static str) -> Reader<'a> {
        Reader { bytes, at: 0, what }
    }

    pub fn bytes(&mut self, len: usize) -> Result<&'a [u8], Error> {
        let piece = self.at.checked_add(len).and_then(|end| self.bytes.get(self.at..end)).ok_or(Error::Malformed(self.what))?;
        self.at += len;

        Ok(piece)
    }

    pub fn array<const N: usize>(&mut self) -> Result<[u8; N], Error> {
        Ok(self.bytes(N)?.try_into().expect("N bytes"))
    }

    pub fn u64(&mut self) -> Result<u64, Error> {
        Ok(u64::from_le_bytes(self.array()?))
    }

    pub fn offset(&mut self) -> Result<usize, Error> {
        Ok(u32::from_le_bytes(self.array()?) as usize)
    }

    pub fn roots(&mut self, count: usize) -> Result<Vec<Root>, Error> {
        (0..count).map(|_| self.array()).collect()
    }

    pub fn at(&self) -> usize {
        self.at
    }
}

/// The 4-byte offset at the start of a container whose first field is variable, and the slot at
/// the start of what it points to: how an update's or a bootstrap's fork is told before reading it.
pub(crate) fn slot_at_offset(bytes: &[u8], what: &'static str) -> Result<u64, Error> {
    let offset = u32::from_le_bytes(bytes.get(..4).ok_or(Error::Malformed(what))?.try_into().expect("4 bytes")) as usize;
    let slot = offset.checked_add(8).and_then(|end| bytes.get(offset..end)).ok_or(Error::Malformed(what))?;

    Ok(u64::from_le_bytes(slot.try_into().expect("8 bytes")))
}

/// Whether every byte is zero, checked 8 at a time.
pub(crate) fn zero_bytes(bytes: &[u8]) -> bool {
    let (words, rest) = bytes.as_chunks::<8>();

    rest.iter().all(|byte| *byte == 0) && words.iter().all(|word| u64::from_ne_bytes(*word) == 0)
}

/// Whether every root is zero, as an absent branch is.
pub(crate) fn all_zero(roots: &[Root]) -> bool {
    roots.iter().all(|root| *root == [0u8; 32])
}
