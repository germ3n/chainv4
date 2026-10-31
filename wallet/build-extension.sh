#!/bin/sh
# Builds the wallet as a browser extension, for Chrome and for Firefox, into target/extension: each
# a folder to load unpacked (chrome://extensions, about:debugging) and a zip for the stores. The
# toolbar button opens the wallet in Chrome's side panel or Firefox's sidebar, and it asks for leave
# to reach a node as it first talks to it. Needs trunk, python3 and zip.
#
#   wallet/build-extension.sh [--browser chrome|firefox] [--store]
#
# --store is what the Chrome Web Store and Firefox Add-ons take: the `store` feature alone, as neither
# takes an extension that mines, so no miner goes in, and it fails if one did. Its folders and zips
# are named -store, beside the others.
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
store=

while [ $# -gt 0 ]; do
    case "$1" in
        --browser) [ $# -ge 2 ] || { echo "--browser takes chrome or firefox" >&2; exit 1; }; browsers=$2; shift 2 ;;
        --store) store=-store; shift ;;
        -h | --help) sed -n '2,15p' "$0" | cut -c3-; exit 0 ;;
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
web="$out/web$store"
rm -rf "$web"
mkdir -p "$out"

if [ -n "$store" ]; then
    # index.html with no miner: trunk reads what to build from the page, and this names it apart so
    # the page the nodes serve keeps its miner.
    page=index.store.html
    trap 'rm -f "$here/$page"' EXIT
    python3 -I - index.html "$page" <<'PY'
import pathlib, sys
lines = pathlib.Path(sys.argv[1]).read_text().splitlines(keepends=True)
kept = [line for line in lines if "miner-worker" not in line]
if len(lines) - len(kept) != 2:
    sys.exit(f"expected the miner's two lines in {sys.argv[1]}, found {len(lines) - len(kept)}")
wallet = '<link data-trunk rel="rust" data-bin="wallet"'
if sum(wallet in line for line in kept) != 1:
    sys.exit(f"expected the wallet's one line in {sys.argv[1]}")
pathlib.Path(sys.argv[2]).write_text("".join(kept).replace(wallet, wallet + ' data-cargo-no-default-features data-cargo-features="store"'))
PY
    trunk build "$page" --release --dist "$web" --public-url ./
    [ -f "$web/index.html" ] || mv "$web/$page" "$web/index.html"

    # What the stores refuse must not be in what goes to them, whatever comes to change above.
    if find "$web" -name '*miner*' | grep -q . || grep -l -a -e 'miner-worker' -r "$web" | grep -q .; then
        echo "a miner is in the store build, in $web" >&2
        exit 1
    fi
else
    trunk build --release --dist "$web" --public-url ./
fi

# trunk's inline module script, which starts the wasm, into a file of its own.
python3 -I - "$web/index.html" <<'PY'
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
    dir="$out/$browser$store"
    zip="chainv4-wallet-$browser$store.zip"
    rm -rf "$dir" "$out/$zip"
    cp -R "$web" "$dir"
    cp -R extension/background.js extension/access.js extension/bridge.js extension/inpage.js extension/content.js extension/icons "$dir/"
    sed "s/VERSION/$version/" "extension/manifest.$browser.json" > "$dir/manifest.json"
    (cd "$dir" && zip -q -X -r "../$zip" .)
    echo "built $here/$dir, and $here/$out/$zip"
done
