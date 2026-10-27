# Faucet

A node can run a faucet for a dev or test chain: coins from a key of the operator's, a drip to
each address that asks, once per address and per connection each period. Each request carries a
moment's proof of work, which the apps and the CLI do for the person asking, so scripting many
addresses costs real CPU. Any chain may run one, since the coins are the operator's own; it is
meant for chains whose coins are worth nothing.

## Running one

```sh
chainv4 keygen --out faucet.key            # a key of its own, holding only what it is funded with
chainv4 send transfer --to <its address> --amount <units>   # fund it from any account
chainv4 --http 0.0.0.0:8080 --faucet-key faucet.key --faucet-amount 10 --faucet-every 24h
```

| Flag | What it sets |
| --- | --- |
| `--faucet-key FILE` | The key whose account pays. Without it, there is no faucet. Needs `--http`. |
| `--faucet-amount COINS` | Each drip, in whole coins at the chain's decimals, such as `10` or `0.5`: 10 unless set. |
| `--faucet-every SPAN` | How long one address, and one IP, wait between drips: `24h`, `30m`, `90s` or `2d`; 24h unless set. |
| `--faucet-work BITS` | How many zero bits each request's proof of work starts with, 0 to 40: 20 unless set, about a second in a browser. Each bit doubles it. |

`GET /status` then shows `faucet`, with who pays, how much, how often and how much work, which the
wallet and the explorer show their faucet by.

**On the Docker stack,** `docker/config/faucet.key` turns it on, with `FAUCET_AMOUNT`,
`FAUCET_EVERY` and `FAUCET_WORK` in `docker/.env`. For a proof-of-work chain across machines,
`docker/pow.sh faucet https://your-vps --fund 1000`, on your machine, makes the faucet's key,
funds it with 1,000 coins from your own key through that node, and adds the settings to
`docker/.env`; then `push` and `up` as usual. Run it again to top it up.

## Asking for coins

- **The wallet:** on a node with a faucet, the Overview has a **Faucet** card: pick one of the
  account's addresses and **Get**.
- **The explorer:** **Faucet**, under Tools: an address, an Ethereum `0x…`, or a name.
- **The CLI:** `chainv4 faucet --node https://… --to <address or name>`.
- **By hand:** `POST /faucet` with `{"to": "ed25519:0x…", "window": W, "nonce": N}`; see
  [the API](api.md#get-faucet-and-post-faucet).

## The proof of work

A request names a ten-minute window, `now / 600000`, and a nonce whose SHA-256, over the label
`chainv4 faucet`, the genesis id, the address as the chain encodes it, the window and the nonce,
big-endian, starts with as many zero bits as the faucet asks. The faucet takes this window's and
the last one's. A solution is good for one chain, one address and one window alone:
`chainv4::faucet::solve_faucet` works one out, a slice at a time where an app wants its frames.

## What stops one person draining it

- **Once per address each period,** however it asks.
- **Once per IP each period.** Behind a proxy on the node's machine or its private network, as the
  Docker stack's nginx is, the IP is the last one `X-Forwarded-For` names: the one the proxy added,
  which a caller cannot forge. From anywhere else, the connection's own.
- **The proof of work,** for each request.
- **A drip that does not go out** frees the address and IP to ask again: one to bytes that are no
  key, say, or one the node would not take.

The cooldowns live in the node's memory: a restart forgets them. The faucet keeps its own nonce
counter, so drips go out one after another before the chain has taken the last.
