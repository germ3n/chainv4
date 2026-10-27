//! The node's network key: the Ed25519 key its libp2p peer id is of, kept in a file so the id is
//! the same each start, and a peer can be named by it, as /p2p/<id> in --peer. Made at the first
//! start, beside the database unless --net-key names another file, and readable by its owner
//! alone. It signs gossip and connections, and nothing on the chain: losing it costs the node its
//! name, not coins.

use {
    crate::structs::hex::{from_hex, to_hex},
    libp2p::identity::Keypair,
    r#macro::document,
    std::path::Path,
};

#[document(
    summary = "The network key file's name, beside the database's directory unless --net-key names another: net.key.",
    category = "Node",
    see_also = "net_key",
)]
pub const NET_KEY_FILE: &str = "net.key";

/// What the file's one line starts with, before the secret's hex.
const PREFIX: &str = "libp2p-ed25519:";

#[document(
    summary = "The node's network key from its file, made and written there, readable by its owner alone, when there is none. A file that is there but does not read is an error, never replaced: it would change the node's peer id.",
    category = "Node",
    params = { path = "The key file." },
    returns = "The key, or why it could not be read or made.",
    see_also = "NET_KEY_FILE, NetConfig::key",
)]
pub fn net_key(path: &Path) -> Result<Keypair, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => read(&text).ok_or_else(|| format!("{} is not a network key: one line, {PREFIX} and 64 hex digits. Move it aside for a new one, and a new peer id", path.display())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let mut secret = [0u8; 32];
            getrandom::fill(&mut secret).map_err(|err| format!("no randomness for a network key: {err}"))?;
            let key = Keypair::ed25519_from_bytes(secret).map_err(|err| err.to_string())?;
            write(path, &format!("{PREFIX}{}\n", &to_hex(&secret)[2..]))?;
            log::info!("[net] made a network key in {}", path.display());

            Ok(key)
        }
        Err(err) => Err(format!("cannot read {}: {err}", path.display())),
    }
}

fn read(text: &str) -> Option<Keypair> {
    let hex = text.trim().strip_prefix(PREFIX)?;
    let secret: [u8; 32] = from_hex(&format!("0x{hex}"))?.try_into().ok()?;

    Keypair::ed25519_from_bytes(secret).ok()
}

/// Writes the key beside where it goes, readable by its owner alone, then moves it in place, so a
/// crash never leaves half a key.
fn write(path: &Path, text: &str) -> Result<(), String> {
    let staged = path.with_extension("key.new");
    let failed = |err: std::io::Error| format!("cannot write {}: {err}", path.display());

    if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(failed)?;
    }

    {
        use std::io::Write;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);

        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);

        let mut file = options.open(&staged).map_err(failed)?;
        file.write_all(text.as_bytes()).and_then(|()| file.sync_all()).map_err(failed)?;
    }

    std::fs::rename(&staged, path).map_err(failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_is_made_once_and_read_back_the_same() {
        let dir = std::env::temp_dir().join(format!("chainv4-net-key-{}", std::process::id()));
        let path = dir.join("data").join(NET_KEY_FILE);
        let _ = std::fs::remove_dir_all(&dir);

        let made = net_key(&path).unwrap();
        let again = net_key(&path).unwrap();

        assert_eq!(made.public().to_peer_id(), again.public().to_peer_id(), "the same peer id each start");
        assert!(std::fs::read_to_string(&path).unwrap().starts_with(PREFIX));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600, "its owner's alone");
        }

        // A file that does not read stops the node, rather than giving it a new name.
        std::fs::write(&path, "not a key").unwrap();
        assert!(net_key(&path).unwrap_err().contains("is not a network key"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "not a key", "left as it was");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
