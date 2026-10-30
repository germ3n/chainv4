# Deploying a fresh test chain

How the test chain was last started from nothing (2026-10-30, commit `f158f90`), step by
step, with everything enabled: a runbook for the next one. [Docker](docker.md) explains the
pieces; this is the order they go in, and what bit along the way.

It assumes what `docker/pow.sh init` made once: `~/.chainv4-pow` with `owner.key` (the account
holding the coin, the council, and the validator's account), `validator.key`, `faucet.key` and
the binary `~/.chainv4-pow/bin/chainv4`, built by `regenesis` from the checkout. Use that binary
for every command below: a newly built one elsewhere may be blocked by the Mac's firewall (LuLu),
which shows as `cannot reach the node: timeout`.

```sh
B=~/.chainv4-pow/bin/chainv4
N=https://66-78-40-224.sslip.io       # the public node's API, behind nginx
K=~/.chainv4-pow/owner.key
VPS=root@66.78.40.224; SSH="ssh -i ~/.ssh/id_kyun -o IdentitiesOnly=yes"
```

## 0. Fresh machines

On each new machine, as root (Debian forky here; the node builds in its own Debian image, so the
host needs no Rust, clang or LLVM):

```sh
apt-get update
apt-get install -y --no-install-recommends podman podman-compose rsync curl python3 ca-certificates \
  uidmap passt netavark aardvark-dns crun nftables iptables conntrack
```

`nftables` matters: with `--no-install-recommends` it does not come along, and every build then
stops at its first network step with `netavark: nftables error: unable to execute "nft"`. So does
`conntrack`, which `up` clears stale UDP entries with (see [Docker](docker.md)). A build takes about
20 minutes on 4 cores and 8 GB, and fetches the mlua forks from GitHub.

## 1. Keys

The federation, 2 of 3 secp256k1 keys kept on this machine, and two working keys: `evm.key`
pays gas on EVM chains (chainv4's own EVM too) and `relay.key` pays the Bitcoin relay's headers.

```sh
mkdir -p ~/.chainv4-pow/federation
for i in 1 2 3; do $B keygen --scheme secp256k1 --out ~/.chainv4-pow/federation/signer$i.key; done
$B keygen --scheme secp256k1 --out ~/.chainv4-pow/evm.key
$B keygen --scheme secp256k1 --out ~/.chainv4-pow/relay.key
```

keygen prints each key's compressed public key, which the federation lists, and its address;
`$B key show ~/.chainv4-pow/federation/*.key ~/.chainv4-pow/evm.key ~/.chainv4-pow/relay.key`
prints them again later, never the secrets, as funding them in step 4 wants.

## 2. Genesis

```sh
docker/pow.sh regenesis --yes      # same keys, this build's defaults; old genesis and chain set aside
```

Then add the federation to `docker/config/genesis.json` and copy it over
`~/.chainv4-pow/genesis.json`, so both are the same file:

```json
"federation": { "signers": ["0x02…", "0x03…", "0x03…"], "threshold": 2,
                "chain_depth": 20, "evm_depth": 12, "bitcoin_depth": 6 }
```

What the defaults already hold: every engine (`lua`, `wasm`, `evm`), FFG finality on the VPS's
votes, council governance (the owner), names under `dev`, the collection registry, base fees.
The deployment proxy comes with the EVM. Leave `directory` null: the council names the contracts
once they exist (step 5). Check the file opens before shipping it:

```sh
$B --db /tmp/check.db --genesis docker/config/genesis.json --http 127.0.0.1:18777 --no-mdns &
curl -s 127.0.0.1:18777/status    # its genesis id and federation; then stop it and rm the db
```

**Several validators:** give each machine a voting key of its own and each operator an account,
kept on this machine (`~/.chainv4-pow/validators/`), and list them in the genesis's
`finality.validators`, each `{ "account", "voter", "bond" }`, with the machines' addresses in
`bootnodes` as `/ip4/ADDRESS/udp/4001/quic-v1`. Bonds in genesis are minted, so an account needs only
a little in `allocations` for its fees. Two of three finalize, so one may be down. This chain has
three: the public node's voter is `validator.key`, the others `validators/v1-voter.key` and
`v2-voter.key`, their accounts `validators/*-account.key`.

## 3. The VPS

In `docker/.env`: `EXTRA_ARGS=--no-mdns --archive` (every height's state kept),
`INDEX_EVENTS=genesis` and `INDEX_ADDRESSES=genesis` (events and addresses indexed from the
start), and no `SWAP_CONTRACT`, `DEX_CONTRACT` or `DEX_ROUTER` left from the old chain: the
directory names them now.

```sh
docker/pow.sh push $VPS -i ~/.ssh/id_kyun
$SSH $VPS 'cd chainv4 && docker/pow.sh reset --yes'
$SSH $VPS 'cd chainv4 && (setsid nohup docker/pow.sh up > /root/pow-up.log 2>&1 < /dev/null &)'
until curl -s $N/status | grep -q '"genesis":"0x<new id>'; do sleep 20; done    # ~5 minutes
```

The node's log says `keeping the state of every height, as an archive` and that both indexes
hold blocks from height 0.

**More machines:** `push` takes one `docker/.env` and one `validator.key` to wherever it is pointed,
and the faucet's key too, so with several machines copy the repository with `rsync` as `push` does,
excluding `/docker/.env` and `/docker/config/*.token` as well, then give each its own: `.env`,
`config/validator.key`, `config/report.token`, and the faucet's key to the public node alone. Each
`.env` has that machine's `REWARD_TO`, `SERVER_NAME` and the others' addresses in `PEERS`; the public
node has `TLS_MODE=letsencrypt`, `--archive`, `POOL_OPEN=1`, `MINING_SOLO=1`, `MINING_PUBLIC`,
`TOR=1`, `TOR_MINING=1` and the faucet's settings, the others `self-signed` and `--no-mdns` alone.
`docker/compose.sh restart node` alone leaves nginx on the node's old address, answering 502:
restart nginx with it.

**A worker machine** keeps no chain: the node's binary copied out of the public node's container
(`podman cp chainv4_node_1:/usr/local/bin/chainv4 …`) to `/opt/chainv4`, under
`/etc/systemd/system/chainv4-worker.service`, `Restart=always`:

```sh
/opt/chainv4/chainv4 worker --node PUBLIC:4002 --payout ADDRESS --threads 4 --name cloud-worker \
  --claims /opt/chainv4/pool-claims.json
```

## 4. Contracts

Fund the faucet (its address is in `GET /status`), `evm.key`, `relay.key` and each federation
key from the owner, `send transfer --to … --amount …` in the smallest unit (18 decimals). Then:

```sh
$B send --node $N --key $K deploy --lua contracts/lua/swap.lua           # swap contract
$B dex --node $N --key $K deploy                                         # native DEX
$B dex --node $N --key $K --evm deploy                                   # WDEV, factory, router
$B nft --node $N --key $K factory                                        # NFT factory
$B bridge --node $N --key $K deploy                                      # gateway here
$B swap evm-deploy --eth-rpc $N/rpc --key ~/.chainv4-pow/evm.key         # Swap.sol, via the proxy
cargo build --manifest-path contracts/Cargo.toml --target wasm32-unknown-unknown --release \
  --workspace --exclude chainv4-sdk --exclude chainv4-sdk-macros
$B send --node $N --key $K deploy --wasm contracts/target/wasm32-unknown-unknown/release/btc_relay.wasm
$B bridge --node $N --key $K --esplora https://mempool.space/testnet4/api \
  btc relay-start <relay> <testnet4 tip rounded down to 2016> --network test
$B bridge --node $N --key $K btc deploy <relay>                           # Bitcoin gateway
```

Samples: tokens are `contracts/lua/token.lua` with `NAME` and `SYMBOL` changed (GLD, SLV, TUSD);
`nft create nft "Test Pixels" PIX --factory …` and `nft create multi "Test Items" ITEM`, then
`nft mint`; names `swap`, `dex`, `gateway`, `btcgateway`, `btcrelay`, `nfts`, `gold`, `silver`, `tusd`,
`pixels`, `items`, `guestbook` and `counter`; pools with `dex add coin <token> <coin amount> <token amount> --dex …`, and on the EVM
DEX a `Token.sol` from `evm.key` (`send deploy --evm contracts/evm/Token.bin --constructor
"(string,string,uint256)" --arg … `) with `dex --evm --router … add`; examples on each engine:
`guestbook.lua`, `counter.wasm`, `Counter.bin` (`--constructor "(uint256)" --arg 0`); names with
`send name register LABEL` then `send name target LABEL.dev --to …`; a 2-of-3 multisig with EVM
members (`create-multisig --member KEY,,0x…`, the member's public key, `secp256k1:0x02…`, which
`key show` prints as `public`: its address is refused) and its contract, `multisig --key evm.key evm …
deploy --eth-rpc $N/rpc`.

**What bit:**
- `dex add` sometimes stopped after its approval, the pool never opened: run it again, the
  approval being there already.
- Esplora (mempool.space) throttles a burst of header fetches, answering an HTML page or timing
  out. `relay-sync` now asks at an even pace, asks again waiting longer each time, submits 25
  headers a transaction, and keeps what it fetched when Esplora stops answering midway; the
  looping service in step 6 catches up.
- A name's `target` once did not take; check `GET /names/LABEL.dev` and send it again.

## 5. The directory

The council, the owner alone, names the contracts; it is in force at once with one member:

```sh
$B send --node $N --key $K approve-change --directory '{"gateway":"contract:0x…",
  "btc_gateway":"contract:0x…","evm_gateways":[],"swap":"contract:0x…","dex":"contract:0x…",
  "dex_router":"0x…","nft_factory":"contract:0x…"}'
curl -s $N/status    # directory
```

The apps take the directory first; names such as `swap.dev` and the node's flags are fallbacks.

## 6. The Bitcoin relay on the VPS

The node's binary copied out of its container (`podman cp chainv4_node_1:/usr/local/bin/chainv4
/root/chainv4-relay/`), `relay.key` beside it, and `/root/chainv4-relay/relay.sh`: `bridge --node $N --key relay.key --esplora https://mempool.space/testnet4/api btc relay-sync <relay>
--batch 25`, its node the public `https://` API, as the node's port 8080 is not reachable from the
host, then two minutes' sleep, for ever, under `/etc/systemd/system/chainv4-relay.service` (`Restart=always`). After a rebuild of the
image, copy the binary out again and `systemctl restart chainv4-relay`.

## 6b. The onion

`TOR=1` and `TOR_MINING=1` in `docker/.env`, then push and up: tor starts with the stack, the
node listens for peers over TCP for the onion as well, and `docker/pow.sh onion` on the VPS prints
the address. Check it from here through a local Tor (`brew install tor`, then `tor &`), and join
it over Tor alone with `docker/pow.sh join ADDRESS.onion --cpu`:

```sh
curl --socks5-hostname 127.0.0.1:9050 http://ADDRESS.onion/status
```

## 7. The laptop and the apps

```sh
nohup docker/pow.sh join 66.78.40.224 --cpu > ~/.chainv4-pow/join.log 2>&1 &   # never the GPU
sh <scratchpad>/apps-build.sh      # trunk web bundles, then arm64 APKs of the explorer and wallet
```

The APKs (`wallet/target/android/chainv4-wallet-arm64.apk`, the explorer's beside it) go to
gofile anonymously: the first `curl -F file=@… https://upload.gofile.io/uploadfile` answers a
folder and a guest token, and the second goes into that folder with the token. Publish their
SHA-256s with the link.

## The last one

| What | Where |
| --- | --- |
| Genesis | `0xee81738047ca1e17…`, named `chainv4 testnet`, with the supply in state; the directory is governance's change 1, the directory's version 1 |
| Swap contract, `swap.dev` | `contract:0xc4ad35b6…68fb7` |
| Native DEX, `dex.dev` | `contract:0x6c1df38c…90a4` |
| EVM DEX router | `0xfd9524e58c1a904b439e9a3292a95cf6562aa458` |
| NFT factory, `nfts.dev` | `contract:0x5c352729…4297` |
| Gateway, `gateway.dev` | `contract:0xe6912df3…55dc` |
| Bitcoin relay, `btcrelay.dev` | `contract:0x717374cf…e9c0` (testnet4 from 157248) |
| Bitcoin gateway, `btcgateway.dev` | `contract:0xc53df990…04e7` |
| Swap.sol | `0xb3f77d55ac803f023df8f4d21fd1d103e5be2d50` |
| Tokens `gold.dev`, `silver.dev`, `tusd.dev`; ERC-20 TETH | GLD, SLV, TUSD pooled with the coin; TETH `0x3c262351…88e8` on the EVM DEX |
| Collections `pixels.dev`, `items.dev` | 3 NFTs; ids 1 and 2, 100 each |
| Examples | `guestbook.dev`, `counter.dev`, Counter.sol `0x058f1d0b…3be5` |
| Sample multisig | `multisig:0x3955d3dd…4d18`, 2 of 3, its contract `0xa4629353…1f2f` |
| Onion | `nvudjdqvq6voglt7xdjrpx6jnf2p2wip4ul73wobcqnudgrpf4atrkad.onion` |
| Mining | `66.78.40.224:4002`, an open pool that takes workers mining alone too (`POOL_OPEN=1`, `MINING_SOLO=1`, `MINING_PUBLIC`), and `/mining/ws` behind nginx |
| Apps | https://gofile.io/d/o6woVRVR |
