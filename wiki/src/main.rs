use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let src = manifest.join("../src");
    let dest = manifest.join("../target/wiki/index.html");

    if let Err(err) = wiki::generate(&src, &dest) {
        eprintln!("wiki: {err}");

        return ExitCode::from(1);
    }

    println!("wrote {}", dest.display());

    ExitCode::SUCCESS
}
