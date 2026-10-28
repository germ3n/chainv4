//! A miner's claims on its pools, kept in a file of its own: what `chainv4 worker` writes as the
//! pool tells it of each new one, and `chainv4 claim --saved` claims from. The chain holds each
//! pool's roots and its escrow, but only the pool holds the leaves under them; a miner that keeps
//! its own leaf and path still claims should the pool's node be gone.
//!
//! A claim reads as GET /pool/claim/{address} answers it: the pool, the height and id of the
//! block whose root holds it, the miner, all it earned, its place, how many claims the root
//! holds, the Merkle path, and the chain's hash the path's hashes are in.

use {
    crate::registry::pools::{PoolClaim, SavedClaims},
    crate::structs::address::Address,
    crate::structs::hash::{Hash, HashType},
    crate::structs::hex::from_hex,
    crate::structs::u256::u256,
    r#macro::document,
    serde_json::{Value, json},
    std::path::Path,
};

#[document(
    summary = "A claim as JSON, as GET /pool/claim/{address} answers it and a miner's claims file keeps it.",
    category = "Mining",
    see_also = "claim_from_json, PoolClaim",
)]
pub fn claim_to_json(claim: &PoolClaim) -> Value {
    json!({
        "pool": claim.pool.to_string(),
        "height": claim.height,
        "block": claim.block.to_string(),
        "to": claim.to.to_string(),
        "earned": claim.earned.to_string(),
        "index": claim.index,
        "count": claim.count,
        "path": claim.path.iter().map(ToString::to_string).collect::<Vec<_>>(),
        "hash": claim.block.algorithm().name(),
    })
}

#[document(
    summary = "A claim from JSON, as claim_to_json writes it or GET /pool/claim/{address} answers it.",
    params = { hash = "The chain's hash, for JSON that does not say it." },
    category = "Mining",
    see_also = "claim_to_json",
)]
pub fn claim_from_json(value: &Value, hash: Option<HashType>) -> Result<PoolClaim, String> {
    let hash = match value["hash"].as_str() {
        Some(name) => name.parse().map_err(|_| format!("a claim in an unknown hash, {name}"))?,
        None => hash.ok_or("a claim that does not say its chain's hash")?,
    };
    let address = |name: &str| -> Result<Address, String> { value[name].as_str().and_then(|text| text.parse().ok()).ok_or(format!("a claim with no {name}")) };
    let whole = |name: &str| value[name].as_u64().ok_or(format!("a claim with no {name}"));
    let hash_of = |text: &str| from_hex(text).and_then(|bytes| Hash::from_bytes(hash, &bytes)).ok_or(format!("a claim with a hash that is not the chain's, {text}"));

    Ok(PoolClaim {
        pool: address("pool")?,
        height: whole("height")?,
        block: hash_of(value["block"].as_str().ok_or("a claim with no block")?)?,
        to: address("to")?,
        earned: value["earned"].as_str().and_then(|text| u256::from_str_radix(text, 10).ok()).ok_or("a claim with no earned")?,
        index: whole("index")?,
        count: whole("count")?,
        path: value["path"].as_array().ok_or("a claim with no path")?.iter().map(|step| hash_of(step.as_str().unwrap_or(""))).collect::<Result<_, _>>()?,
    })
}

#[document(
    summary = "The claims kept in a file: none for one that is not there yet.",
    category = "Mining",
    see_also = "save_claim, SavedClaims",
)]
pub fn read_claims(file: &Path) -> Result<SavedClaims, String> {
    let text = match std::fs::read_to_string(file) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(SavedClaims::default()),
        Err(err) => return Err(format!("{}: {err}", file.display())),
    };
    let value: Value = serde_json::from_str(&text).map_err(|err| format!("{} is not JSON: {err}", file.display()))?;
    let claims = value["claims"].as_array().ok_or(format!("{} holds no claims", file.display()))?;

    let claims = claims.iter().map(|claim| claim_from_json(claim, None)).collect::<Result<Vec<_>, _>>().map_err(|err| format!("{}: {err}", file.display()))?;

    Ok(SavedClaims::kept(claims, value["settled"].as_u64()))
}

#[document(
    summary = "Keeps a claim in a file, beside those kept there already, as SavedClaims::keep does: read, kept, and written through a file beside it renamed over it, so a crash leaves the old or the new.",
    params = { settled = "The highest height the pool's blocks can no longer be undone at, as far as is known." },
    returns = "Whether the file changed.",
    category = "Mining",
    see_also = "read_claims, SavedClaims::keep",
)]
pub fn save_claim(file: &Path, claim: PoolClaim, settled: Option<u64>) -> Result<bool, String> {
    let mut saved = read_claims(file)?;

    if !saved.keep(claim, settled) {
        return Ok(false);
    }

    let text = serde_json::to_string_pretty(&json!({ "claims": saved.all().iter().map(claim_to_json).collect::<Vec<_>>(), "settled": saved.settled() })).map_err(|err| err.to_string())?;
    let next = file.with_extension("json.new");

    if let Some(parent) = file.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|err| format!("{}: {err}", parent.display()))?;
    }

    std::fs::write(&next, text).and_then(|()| std::fs::rename(&next, file)).map_err(|err| format!("{}: {err}", file.display()))?;

    Ok(true)
}

#[cfg(test)]
mod tests {
    use {super::*, crate::crypto::keypair::Keypair};

    fn claim(height: u64) -> PoolClaim {
        let hash = HashType::default_chain();
        let step = Hash::from_bytes(hash, &[7; 32]).unwrap();

        PoolClaim {
            pool: Keypair::from_seed([1; 32]).public().address(),
            height,
            block: Hash::from_bytes(hash, &[height as u8; 32]).unwrap(),
            to: Keypair::from_seed([2; 32]).public().address(),
            earned: u256::new(1_000 + u128::from(height)),
            index: 3,
            count: 9,
            path: vec![step, step],
        }
    }

    #[test]
    fn a_claim_reads_back_as_it_was_written() {
        let claim = claim(5);

        assert_eq!(claim_from_json(&claim_to_json(&claim), None), Ok(claim.clone()));
        let mut bare = claim_to_json(&claim);
        bare.as_object_mut().unwrap().remove("hash");
        assert!(claim_from_json(&bare, None).is_err(), "no hash to read its path by");
        assert_eq!(claim_from_json(&bare, Some(HashType::default_chain())), Ok(claim));
    }

    #[test]
    fn claims_kept_in_a_file_survive_and_the_same_one_twice_writes_nothing() {
        let dir = std::env::temp_dir().join(format!("chainv4-claims-{}", std::process::id()));
        let file = dir.join("pool-claims.json");
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(read_claims(&file), Ok(SavedClaims::default()), "none before the first");
        assert_eq!(save_claim(&file, claim(5), None), Ok(true));
        assert_eq!(save_claim(&file, claim(6), Some(5)), Ok(true));
        assert_eq!(save_claim(&file, claim(6), Some(5)), Ok(false));

        let saved = read_claims(&file).unwrap();
        assert_eq!(saved.all(), &[claim(6), claim(5)]);
        assert_eq!(saved.settled(), Some(5), "the settled height is kept with them");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
