# Hardcoded values in chainv4: what should be configurable or votable

This was a read-only audit. Nothing was built or run. Line numbers are from the working tree on 2026-11-03.

**Classes:** A = governance (votable), B = genesis only, C = node operator flag or config, D = app setting, E = leave it.

**Columns:**
- "Past validity?" asks whether the change would alter which past blocks are valid, and so needs activation at a height or epoch.
- "Contracts?" asks whether deployed contracts depend on the value.
- "Consensus?" asks whether the fix needs a redeploy of every node (R) or a regenesis (G).

Today governance can change consensus, bootnodes, name_prices, collections, federation, directory, fees, reward, limits, finality, switch_finality, upgrade and name. Some items below are already in `docs/todo.md`, and are marked **(todo)**:
- vm::cost from genesis
- `Change::Engines`
- the DEX fee decision
- the fixed strike rules
- the engine pools as node flags
- "no connection or request limits"

---

## 1. Top 15, in priority order

| # | Where | Value | What it governs | Class / kind | Past validity? | Contracts? | Consensus? |
|---|---|---|---|---|---|---|---|
| 1 | `src/governance/mod.rs:93-97` (`Council { members, threshold, approval_epochs }`), and `Electorate { approval_epochs }` at :89 | Genesis's council and threshold | Who may approve changes. **Governance cannot change itself.** A council chain, such as the devnet with the owner key alone, can never add members, rotate a lost key or raise the threshold without a regenesis. | **A, new `Change::Governance`.** Members, threshold and approval_epochs, with the kind kept fixed. Its own `ChangeKind`, decided by the current council. | No. Apply from the next epoch boundary, so the approvals still open are counted under one set. Void the open approvals of every kind on decision, or keep them only for members who stay. | No | R. A new `Change` tag (13), with no change to genesis. Light clients know only `GovernanceKind`, so they are unaffected. |
| 2 | `src/node/mod.rs:63` `MEMPOOL_CAPACITY`, `src/mempool/mod.rs:236-250` and `:303` | 50,000 txs. No per-sender cap, no nonce-gap cap, no replacement: a second tx at the same nonce is `Duplicate`. Full means reject, with no eviction of cheaper txs. | Mempool policy. **Likely DoS:** each tx's reserve is checked against the whole balance on its own. One funded key can queue 50k far-future-nonce txs at a fair price. `select` stops at gaps, so they never mine, and `prune` never drops them, which locks everyone else out with `Full`. | **C.** `--mempool-size`, `--mempool-per-sender` (e.g. 64), `--max-nonce-gap` (e.g. 64), `--replace-bump-percent` (e.g. 10, for fee replacement), plus eviction of the lowest-priority tx when full. Also check the reserve cumulatively per sender. | No | No | No. Node-local only. |
| 3 | `contracts/lua/btc_gateway.lua:372-393` (`M.setup` sets `fee_cap`, `fee_floor` and `grace` **once**, with no setter and no `check_perms`). The CLI defaults at `src/cli/bridge/btc.rs:49-57` are fee_cap 50,000 sat, floor 2 sat/vB and grace 168 h. | Fixed for the gateway's life | The most a release may pay, the least per vByte, and how long an old federation's deposit addresses stay open. With a fixed 50k-sat cap, a fee spike can make every release impossible. The only way out is a new gateway and a sweep. | **A, extend `Change::Federation`** with a `btc: { fee_cap, fee_floor, grace_ms, cancel_after_ms, least_withdrawal, default_fee_part }` block, or add a new `Change::Bridge`. The gateway already reads the federation through a host call (`cost::FEDERATION`). Fold in `BTC_CANCEL_AFTER_MS` (`src/bridge.rs:197`), `LEAST`, `LEAST_PAYOUT` and `DEFAULT_FEE_PART` (the "a tenth") at `btc_gateway.lua:35-38`. | No. It applies to proposals and withdrawals made after the decision. Keep the cap a withdrawal agreed to (`most_fee`) binding. | **Yes.** The gateway must read these values from the registry, which needs a new gateway deployed and swept to. | R. `Federation`'s encoding grows, so the new fields need Unset or Framed encoding to keep genesis ids. |
| 4 | `src/vm/evm/mod.rs:337` `CfgEnv::new_with_spec(SpecId::PRAGUE)` | PRAGUE | Which EVM opcodes exist, their gas, and EIP-170/3860 code-size limits. Moving to OSAKA or later is impossible without silently changing how old blocks replay. | **B now, then A.** Add a genesis `evm_spec` (Unset-encoded), and a `Change::EvmSpec { from_height, spec }`, or let `Change::Upgrade` carry it. Keep a schedule of (height, spec) so a replay picks each block's spec. | **Yes.** Activation must be at a height, and the schedule must be kept in state. | **Yes.** Opcodes, gas and code size all change. | R, plus a height. The spec also pins the revm version: a revm bump that changes PRAGUE semantics is a fork. |
| 5 | `src/vm/mod.rs:127-189` (`vm::cost`: STORAGE_WRITE 500, CALL 1,000, VERIFY 3,000 …). The EVM precompiles: `vm/evm/native.rs:57-59` (CALL_GAS 2,600, WORD_GAS 3) and `vm/evm/names.rs:34-36` (LOOKUP_GAS 3,000). | Compiled in | What each host operation costs in fuel. `fees.schedule` only scales them all together through `fuel_per_gas`. | **A, join `Change::Fees`** (a `costs` table in `ScheduleConfig`). **(todo)** covers vm::cost. Add the precompile gas to the same table. | No. It applies from the block after the decision, as Fees does. | Gas estimates move, and a contract that hardcodes call stipends could break. | R. A regenesis only if genesis must carry the table; Unset avoids that. |
| 6 | `src/vm/host.rs:23-47` (MAX_CALL_DEPTH 8, MAX_KEY_LEN 256, MAX_VALUE_LEN 64 KiB, MAX_SCAN 256, MAX_EVENT_DATA 16 KiB, MAX_EVENTS 256). Also `vm/wasm/mod.rs:30` MAX_MODULE_LEN 512 KiB, `:58-59` recursion 1,000 and stack 1M, and `vm/lvm/compiler.rs:20` MAX_SOURCE_LEN 128 KiB. | Compiled in | Contract size, storage and call limits | **A, extend `Change::Limits`**: `BlockLimits` gains `contract: ContractLimits`. | No. It applies from the next block. **Raising is safe. Lowering can brick contracts** that store 64 KiB values or nest 8 deep, so allow lowering only with a warning. | Yes | R. `BlockLimits` needs Unset-encoded fields. MAX_BODY (`api/mod.rs:91`) must follow MAX_MODULE_LEN. |
| 7 | `src/registry/names.rs:60-62` (`grace_ms`, `max_years`) | Genesis only. `Change::NamePrices` covers the prices alone. | The grace period after expiry, and the most years ahead a name can be held | **A, widen `NamePrices` into `Change::Names`.** Everything but `suffix`. | No. A held name keeps its expiry. Say whether a new grace applies to names already in grace (recommended: yes, judged at the time). | The EVM names precompile reads expiry, and nothing else | R |
| 8 | `src/chain/mod.rs:244` `pow_window` (genesis only). `src/governance/mod.rs:217`: "a proof-of-work consensus keeps genesis's window". `src/consensus/pow.rs:19-25` (ADJUSTMENT 64, MAX_STEPS_DOWN 32, MAX_SWING 4). | Fixed | Difficulty retargeting. **Awkward:** governance can change `target_block_ms` but not the window, or the step and swing that pair with it. | **A, let `Change::Consensus`'s `PowConfig` carry `window`, `adjustment`, `max_steps_down` and `max_swing`** as optional fields. | Already epoch-activated by Consensus | No | R. New PowConfig fields need Unset in genesis. |
| 9 | `src/consensus/mod.rs:48` `MAX_DRIFT_MS` | 15 s | How far ahead of the local clock a block may be, under every consensus kind. 15 s is 3 slots at the devnet's 5 s blocks, 7 at 2 s and 0.25 at 60 s, so it does not scale with block time. | **B or A.** Put it in `ConsensusConfig`, or derive it, e.g. `max(15 s, 3 × slot)`. | It is checked against the local clock, never on replay, so changing it never invalidates the past. | No | R |
| 10 | `src/swap/mod.rs:317` CHAIN_DEPTH 20, `:326` EVM_DEPTH 12 | Fixed | When a swap acts on a lock. These duplicate `Federation.chain_depth` and `evm_depth`. The wallet exposes `bitcoin_depth` only (`wallet/src/app.rs`). | **D plus C.** Wallet settings and `chainv4 swap --chain-depth/--evm-depth`, defaulting to the federation's values or `max_reorg_depth`. | No | No | No |
| 11 | `src/registry/validators.rs:268-280` `slash` | The whole bond. The reporter gets bond / reporter_divisor and the rest is burned. | Slashing severity | **A, join `Change::Finality`**: `FfgConfig.slash_permille`, defaulting to 1000. | No. It applies at inclusion. | No | R. It is Framed already, so add a new trailing field. |
| 12 | `src/chain/mod.rs:232` `token_functions` | Genesis only | What makes a contract a token. **Asymmetric:** `collections` is governable. | **A, new `Change::TokenFunctions`**, or merge into a `Registries` kind with Collections | No. Re-register on the next upgrade or refresh, as Collections does. | Yes, registry membership | R |
| 13 | `src/chain/mod.rs:247` `vote_reward_source` | Genesis only | Whether vote rewards are minted or paid from the fee pool | **A, join `Change::Fees`** (or Finality) | No. It applies from the next block, and the pool's balance stays. | No | R |
| 14 | `contracts/lua/dex.lua:172-174`, `contracts/evm/Dex.sol:196-198, 274-275`, `src/dex.rs:19`, and the apps (`wallet/src/views/trade.rs:630`, `explorer/src/views/dex.rs:31`) | 0.3% (997 / 1000), five copies | The DEX fee | **(todo)** "Decide what the DEXes' fees are for". If made variable, make it a factory or pool parameter that the apps read. Don't make it a chain vote. | Not a block rule | **Yes.** A redeploy of the DEX. | No |
| 15 | `src/node/mod.rs:1064` (serving `SyncRequest::Blocks`), `src/net/mod.rs:36` MAX_SYNC_BLOCKS 128, `src/net/codec.rs:12` MAX_RESPONSE 64 MiB, `src/node/sync.rs:259` | 128 blocks, no byte bound | **Likely bug.** A node serves up to 128 blocks whatever their size, and the codec refuses a response over 64 MiB. With `max_block_bytes` up to 6 MiB, about 11 full blocks pass the limit. The asker always asks for min(gap, 128), so a run of big blocks may never sync from that peer. | **C plus a fix.** Cap the blocks served by bytes, as `RangeLimits::SERVED` does for HTTP, and make it `--sync-batch-bytes` | No | No | No, but every node must serve within the cap |

---

## 2. Already configurable, but awkwardly

1. **`--swap-contract`, `--dex-contract` and `--dex-router`** (`src/main.rs:364-378`) duplicate the governed `Directory` (swap, dex, dex_router). `GET /status` takes the flags (`src/api/mod.rs:880-898`), so an operator's flag can point the apps somewhere other than what governance announced.
   - Fix: default to the directory, and show any flag value as the operator's override, or drop the flags.
2. **`--cancel-after`** (`src/cli/bridge/mod.rs:80`, default `BTC_CANCEL_AFTER_MS` 6 h) is set by each signer, but when a release gets voted down is federation-wide policy.
   - In effect, the threshold-th most patient signer decides.
   - Fix: move it into the federation's or gateway's settings, as in #3.
3. **The BTC gateway's setup flags become permanent** (fee_cap, fee_floor, grace_hours). See #3.
4. **Governance can change `target_block_ms` but not `pow_window`.** See #8.
5. **Some values are counted in blocks, but their docs reason in 2 s blocks**, while the devnet runs 5 s blocks:
   - `MAX_UNDERPRICED_BLOCKS` 32 (`src/mempool/mod.rs:41`), "about a minute", is 160 s on the devnet.
   - `DEFAULT_STATE_HISTORY` 10,000 (`src/chain/mod.rs:125`), "about 5.5 hours", is 14 h on the devnet.
   - Fix: express both in time, or document them per block time.
6. **`MAX_HISTORY` 64 epochs** (`src/chain/finality/votes.rs:39`) is how far back surround votes are caught. `withdrawal_delay` is governable through `Change::Finality`, so if it is ever voted above 64, a surround made more than 64 epochs back goes unpunished while the bond is still locked.
   - Fix: derive it as `max(64, withdrawal_delay + vote_depth)`.
7. **Swap depths duplicate the federation's depths**, and the wallet exposes only the Bitcoin one. See #10.
8. **`--http-per-ip`** limits concurrent requests only. There is no per-second rate. See the network section.

---

## 3. The rest, by area

### Consensus and chain rules

| Where | Value | What / note | Class |
|---|---|---|---|
| `src/chain/mod.rs:234` `max_reorg_depth` | Genesis | Fork-choice depth and undo retention. Changing it live interacts with undo records already pruned. | **B** (keep) |
| `src/consensus/staked.rs:33` `MAX_DRAW_WAIT` | 1,024 slots | The look-ahead for a staked draw | **B** (could join `ConsensusConfig::Staked`) |
| (none) | No per-tx byte cap | A single tx may fill `max_block_bytes`. Only `Tx::Ethereum` has `MAX_RAW` 128 KiB (`src/crypto/ethereum.rs:15`). | **A, `Change::Limits`**: add `max_tx_bytes` and fold `MAX_RAW` into it |
| `src/registry/multisig.rs:26` `MAX_MEMBERS` | 20 | The largest native multisig | **A, `Change::Limits`** (low priority). Raising is safe. |
| `src/registry/names.rs:27,39,42,45` | MIN_LABEL 3, MAX_TEXTS 16, MAX_TEXT_KEY 32, MAX_TEXT_VALUE 512 | Name rules. MIN_LABEL is the index base of `prices`. | **B** (into `NamesConfig`, Unset). Could join `Change::Names` later. |
| `src/mempool/mod.rs:41` `MAX_UNDERPRICED_BLOCKS` | 32 | How long an underpriced tx is kept | **C** |
| `src/node/mod.rs:61` `UNBOUNDED_CHAIN_TXS` | 1,000 | The tx cap per block when a chain has no gas or byte bound | **C**: make it the default of `--max-block-txs` |

### Bridges

| Where | Value | What / note | Class |
|---|---|---|---|
| `src/bridge.rs:425-433` | BTC_RATE_TIMES_FASTEST 3, ALWAYS_SIGNED 25, BLIND 200 sat/vB | The highest fee rate a signer will sign for | **C**: per-signer policy, next to the existing `--max-fee-rate` |
| `src/cli/bridge/btc.rs:23` `SOON_BLOCKS` | 3 | The fee-rate target for proposals | **C**: `--target-blocks` |
| `src/cli/bridge/btc.rs:166-170` | ESPLORA_PACE 150 ms, TRIES 6 | Esplora throttling | **C** |
| `src/cli/bridge/mod.rs:36` `LOG_WINDOW` | 5,000 blocks | The `eth_getLogs` span. Many RPC providers cap it at 500 to 10k. | **C**: `--log-window` |
| `src/bridge.rs:480-483` and `btc_gateway.lua:32,49` | 50 withdrawals, 100k vB | Duplicated in Rust and Lua | **E**: Bitcoin relay policy. Have Rust read them from the gateway. |
| `src/bridge.rs:629`, `btc_gateway.lua:43`, `src/swap/htlc.rs:57`, `src/crypto/bitcoin/psbt.rs:587` | Dust 546, 546, 294, 330 | Bitcoin dust rules | **E** |
| `src/registry/federation.rs:25` `MOST_SIGNERS` | 20 | Bounded by CHECKMULTISIG and `ONE_INPUT_VBYTES` 710 | **E** |
| `contracts/examples/btc_relay/src/lib.rs:22-30` | 2016, 14 d, 11, 2 h, 2,000 headers | Bitcoin consensus, and the relay's batch size | **E** |

### Swaps

| Where | Value | What / note | Class |
|---|---|---|---|
| `src/swap/mod.rs:36` `MIN_GAP_MS` | 4 h | The safety floor between the two deadlines | **D**: allow raising, never lowering |
| `src/swap/mod.rs:42,47` | 24 h, 12 h | Default deadlines. The CLI already has `--first` and `--second`. | **D**: add them to the wallet's Swaps page if it lacks them |
| `src/swap/mod.rs:320` BITCOIN_DEPTH | 3 | Already a wallet setting | OK |

### Economics: faucet and mining

| Where | Value | What / note | Class |
|---|---|---|---|
| `src/api/faucet.rs` | Per address and per IP only | There is no total budget per period, so many IPs drain the key. Limits live in memory (todo). | **C**: `--faucet-budget` per `--faucet-every` |
| `src/faucet.rs:22` `FAUCET_WINDOW_MS` | 10 min | Shared with the clients that compute the work | **E** |
| `src/mining/shares.rs:34` `SHARE_EVERY` | 10 s | The vardiff target | **C**: `--pool-share-every` |
| `src/mining/shares.rs:37-41` | MOST_SHARES 2^20, KEPT_BLOCKS 100 | Bounds on the pool's books. Shares are already bounded by `--pool-window`. | **C** (low) |
| `src/mining/pool.rs:33-45` | 256 connections, 32 per IP, 64 devices, 10 s handshake | Mining-port limits | **C** |
| `src/node/mod.rs:74-77` | TEMPLATE_REFRESH 2 s, MOST_SOLO 16 | Template rebuilds, and solo payouts per node | **C** |
| `src/api/rpc.rs:139`, `src/chain/fees/mod.rs:243` | eth_gasPrice = 2 × base, and offer = 2 × base + tip (tip 0) | The default fee suggestion | **C** for the node (`--suggest-multiplier`), **D** for a default tip in the wallet |

### Network (per node)

| Where | Value | What / note | Class |
|---|---|---|---|
| `src/net/bans.rs:25-28` | BAN_SCORE 100, SCORE_WINDOW 600 s | Ban threshold and decay. `--ban-for` already exists. | **C** |
| `src/net/backlog.rs:23-26` | 10 strikes in 60 s | **(todo)** "strike rules are fixed" | **C** |
| `src/net/mod.rs:36-45` | Sync caps of 128 blocks, 64 justifications, 1,024 headers, 512 entries | Server-side caps | **C** (and #15) |
| `src/net/swarm.rs:56-61` | TX_BATCH 256, 50 ms | Gossip batching | **C** (low) |
| `src/net/swarm.rs:113-126` | Redial 15 s, bootstrap 120 s, 3 dials at once, retry after 300 s | Dialling cadence | **C** (low) |
| `src/node/peers.rs:18` | 64 peers remembered | The peer store | **C** (low) |
| `src/net/tor.rs:41` | 90 s | Tor connect timeout | **C** |
| (none) | No inbound connection cap | **(todo)** "No connection or request limits" | **C**: `--max-peers-in` |

### HTTP API and RPC (per node)

| Where | Value | What / note | Class |
|---|---|---|---|
| `src/api/mod.rs:67-89` | Page sizes, defaults 20 to 100, maximums 100 to 1,000 | Listing sizes | **C** (low; a single `--api-max-page` would do) |
| `src/api/mod.rs:91` `MAX_BODY` | 2 MiB | Largest request body. Must cover a hex-encoded max module (#6). | **C**, derived from the limits |
| `src/api/mod.rs:93-107` | RANGE_PER_IP 2, RANGE_WAIT 2 s, MINING_RELAYS_PER_IP 32 | Per-client caps | **C** |
| `src/chain/range.rs:49` `RangeLimits::SERVED` | 100 blocks, 8 MiB, 4,096 entries, 1 s | `/blocks/range` | **C** |
| `src/api/rpc.rs:30-42` | 10k log blocks, 1k addresses and topics, batch 100, fee history 1,024 | RPC limits, like geth's `--rpc.*` | **C** |
| `src/api/filters.rs:19` | 1,000 txs scanned | Filter cost | **C** (low) |
| (none) | Concurrency per IP only | No per-second request rate | **C**: `--http-rate` |

### Node

| Where | Value | What / note | Class |
|---|---|---|---|
| `src/node/updates.rs:19` `CHECK_EVERY_MS` | 1 h | Update manifest polling | **C**: `--update-every` |
| `src/chain/finality/votes.rs:35-37` | MAX_PENDING 4,096, MAX_EVIDENCE 64 | Memory bounds | **C** (low) |
| `src/chain/mod.rs:132` | MAX_BAD_BLOCKS 1,024 | How many bad blocks are remembered | **C** (low) |
| `src/vm/lvm/mod.rs:32-33`, `src/vm/wasm/mod.rs:54-55` | Lua pool 16 states and 2 builders, Wasm cache 32 MiB and 1,024 modules | **(todo)** "Engine pools sized on the node" | **C** |
| `src/node/mod.rs:58` | EVENT_BUDGET 256 | Fairness of the node loop | **C** (low) |

### Apps

The apps are already well covered: node, proxy, Esplora, EVM RPCs, IPFS gateway, Bitcoin peer and depth, and lock time are all settings.

| Where | Value | What / note | Class |
|---|---|---|---|
| `wallet/src/views/trade.rs:37` `DEADLINE_MS` | 20 min | DEX trade deadline. Slippage is already settable. | **D** |
| wallet and explorer refresh timers (e.g. `wallet/src/chain.rs:32` 3 s, `explorer/src/app.rs:41-45` 1 to 5 s, many `REFRESH` constants of 3 to 15 s) | Polling | One "low data / battery" toggle that scales them all | **D** (low) |
| `ui/src/net.rs:12-21` | 30 s and 12 s, 90 s and 60 s through a proxy | HTTP timeouts. Slow Tor and I2P users. | **D** (low) |
| `explorer/src/reading.rs:45` `MOST_BYTES`, `wallet/src/reading.rs:65` `MOST_TXS` 20k | Storage caps for reading blocks | **D** |
| `ui/src/lib.rs:45` `PUBLIC_NODE` | `https://chain.homepage.gs` | Already overridable by `--node`, `CHAINV4_NODE` and settings. A build-time env would let forks ship their own. | **B** (build time) |

---

## 4. Leave as they are (E): the notable ones

- **Hash and signature domains:** `SEAL_DOMAIN`, `WORK_DOMAIN`, `DRAW_DOMAIN`, the gateway domains, `release DOMAIN`, the faucet label, EIP-712 `NAME`/`VERSION`, the Merkle `LEAF`/`NODE` bytes.
- **Encodings:** `STORAGE_FORMAT`, `PAYOUT_PARTS` 1e9, `MAX_DECIMALS` 77, BIP44/48/84 coin types.
- **`MAX_BATCH_STEPS` 32** (`src/structs/tx.rs:340`): baked into `batch_deploy_nonce` (`& 31`). Changing it changes contract addresses.
- **`MAX_NESTING` 2, `MAX_CLAIM_PATH` 64:** decode-time safety.
- **Block size bounds:** `MIN_BLOCK_BYTES`/`MAX_BLOCK_BYTES` 1 to 6 MiB (`src/params/mod.rs:94-101`) are tied to `MAX_GOSSIP` 8 MiB (`src/net/swarm.rs:109`). Raising one needs the other. The floors of 100k fuel and 1 MiB stay.
- **Luau compiler levels and `MAX_INSTRUCTION_COST`** (`src/vm/lvm/compiler.rs:13-17`, `src/vm/lvm/mod.rs:37`): they change instruction counts and so fuel. That is consensus, and never a knob.
- **`MOST_BOOTNODES` 32, `MOST_NAME_CHARS` 40, `MOST_EVM_GATEWAYS` 32.**
- **DEX `MINIMUM_LIQUIDITY` 1000.**
- **Snapshot `PIVOT_DEPTH` 16:** server and client must agree on it.
- **The light client's step and bundle constants.**

---

## 5. Counts per class

Each table row and top-15 item counts once. A dual class counts under the first class named.

| Class | Count | Items |
|---|---|---|
| **A** (governance) | 13 | 1, 3, 5, 6, 7, 8, 11, 12, 13; plus max_tx_bytes and MAX_RAW, MAX_MEMBERS, MAX_HISTORY derived from withdrawal_delay, and Directory replacing the contract flags |
| **B** (genesis only) | 6 | 4 (EVM spec at genesis, with A to follow), 9 (MAX_DRIFT, or A), max_reorg_depth, MAX_DRAW_WAIT, the name text limits, PUBLIC_NODE at build time |
| **C** (node operator) | 36 | 2, 15, and the network, API, node, mining, bridge-signer and faucet rows, including `--cancel-after` until it moves |
| **D** (app setting) | 8 | 10, swap MIN_GAP and deadlines, trade deadline, refresh toggle, timeouts, reading caps, default tip |
| **E** (leave it) | about 14 groups | Section 4, plus Bitcoin dust, size, signer and relay constants |
| Already in todo | 6 | vm::cost (#5), Engines, the DEX fee (#14), strike rules, engine pools, connection limits |

### New `Change` kinds suggested
- `Governance` (#1)
- `EvmSpec`, or folding it into `Upgrade` (#4)
- `TokenFunctions`, or merging it with Collections (#12)
- Optionally `Bridge` (#3), if not extending Federation

Everything else in class A extends an existing kind:
- `Fees`: costs, vote_reward_source
- `Limits`: contract limits, max_tx_bytes, multisig members
- `NamePrices` becomes `Names`
- `Consensus`: the PoW window and tuning
- `Finality`: slash_permille
- `Federation`: the BTC gateway parameters

### Two likely bugs to verify first
- #2: mempool fill by future nonces
- #15: sync responses over 64 MiB
