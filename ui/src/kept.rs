//! Files that a kill cannot leave half written, for what an app keeps on a device or a desktop: each
//! is written to a file beside it, flushed, read back, and only then renamed over it, so what is there
//! is what was, or what became, and never between.

use {
    std::fs,
    std::io::Write,
    std::path::Path,
};

/// What a file being written goes by until it is whole.
pub const WRITING: &str = "writing";

/// Puts these bytes at this path whole, or leaves what is there: written beside it, flushed, read
/// back, and only then renamed over it.
pub fn written_whole(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let beside = path.with_extension(format!("ron.{WRITING}"));
    let fail = |why: std::io::Error| format!("could not write {}: {why}", path.display());
    let result = (|| {
        let mut file = create_private(&beside).map_err(fail)?;
        file.write_all(bytes).map_err(fail)?;
        file.sync_all().map_err(fail)?;
        drop(file);

        if fs::read(&beside).map_err(fail)? != bytes {
            return Err(format!("{} did not read back as written", path.display()));
        }

        fs::rename(&beside, path).map_err(fail)
    })();

    if result.is_err() {
        fs::remove_file(&beside).ok();
    } else if let Some(folder) = path.parent() {
        flush_folder(folder);
    }

    result
}

/// A new file only this user can read, where that can be asked for.
fn create_private(path: &Path) -> std::io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);

    options.open(path)
}

/// Makes a folder this user's alone, where that can be asked for.
#[cfg(unix)]
pub fn private_folder(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).ok();
}

#[cfg(not(unix))]
pub fn private_folder(_: &Path) {}

/// Makes a rename last through losing power, where a folder can be flushed.
pub fn flush_folder(dir: &Path) {
    #[cfg(unix)]
    if let Ok(folder) = fs::File::open(dir) {
        folder.sync_all().ok();
    }
    #[cfg(not(unix))]
    let _ = dir;
}
