//! Merkle trees over lists, as RFC 6962 (Certificate Transparency) builds them: a leaf hashed
//! with a 0 byte in front, a node over two with a 1, and a list of n split at the largest power
//! of two below n, so no leaf is ever repeated to fill a level. A block's tx root and receipts root
//! are each one.

use {
    super::hasher::Hasher,
    crate::structs::hash::{Hash, HashType},
    r#macro::document,
};

const LEAF: u8 = 0;
const NODE: u8 = 1;

#[document(
    summary = "A leaf's hash: the data hashed with a 0 byte in front.",
    params = {
        hash = "The chain's hash.",
        data = "What the leaf holds, encoded.",
    },
    category = "Crypto",
    see_also = "root, audit_path, verify",
)]
pub fn leaf(hash: HashType, data: &[u8]) -> Hash {
    let mut hasher = Hasher::new(hash);
    hasher.update(&[LEAF]);
    hasher.update(data);

    hasher.finalize()
}

fn node(left: &Hash, right: &Hash) -> Hash {
    let mut hasher = Hasher::new(left.algorithm());
    hasher.update(&[NODE]);
    hasher.update(&left.to_bytes());
    hasher.update(&right.to_bytes());

    hasher.finalize()
}

/// Where a list of n, at least 2, splits: the largest power of two below n.
fn split(count: usize) -> usize {
    1 << (usize::BITS - (count - 1).leading_zeros() - 1)
}

#[document(
    summary = "The root over a list of leaf hashes, in order: the zero hash for none, the leaf itself for one.",
    params = {
        hash = "The chain's hash, for the zero hash of an empty list.",
        leaves = "The leaves' hashes, from leaf.",
    },
    category = "Crypto",
)]
pub fn root(hash: HashType, leaves: &[Hash]) -> Hash {
    match leaves {
        [] => Hash::zero(hash),
        [only] => *only,
        _ => {
            let (left, right) = leaves.split_at(split(leaves.len()));

            node(&root(hash, left), &root(hash, right))
        }
    }
}

#[document(
    summary = "The proof that one leaf is at its place in the list: the root of the other side at each split, the deepest first.",
    params = {
        hash = "The chain's hash.",
        leaves = "The leaves' hashes.",
        index = "The leaf's place.",
    },
    category = "Crypto",
    see_also = "verify",
)]
pub fn audit_path(hash: HashType, leaves: &[Hash], index: usize) -> Vec<Hash> {
    if leaves.len() <= 1 {
        return Vec::new();
    }

    let (left, right) = leaves.split_at(split(leaves.len()));
    let mut path = match index < left.len() {
        true => audit_path(hash, left, index),
        false => audit_path(hash, right, index - left.len()),
    };
    path.push(match index < left.len() {
        true => root(hash, right),
        false => root(hash, left),
    });

    path
}

#[document(
    summary = "Whether a path proves that a leaf is at this place in a list of this many under this root.",
    params = {
        root = "The list's root, such as a block's receipts_root.",
        leaf = "The leaf's hash.",
        index = "Its place.",
        count = "How many leaves the list has.",
        path = "The proof, from audit_path.",
    },
    category = "Crypto",
)]
pub fn verify(root: &Hash, leaf: &Hash, index: usize, count: usize, path: &[Hash]) -> bool {
    fold(leaf, index, count, path).is_some_and(|folded| folded == *root)
}

/// The root a leaf and its path make, if the path fits a list of count.
fn fold(leaf: &Hash, index: usize, count: usize, path: &[Hash]) -> Option<Hash> {
    if index >= count {
        return None;
    }

    if count == 1 {
        return path.is_empty().then_some(*leaf);
    }

    let (sibling, rest) = path.split_last()?;
    let left = split(count);

    Some(match index < left {
        true => node(&fold(leaf, index, left, rest)?, sibling),
        false => node(sibling, &fold(leaf, index - left, count - left, rest)?),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_leaf_proves_its_place_in_lists_of_any_length() {
        let hash = HashType::default_chain();

        for count in 0..=17 {
            let leaves: Vec<Hash> = (0..count).map(|n: u32| leaf(hash, &n.to_be_bytes())).collect();
            let root = root(hash, &leaves);

            for (index, item) in leaves.iter().enumerate() {
                let proof = audit_path(hash, &leaves, index);

                assert!(verify(&root, item, index, count as usize, &proof), "{index} of {count}");
                assert!(!verify(&root, item, (index + 1) % count as usize, count as usize, &proof) || count == 1, "another place");
                assert!(!verify(&root, &leaf(hash, b"other"), index, count as usize, &proof), "another leaf");
                assert!(!verify(&root, item, index, count as usize, &[proof.clone(), vec![*item]].concat()), "a path too long");

                if let Some((_, short)) = proof.split_last() {
                    assert!(!verify(&root, item, index, count as usize, short), "a path too short");
                }
            }
        }

        assert_eq!(root(hash, &[]), Hash::zero(hash));
        assert_ne!(root(hash, &[leaf(hash, b"a"), leaf(hash, b"b")]), root(hash, &[leaf(hash, b"b"), leaf(hash, b"a")]), "order counts");
    }
}
