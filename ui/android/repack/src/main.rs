//! Adds classes.dex to an APK xbuild built, and signs it again: each entry written again as xbuild
//! wrote it, stored ones aligned to 4 bytes as Android maps them and compressed ones compressed,
//! then the dex, compressed, then the APK Signature Scheme v2 block, with the PEM's key (a
//! certificate and its PKCS#8 RSA key), or xbuild's debug key when there is none, so the APK
//! updates one built without the dex. Android 7.0 (API 24), which the apps need at least, checks
//! v2 alone.
//!
//!   chainv4-apk-repack IN.apk classes.dex OUT.apk [--pem FILE]
//!
//! X_PEM, set to the PEM's text, does as --pem does, as for xbuild.

use {
    anyhow::{Context, Result, bail},
    apk::zip::{CompressionMethod, ZipArchive},
    std::io::Read,
    std::path::{Path, PathBuf},
    xcommon::{Signer, Zip, ZipFileOptions},
};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let (Some(input), Some(dex), Some(output)) = (args.next(), args.next(), args.next()) else {
        bail!("chainv4-apk-repack IN.apk classes.dex OUT.apk [--pem FILE]");
    };
    let pem = match (args.next().as_deref(), args.next()) {
        (Some("--pem"), Some(file)) => Some(std::fs::read_to_string(&file).with_context(|| format!("reading {file}"))?),
        (None, _) => std::env::var("X_PEM").ok().filter(|pem| !pem.trim().is_empty()),
        _ => bail!("after the three paths, only --pem FILE"),
    };
    let signer = pem.as_deref().map(Signer::new).transpose().context("the PEM is not a certificate and its PKCS#8 RSA key")?;

    repack(Path::new(&input), Path::new(&dex), &PathBuf::from(&output), signer)?;
    println!("wrote {output}");

    Ok(())
}

/// Writes IN's entries and the dex into OUT, signed.
fn repack(input: &Path, dex: &Path, output: &Path, signer: Option<Signer>) -> Result<()> {
    let mut archive = ZipArchive::new(std::fs::File::open(input).with_context(|| format!("opening {}", input.display()))?)?;
    let mut zip = Zip::new(output, true)?;

    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let name = entry.name().to_string();

        if name == "classes.dex" {
            bail!("{} has a classes.dex already", input.display());
        }

        if entry.is_dir() || name.starts_with("META-INF/") {
            continue;
        }

        let options = match entry.compression() {
            CompressionMethod::Stored => ZipFileOptions::Aligned(4),
            _ => ZipFileOptions::Compressed,
        };
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut bytes)?;
        zip.create_file(Path::new(&name), options, &bytes)?;
    }

    zip.add_file(dex, Path::new("classes.dex"), ZipFileOptions::Compressed)?;
    zip.finish()?;
    apk::Apk::sign(output, signer)?;

    // What Android checks first: the signature holds over what was written.
    apk::Apk::verify(output).context("the APK written does not verify")?;

    Ok(())
}
