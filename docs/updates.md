# Updates

How chainv4 gets its releases out (`src/release/`, `src/node/updates.rs`, `src/cli/update.rs`):
a signed manifest names the newest node, wallet and explorer with a binary for each machine, and
nodes, workers and the apps warn while they are behind it. An upgrade, decided by governance or
announced by a release, stops nodes too old for a change of the rules before the block it starts
at, so they never follow a chain of their own.

## The manifest

A release is one JSON file, `manifest.json`, served by any static host:

```json
{
  "release": {
    "issued": 1793000000,
    "expires": 1795592000,
    "products": [
      {
        "name": "chainv4",
        "version": "0.2.0",
        "least": "0.1.0",
        "notes": "mining pools",
        "files": [
          { "target": "x86_64-unknown-linux-gnu", "url": "https://example.org/releases/chainv4/0.2.0/x86_64-unknown-linux-gnu/chainv4", "sha256": "0x…", "size": 41234567 }
        ]
      }
    ],
    "breaking": [ { "chain": "0x<genesis id>", "least": "0.2.0", "from": 150000 } ]
  },
  "signatures": [ { "key": "ed25519:0x…", "signature": "0x…" } ]
}
```

- **Products** are `chainv4` (the node, the CLI and the worker, one binary), `wallet` and
  `explorer`, each with its newest `version`, the `least` version that still works with the
  network, a line of `notes`, and a binary for each target triple it is built for.
- **`issued`** orders releases: a node takes a manifest only if it was issued after the one it
  has, and keeps the newest in `release.json` beside its database, so a stale host or relay can
  never take it back to an older one.
- **`expires`** is when it stops counting, signed with the rest: 30 days after `issued` unless
  `release new --valid-days` says, up to 366. Past it nothing acts on it, so a host that keeps serving
  an old manifest hides a newer one for that long at most.
- **`breaking`** announces upgrades for chains without governance (see [Upgrades](#upgrades)).
- **The signatures** are over the release's encoding behind a domain of its own (`chainv4
  release`), and cover every file's URL, size and SHA-256, so a binary is the one signed wherever
  it is downloaded from.

## Release keys

Every binary is built with the release keys it trusts and how many must sign: `release/keys.json`,
or the file `CHAINV4_RELEASE_KEYS` names at build time, which a build pipeline uses to pick its own.

```json
{ "threshold": 2, "keys": ["ed25519:0x…", "ed25519:0x…", "ed25519:0x…"], "updates": "https://example.org/releases/manifest.json" }
```

- **A threshold of the keys** must sign a manifest before anything takes it: one stolen key
  pushes nothing. A key that signs twice, or one not listed, counts no more.
- **`updates`** is where nodes look unless `--updates` names another.
- **No keys turns it all off.** The checked-in file has none, so a build from it fetches nothing,
  shows nothing and `chainv4 update` refuses. Make keys with `chainv4 keygen`: an Ed25519 key's
  address is its public key, as keys.json takes it.
- **`floor`** in the same file is the oldest `issued` the build takes: raise it at each release
  build, so a fresh install, with nothing kept yet, goes no further back than that.
- **The keys are in the binary,** so a new build can carry new keys, and the old build trusts the
  old ones until it is replaced.

## Publishing a release

Before building the apps for one, refresh the wallet's Bitcoin checkpoints, so a new install's
light client scans a week rather than months: `scripts/btc-checkpoints.sh`, then commit the diff.

```sh
chainv4 release new --out site/releases --base-url https://example.org/releases \
  --product chainv4 --version 0.2.0 --notes "mining pools" \
  --file x86_64-unknown-linux-gnu=target/release/chainv4 \
  --file aarch64-apple-darwin=dist/chainv4-macos
chainv4 release sign --key release1.key site/releases/manifest.json
chainv4 release sign --key release2.key site/releases/manifest.json
chainv4 release verify site/releases/manifest.json
```

- **`new`** copies each binary to `PRODUCT/VERSION/TARGET/NAME` under `--out` and writes the
  manifest, keeping the other products and upgrades already in it, so the wallet and the explorer
  are released the same way, each on its own. Its signatures are cleared: the release changed.
  `--least` sets the oldest version that still works (it keeps the manifest's otherwise),
  `--breaking GENESIS:LEAST:FROM` announces an upgrade, and `--clear-breaking` drops the old ones.
- **`renew`** issues a manifest anew, changing nothing it names, for it to count for another
  `--valid-days`: `chainv4 release renew site/releases/manifest.json`, then sign it again. Do it
  before the last one lapses, whether or not anything is released.
- **`sign`** adds one key's signature, so each key holder signs on a machine of their own and
  passes the file on; it says once enough have.
- **`verify`** shows what the manifest names and how many of this build's keys signed it.

Then upload the directory to the host. Nothing else is needed: nodes find it within the hour.

**Built by CI:** the **release** workflow (Actions, run by hand) takes a tag that exists, the
URL the directory will be served at, and a line of notes. It builds the node and CLI, the desktop
wallet and the explorer from that tag, for Linux x86-64 and macOS arm64, runs `release new` for
each product, writes `SHA256SUMS`, and leaves it all as a draft GitHub release: the directory as a
tarball, and the manifest and sums beside it. The manifest is unsigned: download it, sign it with
each release key on its holder's machine as above, verify it, and serve the directory. No release
key is ever in CI. The binaries trust the release keys of the repository's `RELEASE_KEYS` variable
(the JSON `release/keys.json` holds), or of that file; with none, or a threshold of 0, the
workflow refuses to build, as binaries trusting no key could never be updated.

## What keeps a host honest

A host can only serve what the keys signed, but it could serve an old manifest to hide a newer one.
So everything that reads a manifest does what a node does:

- **It keeps the newest it saw,** by `issued`, whatever a host or node says later: a node in
  `release.json`, the CLI and a worker in `~/.chainv4/release.json` (or `update --seen FILE`), the
  apps in their settings. One older, or none at all, changes nothing, and the kept one goes on
  showing.
- **It takes nothing older than the floor** built into it.
- **It acts on nothing past `expires`:** no download, no notice and no screen that stops an app. A
  node serving nothing newer says so, and the apps say they cannot tell whether they are up to date.
- **A node goes on obeying an upgrade it already took,** even past `expires`: the chain's upgrade
  does not lapse with the host, and running on past it would fork the node off alone.
- **A file that will not read** counts as none: it says so, goes by the floor alone, and the next
  manifest taken writes it again, whole, by rename.

**A device whose clock is wrong.** What is kept comes from the manifest's own fields, never the
clock, so no wrong clock does lasting harm. Far ahead, everything looks expired: nothing is
installed or shown, and it says the host is behind or the clock is wrong; set right, it all counts
again. Far behind, a manifest past its true expires looks current: it can show a stale notice, but
only the newest it has seen, and it still takes any newer one. Going by a clock was chosen over
pinning to block time, which a node serves and so could lie about.

## Nodes

- **`--updates URL`** is where a node looks, every hour; `off` looks nowhere. It defaults to the
  release keys' `updates`. Under `--tor-only`, or for an onion's URL, it fetches through
  `--tor-proxy`.
- **It warns while it is behind,** at each look: `chainv4 0.2.0 is out, and this node runs 0.1.0`.
- **`GET /release`** serves the manifest it took, for the apps and tools to check themselves,
  with where the node stands and any upgrade that stops it (see [HTTP API](api.md#get-release)).
- **Workers** hear of it from the node as they connect to its mining port, check it against their
  own keys, and warn the same way.
- **`--auto-update`** downloads a newer chainv4's binary for its own target as soon as it has
  taken the release, checks its size and SHA-256, puts it in place of the running binary (written
  as `.new` beside it, then renamed over it), stops, and starts the new binary with the same flags,
  in the same process on Unix. Not for a container: its image is what updates it, and the binary
  in it is not the node's to write.

## Updating by hand

```sh
chainv4 update --node https://node.example.org      # from a node's GET /release
chainv4 update                                       # from the release keys' updates URL
chainv4 update --check                               # only say whether one is out
```

It checks the signatures against this binary's own keys, so the node it asks is only a relay,
says what changed, and asks for `yes` before replacing itself (`--yes` skips that). Running nodes
and workers keep the old binary until they are started again.

## The apps

The wallet and the explorer ask their node's `GET /release` when they connect and every hour,
check the manifest against their own keys, and compare their own version:

- **Behind:** a notice to dismiss, with the notes. On the desktop an **Update** button downloads
  the new binary, checks it, and puts it in place; the app runs it once restarted. In the browser,
  on Android and on iOS, they say to update from where they were installed.
- **Older than `least`:** a screen over the app saying it no longer works with the network, with
  Update on the desktop and Quit.

## Upgrades

An upgrade says: from block `from` on, nodes older than `least` stop. A node too old for it warns
every ten minutes as the block nears, and stops before importing it, or building it, with
`blocks from 150000 on need chainv4 0.2.0 or newer, and this node runs 0.1.0: update it`. Blocks
at or past that height that peers send are left alone. So a release that changes the rules,
such as the block format, reaches the chain without older nodes splitting off onto a fork of
their own; they wait, stopped, at the last block both agree on.

- **By governance** on a chain with it, as `Change::Upgrade`:

  ```sh
  chainv4 send --key member.key vote-change --vote yes --upgrade 0.2.0 --upgrade-from 150000
  ```

  It is kept in state (`change:upgrade`), a later one replaces it, and `from` must be ahead of the
  block deciding it. It is advice to nodes, never a rule blocks are checked by: a node new enough
  runs on as before. `GET /status` shows it as `upgrade`, with the node's own `version`.
- **By a release,** for a chain without governance: its `breaking` entries, each for one chain by
  its genesis id. A node obeys whichever stops it first.
- **Give operators time.** `from` should be far enough ahead for every operator to update, and the
  release with the new binary should be out before the upgrade is decided.
