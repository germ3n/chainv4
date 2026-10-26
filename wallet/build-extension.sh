#!/bin/sh
# Builds the wallet as a browser extension, for Chrome and for Firefox, into target/extension: each
# a folder to load unpacked (chrome://extensions, about:debugging) and a zip for the stores. The
# toolbar button opens the wallet in Chrome's side panel or Firefox's sidebar, and it asks for leave
# to reach a node as it first talks to it. Needs trunk, python3 and zip.
#
#   wallet/build-extension.sh [--browser chrome|firefox]
#
# The web app is trunk's build, as the node serves from /wallet/. MV3 runs no inline script, so the
# script trunk writes into index.html moves to boot.js; extension/ holds the manifests, the
# background script that opens the panel and routes sites' requests, access.js, which asks for the
# node, bridge.js, between the wallet and the background, and inpage.js and content.js, which give
# sites window.ethereum once the wallet's Settings let them.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
cd "$here"
browsers="chrome firefox"

while [ $# -gt 0 ]; do
    case "$1" in
        --browser) [ $# -ge 2 ] || { echo "--browser takes chrome or firefox" >&2; exit 1; }; browsers=$2; shift 2 ;;
        -h | --help) sed -n '2,10p' "$0" | cut -c3-; exit 0 ;;
        *) echo "unknown argument $1; see --help" >&2; exit 1 ;;
    esac
done

for browser in $browsers; do
    case "$browser" in
        chrome | firefox) ;;
        *) echo "--browser is chrome or firefox, not $browser" >&2; exit 1 ;;
    esac
done

for tool in trunk python3 zip; do
    command -v "$tool" > /dev/null || { echo "$tool is missing" >&2; exit 1; }
done

version=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)
out=target/extension
rm -rf "$out"
mkdir -p "$out"

trunk build --release --dist "$out/web" --public-url ./

# trunk's inline module script, which starts the wasm, into a file of its own.
python3 -I - "$out/web/index.html" <<'PY'
import pathlib, re, sys
page = pathlib.Path(sys.argv[1])
html = page.read_text()
scripts = re.findall(r'<script type="module">(.*?)</script>', html, re.S)
if len(scripts) != 1:
    sys.exit(f"expected trunk's one inline script in {page}, found {len(scripts)}")
(page.parent / "boot.js").write_text(scripts[0].strip() + "\n")
html = re.sub(r'<script type="module">.*?</script>', '<script type="module" src="./boot.js"></script>\n<script src="./access.js" defer></script>\n<script src="./bridge.js" defer></script>', html, flags=re.S)
page.write_text(html)
PY

for browser in $browsers; do
    dir="$out/$browser"
    cp -R "$out/web" "$dir"
    cp -R extension/background.js extension/access.js extension/bridge.js extension/inpage.js extension/content.js extension/icons "$dir/"
    sed "s/VERSION/$version/" "extension/manifest.$browser.json" > "$dir/manifest.json"
    (cd "$dir" && zip -q -X -r "../chainv4-wallet-$browser.zip" .)
    echo "built $here/$dir, and $here/$out/chainv4-wallet-$browser.zip"
done
