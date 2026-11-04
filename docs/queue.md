# Queue

Three jobs written for a session that starts knowing nothing of how they came about. Each stands on
its own, and they can run in any order. Written 2026-10-31, at commit `394d89c`: check what has moved
since with `git log`.

## Rules for all three

**Stop after every item.** Each job below is an item, and the review and the CI are one each. When one is done,
report it, give its commit message, and stop. Do not commit, and do not start the next, until the owner
says so, each time. A list of findings stops there too: fixing them waits until the owner has seen it.

**Never:**
- Open or read `validator-mnemonics.txt` at the repository's root, or print, log or upload a secret key
  or seed phrase. A command that shows key files must show public keys only: `chainv4 key show` does.
- Run GPU kernels. No `cargo test` in `gpu/`, and `chainv4-gpu` stays out of any workspace-wide test
  run. Its one test to run is by exact name:
  `cargo test -p chainv4-gpu --lib -- --exact tests::a_gpu_is_chosen_once_natively_and_once_through_opengl`.
- Push, or fetch: the GitHub repository is private, and its owner pushes. Commit only when asked.
- Swap the germ3n forks of mlua and Luau for upstream: they are what meters fuel.
- Use `git stash`: its stack is shared across worktrees.

**Ask first:**
- Before anything hard to undo, on a server above all. Redeploying or restarting a node, a chain reset,
  and uploading a file anywhere each need the owner's word for that occasion.
- On a real design choice, with the option you recommend first.

**Working here:**
- Read [Good to know](good-to-know.md) and the Conventions in [Development](development.md) first. If a rule here
  and a habit of yours differ, this page wins.
- `wallet/`, `explorer/` and `ui/` are cargo workspaces of their own. So is `contracts/`.
- Disk is tight: use `CARGO_INCREMENTAL=0`. An editor's flycheck may delete `target/` under you, so
  run nothing long-lived from a binary inside it.
- Scripts in bash, not zsh, which does not split words.
- The CLI for remote nodes is `~/.chainv4-pow/bin/chainv4`. A freshly built binary is blocked by the
  Mac's firewall (LuLu), which shows as `cannot reach the node: timeout`.
- The wiki's `build.rs` fails on a duplicate documented type name, a `see_also` that is not a page, or a
  link to a guide or heading that is not there.
- **Tests:** every change adds tests, updates those it changes the truth of, and removes those with nothing
  left to prove. Tests are named as sentences, such as
  `a_relay_left_on_a_branch_bitcoin_dropped_syncs_onto_the_one_it_took`. Never weaken a test just to make it pass.
  A test that fails in a build without some feature gets a gate on only the lines that need it, so the rest still
  runs in that build. Gating a whole test is for when all of it needs the feature.
- **Docs:** every change is written up as it lands: the guides in `docs/`, `docs/good-to-know.md` for what would
  surprise someone, and a `[x]` entry in `docs/todo.md`.
- **Checks before reporting an item,** in each crate touched (root, `ui`, `wallet`, `explorer`): `cargo clippy
  --all-targets` clean and `cargo test` for what changed. For `wallet` and `explorer`, also `cargo clippy --target
  wasm32-unknown-unknown`. For `wallet`, also `cargo clippy --no-default-features --features store --all-targets`,
  the store build. Touching iOS or `ui`'s platform code, `cargo check --target aarch64-apple-ios`. Say exactly
  which ran and how they came out. The full matrix is the CI item's, not every item's.
- **Code reads like what is around it:** its naming, idiom and comment density. Comments are full plain sentences
  saying why. Keep the change to what the item asks, and say what else you noticed rather than fix it.
- **UI:** phones matter, upright and sideways. Use the shared widgets in `ui/src/widgets.rs` and `layout.rs`, and
  draw new pages in tests at 360x740, 740x360 and 1200x800.
- **Destructive actions** in the apps, as removals, downgrades and showing keys, take a preview and a typed yes.
- **Algorithms** are each behind a default-on cargo feature with a pinned `u8` tag, and long-lived state is owned
  through `Node`, `Chain` and `Host`, with no `OnceLock` statics. [Development](development.md) has the rest.
- A commit message has a lowercase first line saying what it lets people do, then `- area: ...` bullets,
  then `- docs:` and `- tests:`, with no Co-Authored-By or Claude-Session lines. End each batch with one,
  ready to paste.
- Report only what you verified, and say plainly what you did not.

**Check before starting:** the setting for apps to read from checked blocks is being built, its first part
staged uncommitted in the main tree when this was written. If `GET /blocks/range` or the block-reading
setting is uncommitted, ask whether to wait, as it touches the same wallet files.

## Review, then the full CI

A review of what landed recently, then every check run once, which nothing has done since the chain's
newest features landed.

### The review

Find real bugs, not style. Report them first, most severe first, each with its file and line, a concrete
input that goes wrong, and what happens. Do not fix anything until the owner has seen the list.

In order of what a mistake would cost:
1. **What every node must agree on:** `Tx::Batch` (`src/structs/tx.rs`, `src/chain/execute.rs`), its nesting
   limit, the ABI decoder's cap, and anything that could make two nodes differ on a block.
2. **Keys and wallets:** the unlock modes (`wallet/src/guard.rs`, `vault.rs`, `biometrics.rs`, `lock.rs`,
   `views/settings/opening.rs`), and wallet storage (`wallet/src/store.rs`), where a mistake loses
   someone's wallet. Known and accepted: the HKDF intermediate is not wiped, and synced passkeys can
   release a secret on other devices.
3. **What strangers can send a node:** every API route's bounds, `GET /blocks/range` (`src/chain/range.rs`,
   `src/light/range.rs`) with its per-client gate, the mining port, and decoding of anything from a peer.
4. **What signers see:** that every signing screen shows what is really signed, batches and their steps
   above all (`wallet/src/steps.rs`, `views/steps.rs`, `views/sign.rs`), in the wallet and on the offline signer.
5. **The extension's message paths:** `wallet/extension/*.js`, which sit between web pages and the wallet.

Also check one thing that was never tried: **a new node syncing from genesis.** Consensus rules have changed
with no activation height, as the decoder cap and the nesting limit did, so they apply to old blocks on
replay. Reason through whether any block on the devnet could be judged differently now, and try it on a
private chain.

### The CI

Workflows run only when started by hand, to save the owner's runner credits, so do not add triggers
back. Run **`scripts/ci.sh`**, which covers what `.github/workflows/test.yml` does without spending any, and
never runs a GPU kernel. Its first run was on 2026-10-31: see `git log`. Do not run it in parallel with
other builds, as they share `target/`.

- `test.yml` runs `cargo test --workspace`. Check whether that takes in `chainv4-gpu`, and whether it should
  exclude it as the script does.
- A new feature belongs in the matrix, in `test.yml` and the script's list alike.
- **Narrow the gates that first run added,** as they were put on whole tests to get the matrix passing, and each
  loses coverage in the builds it skips. Say what each covers afterwards:
  - `a_step_reads_as_its_send_command_would_and_names_what_is_wrong` in `src/cli/tests.rs` is gated on `evm` for one
    `evm_call` step. Its transfer, burn and claim steps and its error messages need no EVM, so gate that step and
    its assertion alone, and renumber what follows.
  - `a_chain_without_an_admin_or_with_governance_lets_nobody_set_the_directory_by_hand` in `src/chain/tests.rs` only
    checks a council's genesis with `finality-ffg`. Give it finality that votes in an authority build too, so the
    check runs there.
  - `every_old_transaction_keeps_its_exact_bytes_and_id` and `every_old_receipt_keeps_its_exact_bytes_and_leaf` in
    `src/structs/tx/tests.rs` run only under the default hash and scheme, as their bytes are pinned to those. See
    whether what they guard can also be checked in other builds, such as each kind's tag, without the pinned hex.
- Android's C dependencies need `wallet/build-android.sh` rather than a plain check.

Fix what fails if the fix is plainly right, and report it. Report anything that touches consensus before
fixing it.

## Testnet exercises

Do by hand, on the public devnet, what tests have only done on private chains, and write down what happened.
The batches and the new wallet storage have never met a live chain.

**Where:** the devnet is described in [Deploying](deploy.md): `https://chain.homepage.gs`, and its
`$B`, `$N` and `$K` variables at the top. Keys are in `~/.chainv4-pow`. The coin has no value, but what
you send is public and permanent, so use small amounts and new accounts made for this. Do not touch the
validators' bonds, governance, or the relay's key, and restart nothing.

**Do each, from the CLI, and where it exists from the wallet:**
- **A swap:** chainv4 with an EVM testnet and with Bitcoin's testnet4 ([Swaps](swaps.md)), both to the end and
  once refunded after its deadline. Testnet coins may have to come from the owner: ask.
- **A trade:** both DEXes, native and EVM ([DEX](dex.md)): add liquidity, swap, take it out.
- **A bridge:** there and back, for a coin, a token and an NFT, through the Bitcoin relay as well
  ([Bridges](bridges.md)). The federation's keys are the owner's: ask before signing with them.
- **A multisig transaction** ([Multisig](multisig.md)): made, approved by its members, sent, and members changed.
- **A batch** ([Chain](chain.md#batches)): from an account and from a multisig, one that succeeds, and one with
  a failing step, confirming every step was undone. Check the wallet's and explorer's views of each.
- **The onion address, from outside:** through Tor to the explorer, the wallet and the API. `docker/pow.sh onion`
  on the public node prints the address: see [Deploying](deploy.md).

**Also watch:** node logs for panics and reorgs (`ssh -i ~/.ssh/id_kyun -o IdentitiesOnly=yes`, read-only), and
whether any app hits the rate limit by itself, as the wallet did.

**Report:** a table of each exercise: what you ran, the transaction ids, what the chain said, and what the apps
showed, with every difference from the guides. Correct guides that turn out wrong. A bug found goes in a list,
with what reproduces it. Anything needing a phone, a browser or the owner's hands, say so rather than skip it
silently.

## Pentest the chain

**Twice:** the first run started 2026-11-04; a second, the same brief over again, is queued for after every
item in `docs/todo.md` is in, as that work will have changed most of what this covers.

Attack the chain with malicious contracts and calls, in Lua, Wasm and the EVM, looking on purpose for what
review has only found by luck: the ABI decoder's blow-up, where a small call made every node allocate far more
than it paid for, and bytes nesting without end, which crashed whoever decoded them.

**Only on a private chain of your own, on this machine, never against the public devnet or its servers.** No
exploit goes anywhere but the owner's sight until fixed.

**What counts as a finding, worst first:**
1. **Two nodes disagree on a block:** anything non-deterministic, as time, hash order, floats, memory limits,
   fuel differing by platform or build, or a panic on one and not another. This splits the chain. Run the same blocks
   on differently built nodes, and x86 and arm if you can.
2. **A node dies or stalls:** a panic, an abort, stack overflow, running out of memory, or a hang, whether from a
   transaction, a block, a peer's message or an API call.
3. **Theft or broken rules:** escaping a sandbox, spending what is not yours, writing another contract's
   storage, minting coin, or skipping a fee.
4. **Work not paid for:** anything where a cheap call costs every node far more than its fee covers.

**Attack at least:**
- **Resources in each engine:** memory, fuel, stack, nesting depth, huge returns, huge and many logs, huge calldata,
  deep recursion, and the cost of each host function against what it charges. Check every place where work grows
  with something other than what fuel pays for.
- **The meters themselves:** whether Lua's fuel, in the germ3n Luau fork, can be escaped by long-running library
  calls, string operations, metamethods, `pcall` or coroutines. The same for Wasm, and gas in the EVM.
- **Reentry across engines:** Lua into Wasm into the EVM and back, with state and balance checks at each step.
- **Reentrancy within each engine:** a contract called back before it writes its state down, in Lua, Wasm and the
  EVM. Use hostile receivers on every callback the chain's own contracts make: `safe_transfer` to a contract, token
  and NFT hooks, the DEXes' swaps and liquidity, the swap contract's claim and refund, the gateways' withdrawals and
  refunds, the multisig's execution, and batches. Each is attacked through a receiver that calls back in to take
  twice, break a pool's invariant, or leave a balance its storage no longer matches.
- **State after a revert:** what a failed call or a failed batch step leaves behind, in storage, balances, logs,
  nonces, deployed contracts and names, and every cache between them.
- **Sandbox:** in Lua, what is reachable through `string`, `getmetatable`, `load` and the rest. In Wasm, imports,
  memory growth, tables and malformed modules. In the EVM, precompiles and any way into native contracts.
- **Hostile bytes:** calldata, ABI values, transactions, blocks and range answers. Fuzz every decoder, and check
  that allocation is bounded before reading.
- **Batches and multisigs:** nesting, steps that fight over one nonce or fee, and deploy addresses.

**How:** tests that reproduce go in `src/chain/tests/` with the rest. See `src/chain/tests/evm.rs`, `batch.rs`, and
`contracts/` for how contracts are built and run. Fuzzing is not set up yet and is wanted, in `docs/todo.md`.

**Report:** each finding with its class from above, a test that reproduces it, how cheap the attack is, and a proposed
fix. Also say what you attacked and found nothing in, so it is known. Ask before fixing what changes
consensus: such a fix needs every node at once, and as there are no activation heights yet, it must also not change
how any old block replays.
