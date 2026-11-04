-- The bridges' Bitcoin gateway (docs/bridges.md#bitcoin): BTC deposited to the federation is
-- minted here as wBTC, on a proof no one has to vouch for, and wBTC burned here is paid out in
-- BTC by the federation, through releases proposed and signed on this contract.
--
-- Deposits: each address here has its own deposit address on Bitcoin, a P2WSH of
--   <its 33 bytes> OP_DROP <chain id, 8 bytes><this gateway's 33 bytes> OP_DROP
--   <threshold> <signer keys…> <count> OP_CHECKMULTISIG
-- so any Bitcoin wallet pays it, and only the federation's threshold spends it. Once the payment
-- is bitcoin_depth deep in the Bitcoin light client contract (contracts/examples/btc_relay),
-- anyone shows it, and its value is minted to the address it was for, if it is at least the least
-- deposit the gateway was set up with. Bitcoin outlives a chain,
-- and a gateway deployed by the same keys on a new one has the same address and holds nothing yet,
-- so without the chain's id in the script the same proof would mint again there.
--
-- Releases: a withdrawal burns wBTC for a Bitcoin script and amount. A signer of the federation in
-- force proposes a release, as its fee is what the withdrawals it pays lose:
-- an unsigned Bitcoin transaction this contract checks pays exactly queued withdrawals from the
-- deposits it holds, less their shares of its fee, change back to the federation, its fee under the
-- cap. What it holds falls by just what was burned, so wBTC stays backed. The signers sign each
-- input as BIP143 does, and post their signatures here, each checked; anyone assembles the
-- witnesses and broadcasts it; and once it is deep enough, anyone shows it, and its inputs are
-- spent and its change held. A sweep is a release that pays everything an old federation holds
-- to the one in force. A release that cannot be finished is cancelled by the federation in force:
-- see M.cancel for why that cannot pay anyone twice. One with all its signatures that Bitcoin
-- leaves unmined is bumped: replaced by one paying more (M.replace), or paid for by a child
-- spending its change (M.child); each is a proposal of its own, signed as a release is, and
-- whichever Bitcoin mines is what is shown.
--
-- Events: DepositedBtc(txid, vout, sats, recipient), WithdrawRequested(id, script, sats, sender),
-- Proposed(proposal, txid), Replaced(proposal, replaced), Child(proposal, parent),
-- SignedBtc(proposal, input, signer), Released(proposal, txid), CancelVoted(proposal, signer),
-- Cancelled(proposal, txid).

local M = {}

local SIGHASH_ALL = 1
local MOST_WITHDRAWALS = 50
-- What a withdrawal is at least, in satoshis, whatever the floor asks; and what reaches its
-- script at least, above Bitcoin's dust for any output, so a fee never eats it whole.
local LEAST = 10000
local LEAST_PAYOUT = 1000
-- The most a withdrawal may lose to its share of a fee unless it says, as a part of it.
local DEFAULT_FEE_PART = 10
-- What a release repays of what children cost the backing unless the policy says: a fifth of its own
-- fee at most, and at most 500 satoshis from any one withdrawal, so the next withdrawers bear it
-- thinly, about five releases repaying a child's fee.
local DEFAULT_REPAY_PCT = 20
local DEFAULT_REPAY_MOST = 500
-- What Bitcoin's nodes relay at the least, without witnesses; an inner node of a Merkle tree is
-- 64 bytes, and a transaction of that size could be taken for one (CVE-2017-12842).
local LEAST_TX = 65
-- What change is at least: Bitcoin's dust for a P2WSH output, under which its nodes relay nothing.
-- Every release keeps one, which the withdrawals top up to this where it would be less, so a child
-- can always spend it to pay for the release.
local LEAST_CHANGE = 330
-- What a release's transaction is, and each of its inputs' sequence: fixed. 0xfffffffd signals
-- BIP125, so a replacement paying more takes its place in Bitcoin's mempools, and with its top bit
-- set it carries no BIP68 delay that keeps it out of a block while it holds what it spends.
local VERSION = "\2\0\0\0"
local SEQUENCE = "\253\255\255\255"
-- What a replacement pays past every version it replaces, a virtual byte: Bitcoin's incremental
-- relay fee, which its nodes ask of a replacement before they take it.
local LEAST_BUMP = 1
-- The most virtual bytes a release may be: past 100,000 Bitcoin's nodes do not relay it.
local MOST_VBYTES = 100000
-- The most a release spending one input can be, under the largest federation there is, 20 of 20,
-- paying a withdrawal and change. The fee cap has to pay this at the floor, or a floor could leave
-- no release at all it allows.
local ONE_INPUT_VBYTES = 710
-- What a deposit is at least unless the gateway was set up with another: dust minted as wBTC would
-- be dragged into releases that cost more to sign than it is worth.
local DEFAULT_LEAST_DEPOSIT = 10000
-- How many confirmations a coinbase needs before Bitcoin lets anything spend it, so before a
-- release spending one could be mined: minted sooner, it would hold up the release it went in.
local COINBASE_MATURITY = 100
-- How many blocks a lowered most fee waits before it holds, so a release proposed against the old
-- limit, up to 50 withdrawals with it, is not undone by one owner lowering theirs as it lands.
local LOWER_MOST_FEE_AFTER = 30

local utxos = storage.namespace("utxos")
local withdrawals = storage.namespace("withdrawals")
local proposals = storage.namespace("proposals")
local signatures = storage.namespace("signatures")
local votes = storage.namespace("votes")

local function owner_only()
    if ctx.sender:text() ~= storage.get("owner") then
        error("only the gateway's owner may")
    end
end

local function hex(bytes)
    return (bytes:gsub(".", function(byte)
        return string.format("%02x", string.byte(byte))
    end))
end

local function whole(value, what)
    local number = tonumber(tostring(value))

    if number == nil or number < 0 or number % 1 ~= 0 then
        error(what .. " is a whole number")
    end

    return number
end

local function count(key)
    return tonumber(storage.get(key) or "0")
end

-- Bitcoin's serialization: little-endian numbers and compact sizes.
local function le(number, size)
    local out = {}

    for _ = 1, size do
        table.insert(out, string.char(number % 256))
        number = math.floor(number / 256)
    end

    return table.concat(out)
end

local function compact(size)
    if size < 0xfd then
        return string.char(size)
    elseif size <= 0xffff then
        return "\253" .. le(size, 2)
    end

    return "\254" .. le(size, 4)
end

-- A reader over a transaction's bytes.
local function reader(bytes)
    local at = 1
    local r = {}

    function r.take(size)
        if at + size - 1 > #bytes then
            error("a transaction that ends early")
        end

        local taken = bytes:sub(at, at + size - 1)
        at = at + size
        return taken
    end

    function r.number(size)
        local taken, number = r.take(size), 0

        for index = size, 1, -1 do
            number = number * 256 + string.byte(taken, index)
        end

        return number
    end

    function r.compact()
        local first = r.number(1)

        -- Written longer than it needs, Bitcoin's nodes refuse it, and rewritten shorter it is
        -- another transaction than the one whose id a release was proposed with.
        if first == 0xfd then
            local size = r.number(2)

            if size < 0xfd then
                error("a size written longer than it needs")
            end

            return size
        elseif first == 0xfe then
            local size = r.number(4)

            if size <= 0xffff then
                error("a size written longer than it needs")
            end

            return size
        elseif first == 0xff then
            error("a size no transaction needs")
        end

        return first
    end

    function r.done()
        return at > #bytes
    end

    return r
end

-- A transaction without its witness, as its id is made of: its inputs' outpoints and sequences,
-- and its outputs, each kept as bytes too, as BIP143 hashes them.
local function parse(raw)
    local r = reader(raw)
    local tx = { inputs = {}, outputs = {} }
    tx.version = r.take(4)

    if raw:sub(5, 6) == "\0\1" then
        error("give the transaction without its witness")
    end

    for _ = 1, r.compact() do
        local outpoint = r.take(36)
        local script = r.take(r.compact())
        local sequence = r.take(4)
        table.insert(tx.inputs, { outpoint = outpoint, script = script, sequence = sequence })
    end

    for _ = 1, r.compact() do
        local value = r.take(8)
        local script = r.take(r.compact())
        table.insert(tx.outputs, { sats = reader(value).number(8), script = script, bytes = value .. compact(#script) .. script })
    end

    tx.locktime = r.take(4)

    if not r.done() then
        error("a transaction with more after it")
    end

    return tx
end

-- Scripts: a push of up to 75 bytes, a small number, a multisig, a deposit's, and P2WSH.
local function push(data)
    return string.char(#data) .. data
end

local function small(number)
    if number <= 16 then
        return string.char(0x50 + number)
    end

    return "\1" .. string.char(number)
end

local function multisig(threshold, keys)
    local parts = { small(threshold) }

    for _, key in ipairs(keys) do
        table.insert(parts, push(key))
    end

    table.insert(parts, small(#keys) .. "\174")
    return table.concat(parts)
end

local function deposit_script(recipient, threshold, keys)
    return push(recipient) .. "\117" .. push(le(chain.id(), 8) .. ctx.contract:encode()) .. "\117" .. multisig(threshold, keys)
end

local function p2wsh(script)
    return "\0\32" .. hash.sha256(script):bytes()
end

-- Whether Bitcoin's nodes relay an output to a script: P2PKH, P2SH or a witness program. A release
-- with any other output could be signed and never mined, holding all it spends.
local function relayed(script)
    local size, first = #script, string.byte(script, 1)

    if size == 25 and script:sub(1, 3) == "\118\169\20" and script:sub(24, 25) == "\136\172" then
        return true
    elseif size == 23 and script:sub(1, 2) == "\169\20" and script:sub(23, 23) == "\135" then
        return true
    elseif first == 0 then
        return (size == 22 or size == 34) and string.byte(script, 2) == size - 2
    end

    return first ~= nil and first >= 0x51 and first <= 0x60 and size >= 4 and size <= 42 and string.byte(script, 2) == size - 2
end

-- What the federation in force names of its Bitcoin policy, or what this gateway was set up with
-- while it names none: governance changes it with the federation, and it holds from the block
-- after a decision.
local function policy(name, stored)
    local fed = federation.get()
    local btc = fed and fed.btc

    if btc ~= nil then
        return btc[name]
    end

    return stored and tonumber(storage.get(stored))
end

local function fee_cap()
    return policy("fee_cap", "fee_cap")
end

local function fee_floor()
    return policy("fee_floor", "fee_floor")
end

local function grace_ms()
    return policy("grace_ms", "grace")
end

local function fee_part()
    return policy("fee_part", nil) or DEFAULT_FEE_PART
end

local function repay_pct()
    return policy("repay_pct", nil) or DEFAULT_REPAY_PCT
end

local function repay_most()
    return policy("repay_most", nil) or DEFAULT_REPAY_MOST
end

-- The federation in force.
local function in_force()
    return federation.get() or error("this chain has no federation")
end

-- The federations of each version, as the chain keeps every one it decided, read once a call:
-- every call starts afresh, and a release spending many outputs of one federation asks for it once.
local known = {}

-- A federation's threshold and keys by its version, and the block time the next was decided at,
-- once one was, after which its deposit addresses stay open for the grace period. The chain says
-- so, not what this gateway saw: one replaced before anything called the gateway is known all the same.
local function fed_at(version)
    version = whole(version, "the version")
    local fed = known[version]

    if fed == nil then
        fed = federation.at(version) or error("no federation of that version")
        known[version] = fed
    end

    return fed.threshold, fed.signers, fed.replaced_time
end

-- The witness script a held output was paid to: a deposit's, or for change, the bare multisig.
local function witness_script(version, recipient)
    local threshold, keys = fed_at(version)

    if recipient == "" then
        return multisig(threshold, keys)
    end

    return deposit_script(recipient, threshold, keys)
end

local function relay()
    return address.parse(storage.get("relay") or error("the gateway has no relay: setup first"))
end

-- Whether a transaction is in a Bitcoin block deep enough, by the relay: its id. The block's
-- coinbase and its path show how tall the block's tree is: see the relay's includes.
local function proven(block, raw, index, path, coinbase, coinbase_path, depth)
    if #raw == 64 then
        error("a transaction of 64 bytes: it could be an inner node of a Merkle tree")
    end

    local txid = crypto.sha256d(raw)

    if tonumber(tostring(contracts.call(relay(), "confirmations", block))) < depth then
        error("not deep enough yet in the relay's main chain")
    end

    if not contracts.call(relay(), "includes", block, txid, index, path, coinbase, coinbase_path) then
        error("the transaction is not in that block at that place")
    end

    return txid
end

local function wbtc()
    return address.parse(storage.get("wbtc"))
end

-- The height of the relay's tip.
local function relay_height(at)
    local _, height = contracts.call(at, "tip")

    return tonumber(tostring(height))
end

-- What an input spends, as the gateway keeps it: its transaction's id in hex and its output.
local function outpoint(input)
    return hex(input.outpoint:sub(1, 32)) .. ":" .. reader(input.outpoint:sub(33, 36)).number(4)
end

-- What a release's input weighs in its witness at most: the count of its items, CHECKMULTISIG's
-- extra pop, the threshold of signatures, each at most 73 bytes and a size, and the script.
local function witness_weight(threshold, script)
    return 2 + threshold * 74 + #compact(#script) + #script
end

-- Lists an output this gateway holds, by its place.
local function list(key)
    local n = count("utxo_count")
    utxos:set("at:" .. n, key)
    storage.set("utxo_count", tostring(n + 1))
end

-- Keeps an output this gateway holds, and lists it.
local function hold(key, sats, version, recipient)
    utxos:set(key .. ":sats", tostring(sats))
    utxos:set(key .. ":version", tostring(version))
    utxos:set(key .. ":recipient", recipient)
    utxos:set(key .. ":state", "unspent")
    list(key)
end

-- Keeps a release's change, shown mined. A child paying for it already knows it, unlisted, as
-- what it spends: it is listed now, still pending under that child, or spent if the child was
-- shown first, and only otherwise unspent. Each output is held once, whoever showed it first.
local function show_change(key, sats, version)
    local state = utxos:get(key .. ":state")

    if state == nil then
        return hold(key, sats, version, "")
    end

    if utxos:get(key .. ":unlisted") then
        utxos:set(key .. ":unlisted", nil)
        list(key)
    end

    if state == "unshown" then
        utxos:set(key .. ":state", "unspent")
    end
end

-- Run once by the chain: whoever deploys owns the gateway.
function M.init()
    storage.set("owner", ctx.sender:text())
end

-- The relay, started and synced to Bitcoin's tip, the wBTC token's template
-- (contracts/lua/wrapped.lua), the most fee a release may pay, in satoshis, the least it may pay a
-- virtual byte, and how long an old federation's deposit addresses stay open, in milliseconds; set
-- once, and what holds while the federation names no Bitcoin policy of its own, which governance
-- may set, and then holds instead. Then the least a deposit mints, in satoshis, 10,000 unless
-- given, which no policy changes. A chain restarted from the identical genesis has the same id and this gateway the same
-- address, so what is at or under the relay's tip now, before this gateway existed, is never a
-- deposit to it.
function M.setup(relay_address, template, fee_cap, fee_floor, grace_ms, least_deposit)
    owner_only()

    if storage.get("relay") then
        error("the gateway is set up already")
    end

    fee_cap, fee_floor = whole(fee_cap, "the fee cap"), whole(fee_floor, "the fee floor")

    if fee_floor < 1 then
        error("the fee floor is at least 1 satoshi a virtual byte")
    end

    if fee_cap < fee_floor * ONE_INPUT_VBYTES then
        error("a fee cap under what the floor asks of the smallest release: it would allow none")
    end

    storage.set("relay", relay_address:text())
    storage.set("first_height", tostring(relay_height(relay_address) + 1))
    storage.set("fee_cap", tostring(fee_cap))
    storage.set("fee_floor", tostring(fee_floor))
    storage.set("grace", tostring(whole(grace_ms, "the grace period")))
    storage.set("least_deposit", tostring(whole(least_deposit or DEFAULT_LEAST_DEPOSIT, "the least deposit")))
    in_force()
    local token = contracts.clone(template, "wbtc", "Wrapped Bitcoin", "wBTC", 8, ctx.contract)
    storage.set("wbtc", token:text())
    return token
end

-- What a deposit is at least to mint, in satoshis.
local function least_deposit()
    return tonumber(storage.get("least_deposit") or DEFAULT_LEAST_DEPOSIT)
end

function M.least_deposit()
    return least_deposit()
end

function M.wbtc()
    return wbtc()
end

-- The Bitcoin light client contract deposits and releases are proven to.
function M.relay()
    return relay()
end

-- An address's deposit script under the federation in force, and the P2WSH output that pays it:
-- what a wallet shows as its bc1q… deposit address.
function M.deposit_script(recipient)
    local fed = in_force()
    local script = deposit_script(recipient:encode(), fed.threshold, fed.signers)

    return script, p2wsh(script), fed.version
end

-- Shows a payment to a deposit address, bitcoin_depth deep, and mints its value as wBTC to the
-- address it was for. raw is the transaction without its witness; index and path are its place
-- and Merkle path in the block; coinbase and coinbase_path the block's first transaction, without
-- its witness, and its path; vout the output; version the federation the address was made under,
-- the one in force or any before it within the grace period of when the chain replaced it, whether
-- or not this gateway ever ran under it. A coinbase, at place 0, waits until Bitcoin lets it be
-- spent, 100 confirmations, as a release spending it could not be mined before.
function M.deposit(block, raw, index, path, coinbase, coinbase_path, vout, recipient, version)
    local fed = in_force()
    version = whole(version or fed.version, "the version")
    local threshold, keys, retired = fed_at(version)

    if retired and ctx.time > retired + grace_ms() then
        error("that federation's deposit addresses closed")
    end

    local depth = fed.bitcoin_depth

    if whole(index, "the place") == 0 then
        depth = math.max(depth, COINBASE_MATURITY)
    end

    local txid = proven(block, raw, index, path, coinbase, coinbase_path, depth)
    local confirmations = tonumber(tostring(contracts.call(relay(), "confirmations", block)))

    if relay_height(relay()) - confirmations + 1 < tonumber(storage.get("first_height")) then
        error("mined before this gateway was set up")
    end

    vout = whole(vout, "the output")
    local output = parse(raw).outputs[vout + 1] or error("no such output")

    if output.script ~= p2wsh(deposit_script(recipient:encode(), threshold, keys)) then
        error("the output does not pay that address's deposit address")
    end

    if output.sats < least_deposit() then
        error("a deposit mints at least " .. least_deposit() .. " satoshis")
    end

    local key = hex(txid) .. ":" .. vout

    if utxos:get(key .. ":state") then
        error("that deposit was minted already")
    end

    hold(key, output.sats, version, recipient:encode())
    contracts.call(wbtc(), "mint", recipient, u256(output.sats))
    events.emit("DepositedBtc", txid, vout, u256(output.sats), recipient)
    return output.sats
end

-- What a withdrawal is at least: enough to pay a release of its own at the floor and leave what
-- reaches its script.
local function least_withdrawal()
    return math.max(LEAST, fee_floor() * ONE_INPUT_VBYTES + LEAST_PAYOUT)
end

-- The most a withdrawal of this much may say it loses to fees, checked.
local function fee_limit(most_fee, sats)
    most_fee = whole(most_fee, "the most fee")

    if most_fee < 1 or most_fee > sats - LEAST_PAYOUT then
        error("the most a withdrawal may lose to fees is from 1 satoshi to all but " .. LEAST_PAYOUT .. " of it")
    end

    return most_fee
end

-- Burns wBTC to be paid out in BTC to a Bitcoin output script, such as a bc1q… address's. What
-- arrives is sats less its share of its release's fee, at most most_fee; a tenth of it unless told.
function M.withdraw(script, sats, most_fee)
    sats = whole(sats, "the amount")

    if sats < least_withdrawal() then
        error("a withdrawal pays at least " .. least_withdrawal() .. " satoshis")
    end

    most_fee = fee_limit(most_fee or math.floor(sats / fee_part()), sats)

    if not relayed(script) then
        error("an output script Bitcoin's nodes relay: P2PKH, P2SH or a witness program")
    end

    contracts.call(wbtc(), "burn", ctx.sender, u256(sats))
    local id = count("withdrawal_count") + 1
    storage.set("withdrawal_count", tostring(id))
    withdrawals:set(id .. ":script", script)
    withdrawals:set(id .. ":sats", tostring(sats))
    withdrawals:set(id .. ":most_fee", tostring(most_fee))
    withdrawals:set(id .. ":state", "queued")
    withdrawals:set(id .. ":by", ctx.sender:text())
    events.emit("WithdrawRequested", id, script, u256(sats), ctx.sender)
    return id
end

-- The most a withdrawal may lose to fees, as a release proposed now is judged by: what it agreed to,
-- or a lower limit it asked for once LOWER_MOST_FEE_AFTER blocks have passed, while it is queued. One
-- in a release keeps what the release was proposed under, for its bumps too, until it is queued
-- again; so a release planned against a limit is never undone by its owner lowering it as it lands.
local function lowering_holds(id)
    local from = withdrawals:get(id .. ":lowered_from")

    return from ~= nil and withdrawals:get(id .. ":state") == "queued" and ctx.height >= tonumber(from)
end

local function most_fee_of(id)
    if lowering_holds(id) then
        return tonumber(withdrawals:get(id .. ":lowered"))
    end

    return tonumber(withdrawals:get(id .. ":most_fee"))
end

-- Makes a lowering that holds now the limit, as a release takes the withdrawal or its owner asks
-- again: in a release, the limit it was proposed under is what stays.
local function settle_most_fee(id)
    if lowering_holds(id) then
        withdrawals:set(id .. ":most_fee", withdrawals:get(id .. ":lowered"))
        withdrawals:set(id .. ":lowered", nil)
        withdrawals:set(id .. ":lowered_from", nil)
    end

    return most_fee_of(id)
end

-- Who asked for a withdrawal changes the most it may lose to fees. Raised, it holds at once: as fees
-- rise past it while it waits, or in a release Bitcoin has not mined, so a bump its limit held back
-- can go, as what that release took stays within it. Lowered, only while it is queued, and it holds
-- LOWER_MOST_FEE_AFTER blocks later, as a release planned against the limit before may be on its way.
function M.set_most_fee(id, most_fee)
    id = whole(id, "the withdrawal")

    if withdrawals:get(id .. ":by") ~= ctx.sender:text() then
        error("only who asked for a withdrawal may")
    end

    local state = withdrawals:get(id .. ":state")
    local most = fee_limit(most_fee, tonumber(withdrawals:get(id .. ":sats")))

    if state ~= "queued" and state ~= "proposed" then
        error("a withdrawal that is not queued or in a release")
    end

    -- A lowering that has come to hold is the limit now, and one still waiting gives way to this.
    local now = settle_most_fee(id)
    withdrawals:set(id .. ":lowered", nil)
    withdrawals:set(id .. ":lowered_from", nil)

    if most >= now then
        withdrawals:set(id .. ":most_fee", tostring(most))
    elseif state == "proposed" then
        error("a withdrawal in a release only raises the most it may lose, for a bump")
    else
        withdrawals:set(id .. ":lowered", tostring(most))
        withdrawals:set(id .. ":lowered_from", tostring(ctx.height + LOWER_MOST_FEE_AFTER))
    end

    return true
end

-- What a withdrawal's share of a fee is: all of it evenly, the first in a release taking what will
-- not divide. Each output costs about the same to carry, whatever it pays.
local function fee_share(fee, count, at)
    return math.floor(fee / count) + (at == 1 and fee % count or 0)
end

-- What a signer of the federation in force signs to propose a release, posted by anyone: this chain,
-- this gateway, the number it will have, its transaction and the withdrawals it pays, so a signature
-- says nothing anywhere else, and nothing once any other release took that number. A release
-- cancelled is not brought back by what proposed it: proposing it again takes signing it again.
local function propose_digest(id, raw, ids)
    local named = {}

    for _, withdrawal in ipairs(ids) do
        table.insert(named, le(withdrawal, 8))
    end

    return crypto.sha256d("chainv4-btc-propose" .. le(chain.id(), 8) .. ctx.contract:encode() .. le(id, 8) .. crypto.sha256d(raw) .. le(#ids, 8) .. table.concat(named))
end

-- What a signer signs to propose a bump: a replacement of a proposal, or a child paying for one, each
-- under its own domain, so neither passes for the other or for a release.
local function bump_digest(domain, id, of, raw)
    return crypto.sha256d(domain .. le(chain.id(), 8) .. ctx.contract:encode() .. le(id, 8) .. le(of, 8) .. crypto.sha256d(raw))
end

-- Whether a key is a signer of the federation in force.
local function signer_of(fed, key)
    for _, signer in ipairs(fed.signers) do
        if signer == key then
            return true
        end
    end

    return false
end

-- A release, its replacements and the children paying for them make one tree: the release is its
-- root, each version of the release or of a child is in a group headed by the first of them, and
-- each bump says what it replaced, or for a child what it pays for.
local function root_of(id)
    return tonumber(proposals:get(id .. ":root") or id)
end

local function group_of(id)
    return tonumber(proposals:get(id .. ":group") or id)
end

local function kind_of(id)
    if id ~= group_of(id) then
        return "replacement"
    elseif id ~= root_of(id) then
        return "child"
    end

    return "release"
end

-- Every proposal of a release's tree, the release first.
local function tree(root)
    local found = { root }

    for member in (proposals:get(root .. ":members") or ""):gmatch("%d+") do
        table.insert(found, tonumber(member))
    end

    return found
end

local function state_of(id)
    return proposals:get(id .. ":state")
end

local function number_of(id, field)
    return tonumber(proposals:get(id .. ":" .. field) or "0")
end

-- The withdrawals a proposal pays, in order.
local function ids_of(id)
    local ids = {}

    for withdrawal in (proposals:get(id .. ":ids") or ""):gmatch("%d+") do
        table.insert(ids, withdrawal)
    end

    return ids
end

-- A transaction as every release is: version 2, no lock time, at least what Bitcoin's nodes relay,
-- each input without a script and with a release's sequence.
local function shaped(raw)
    local tx = parse(raw)

    if tx.version ~= VERSION then
        error("a release is a version 2 transaction")
    end

    if #raw < LEAST_TX then
        error("a release under " .. LEAST_TX .. " bytes, which Bitcoin's nodes do not relay")
    end

    if tx.locktime ~= "\0\0\0\0" then
        error("a release has no lock time")
    end

    for _, input in ipairs(tx.inputs) do
        if #input.script ~= 0 then
            error("an input with a script: a release spends witness outputs alone")
        end

        if input.sequence ~= SEQUENCE then
            error("an input with another sequence than a release's")
        end
    end

    return tx
end

-- What a transaction spending outputs this gateway knows weighs at most once signed, in virtual
-- bytes, and what it spends.
local function sized(raw, tx)
    local weight, spent = 4 * #raw + 2, 0

    for _, input in ipairs(tx.inputs) do
        local key = outpoint(input)
        local version = utxos:get(key .. ":version") or error("an input that is not an output the gateway holds unspent")
        spent = spent + tonumber(utxos:get(key .. ":sats"))
        weight = weight + witness_weight(fed_at(version), witness_script(version, utxos:get(key .. ":recipient")))
    end

    local vbytes = math.ceil(weight / 4)

    if vbytes > MOST_VBYTES then
        error("a release larger than Bitcoin's nodes relay: pay fewer, from fewer")
    end

    return vbytes, spent
end

local function paid_out(tx)
    local paid = 0

    for _, output in ipairs(tx.outputs) do
        paid = paid + output.sats
    end

    return paid
end

-- Checks a proposal's withdrawals are paid as asked: what each burned, less its share of what they
-- lose between them and what it repays, within what it agreed to lose, and in the state it must be.
local function pays_as_asked(tx, ids, cost, state, repays)
    for at, id in ipairs(ids) do
        local output = tx.outputs[at] or error("a withdrawal the transaction does not pay")

        if withdrawals:get(id .. ":state") ~= state then
            error("a withdrawal that is not " .. state)
        end

        local share = fee_share(cost, #ids, at) + repays[at]

        if share > most_fee_of(id) then
            error("a share of the fee past what withdrawal " .. id .. " agreed to lose")
        end

        if output.script ~= withdrawals:get(id .. ":script") or output.sats ~= tonumber(withdrawals:get(id .. ":sats")) - share then
            error("an output that does not pay its withdrawal as asked: what it burned, less its share of the fee and what it repays")
        end
    end
end

-- What children cost the backing that no open release repays yet.
local function owed()
    return math.max(0, count("shortfall") - count("repaying"))
end

-- What each withdrawal of a release repays of that: of a budget, the least of what is owed and
-- repay_pct of the release's fee, an even share, at most repay_most, and only what its limit leaves
-- past its share of the fee, so one at its limit repays nothing and waits for nothing. Worked out
-- from the gateway's state alone, as the planner and the gateway must agree on it.
local function repays_for(ids, cost, fee)
    local budget = math.min(owed(), math.floor(fee * repay_pct() / 100))
    local repays, total = {}, 0

    for at, id in ipairs(ids) do
        local room = most_fee_of(id) - fee_share(cost, #ids, at)
        repays[at] = math.max(0, math.min(fee_share(budget, #ids, at), repay_most(), room))
        total = total + repays[at]
    end

    return repays, total, budget
end

-- What each withdrawal of a proposal repays, as it was worked out when it was proposed; nothing for
-- one that pays none, as a child or a sweep.
local function repays_of(id, count)
    local repays = {}

    for repay in (proposals:get(id .. ":repays") or ""):gmatch("%d+") do
        table.insert(repays, tonumber(repay))
    end

    for at = 1, count do
        repays[at] = repays[at] or 0
    end

    return repays
end

-- What a withdrawal says it lost: its share of the release that paid it, or of the newest version
-- that may, and of any child shown paying for it.
local function record_fee(id, share)
    withdrawals:set(id .. ":fee", tostring(share + tonumber(withdrawals:get(id .. ":charged") or "0")))
end

-- Whether a proposal is short of what finishing it takes: an input under its threshold of
-- signatures here, or one Bitcoin already spent in another release, shown here, so it can never be
-- mined however many signed.
local function unfinished(id, tx)
    for index, input in ipairs(tx.inputs) do
        local key = outpoint(input)

        if utxos:get(key .. ":state") == "spent" then
            return true
        end

        local threshold, keys = fed_at(utxos:get(key .. ":version"))
        local count = 0

        for at = 0, #keys - 1 do
            if signatures:get(id .. ":" .. (index - 1) .. ":" .. at) then
                count = count + 1
            end
        end

        if count < threshold then
            return true
        end
    end

    return false
end

local function finished(id)
    return not unfinished(id, parse(proposals:get(id .. ":raw")))
end

-- A bump is of what Bitcoin may yet mine with all its signatures, and one at a time: while one waits
-- for its signatures, another would only compete with it, so it is refused, and one the signers
-- refuse is voted down alone, as M.cancel has it.
local function may_bump(of)
    if not finished(of) then
        error("only what holds all its signatures is bumped: what is short of them is cancelled")
    end

    for _, member in ipairs(tree(root_of(of))) do
        if state_of(member) == "open" and not finished(member) then
            error("another bump of this release waits for its signatures: one at a time")
        end
    end
end

-- Checks what a child adds to the fee of what it pays for can be charged to that one's
-- withdrawals: each one's share of the release's own, with its share of the child's, within what it
-- agreed to lose, and the two fees under the cap. The withdrawals have their outputs already, so
-- the federation pays it from its change, as M.released records.
local function charged_within(parent, fee)
    local ids, cost = ids_of(parent), number_of(parent, "cost")
    local repays = repays_of(parent, #ids)

    for at, id in ipairs(ids) do
        if fee_share(cost, #ids, at) + repays[at] + fee_share(fee, #ids, at) > most_fee_of(id) then
            error("a share of the child's fee, with its share of the release's, past what withdrawal " .. id .. " agreed to lose")
        end
    end

    if fee + number_of(parent, "fee") > fee_cap() then
        error("a fee of nothing, or past the cap, with that of the release it pays for")
    end
end

-- Keeps a proposal, as each kind of it is kept.
local function keep(id, raw, ids, version, fee, cost)
    storage.set("proposal_count", tostring(id))
    proposals:set(id .. ":raw", raw)
    proposals:set(id .. ":txid", crypto.sha256d(raw))
    proposals:set(id .. ":ids", ids)
    proposals:set(id .. ":version", tostring(version))
    proposals:set(id .. ":state", "open")
    proposals:set(id .. ":fee", tostring(fee))
    proposals:set(id .. ":cost", tostring(cost))
    events.emit("Proposed", id, crypto.sha256d(raw))
end

-- A bump joins its release's tree.
local function join(root, group, of, id)
    proposals:set(id .. ":root", tostring(root))
    proposals:set(id .. ":group", tostring(group))
    proposals:set(id .. ":of", tostring(of))
    local members = proposals:get(root .. ":members")
    proposals:set(root .. ":members", members and (members .. "," .. id) or tostring(id))
end

-- Proposes a release, signed by a signer of the federation in force, as btc_propose_digest has it:
-- what it pays in fees comes off the withdrawals, so a stranger could burn all they agreed to lose,
-- or hold them in a release no one signs. raw is an unsigned transaction without witnesses, spending
-- deposits and change this gateway holds, whose first outputs pay the given withdrawals in order, each
-- less its share of what they lose, and whose one more output is change to the federation in force:
-- all it spends past what those withdrawals burned, so the fee costs the backing nothing, or where
-- that is under Bitcoin's dust, dust, the withdrawals losing what tops it up with the fee. With no
-- withdrawals it is a sweep: everything it spends an old federation's, all of it to the one in
-- force, the fee owed to the backing once shown, as no withdrawal pays it. Whatever it proposes
-- Bitcoin must be able to mine, as its inputs and withdrawals wait on it until it is shown mined
-- or cancelled.
function M.propose(raw, signature, ...)
    local fed = in_force()
    local ids, named = {}, {}

    for _, id in ipairs({ ... }) do
        local number = whole(id, "a withdrawal")

        if named[number] then
            error("a withdrawal named twice")
        end

        named[number] = true
        table.insert(ids, number)
    end

    if not signer_of(fed, crypto.recover(propose_digest(count("proposal_count") + 1, raw, ids), signature)) then
        error("only a signer of the federation in force proposes a release")
    end

    if #ids > MOST_WITHDRAWALS then
        error("at most " .. MOST_WITHDRAWALS .. " withdrawals a release")
    end

    local tx = shaped(raw)
    local spends = {}

    for _, input in ipairs(tx.inputs) do
        local key = outpoint(input)

        if spends[key] then
            error("an input spent twice")
        end

        spends[key] = true

        if utxos:get(key .. ":state") ~= "unspent" or utxos:get(key .. ":unlisted") then
            error("an input that is not an output the gateway holds unspent")
        end

        if #ids == 0 and tonumber(utxos:get(key .. ":version")) >= fed.version then
            error("a sweep spends only what older federations hold")
        end
    end

    local vbytes, spent = sized(raw, tx)
    local paid = paid_out(tx)
    local fee = spent - paid

    if fee <= 0 or fee > fee_cap() then
        error("a fee of nothing, or past the cap")
    end

    if #tx.outputs ~= #ids + 1 then
        error("a release pays its withdrawals and one change to the federation in force, and nothing else")
    end

    local change = tx.outputs[#ids + 1]

    if change.script ~= p2wsh(multisig(fed.threshold, fed.signers)) then
        error("change goes to the federation in force alone")
    end

    if change.sats < LEAST_CHANGE then
        error("change under Bitcoin's dust")
    end

    local burned, cost, repays, repaid, budget = 0, 0, {}, 0, 0

    for _, id in ipairs(ids) do
        burned = burned + tonumber(withdrawals:get(id .. ":sats") or error("a withdrawal that is not queued"))
    end

    if #ids > 0 then
        if spent < burned then
            error("a release that spends less than its withdrawals burned")
        end

        -- Its change is what backs the wBTC left: just what it spends past what was burned, but where
        -- that is under dust, dust, which the withdrawals top up as they pay the fee; and what they
        -- repay of what children cost the backing, which goes back to it.
        local base = math.max(spent - burned, LEAST_CHANGE)
        cost = fee + base - (spent - burned)
        repays, repaid, budget = repays_for(ids, cost, fee)

        if change.sats ~= base + repaid then
            error("change that is not all it spends past what its withdrawals burned, or dust where that is less, with what they repay")
        end
    end

    pays_as_asked(tx, ids, cost, "queued", repays)

    for _, id in ipairs(ids) do
        -- See M.cancel: every release paying it must conflict with the ones cancelled before,
        -- until Bitcoin showed they can no longer be mined.
        local anchor = withdrawals:get(id .. ":anchor")

        if anchor and utxos:get(anchor .. ":state") ~= "spent" and not spends[anchor] then
            error("a withdrawal from a cancelled release, which must spend " .. anchor)
        end
    end

    if fee < vbytes * fee_floor() then
        error("a fee under the floor: this release pays at least " .. (vbytes * fee_floor()))
    end

    local id = count("proposal_count") + 1
    keep(id, raw, table.concat(ids, ","), fed.version, fee, cost)
    proposals:set(id .. ":repays", table.concat(repays, ","))
    proposals:set(id .. ":budget", tostring(budget))

    -- Counted as repaying until a version of it is shown or it is cancelled, so no other release
    -- repays the same again.
    if repaid > 0 then
        proposals:set(id .. ":repaid", tostring(repaid))
        storage.set("repaying", tostring(count("repaying") + repaid))
    end

    for _, input in ipairs(tx.inputs) do
        utxos:set(outpoint(input) .. ":state", "pending")
    end

    for at, withdrawal in ipairs(ids) do
        settle_most_fee(withdrawal)
        withdrawals:set(withdrawal .. ":state", "proposed")
        record_fee(withdrawal, fee_share(cost, #ids, at) + repays[at])
    end

    return id
end

-- A release no longer repaying, shown or cancelled, is no longer counted as repaying; shown, what it
-- repaid is repaid, as Bitcoin gave it to the federation's change. Each step down stops at nothing,
-- should one cancelled have been mined anyway after another took up what it would have repaid.
local function settle(root, shown)
    local repaid = proposals:get(root .. ":repaid")

    if repaid ~= nil then
        proposals:set(root .. ":repaid", nil)
        proposals:set(root .. ":settled", repaid)
        storage.set("repaying", tostring(math.max(0, count("repaying") - tonumber(repaid))))
    end

    if shown then
        local settled = number_of(root, "settled")
        proposals:set(root .. ":settled", nil)
        storage.set("shortfall", tostring(math.max(0, count("shortfall") - settled)))
    end
end

-- Replaces a release with all its signatures that Bitcoin has not mined, or a child paying for one,
-- by a version paying more, signed by a signer of the federation in force over btc_replace_digest.
-- It spends just what the one it replaces spends, so Bitcoin mines one of them at most, each input
-- signalling BIP125; it pays the same withdrawals to the same scripts, each less its even share of
-- what they now lose, within what it agreed to lose; its change is what it was, as the withdrawals
-- pay the bump as they paid the fee, but for a sweep's, which the federation pays it from, and a
-- child's, charged to what that pays for; and it pays more than every open version, by Bitcoin's
-- incremental rate, under the cap. Every version stays open until one is shown.
function M.replace(of, raw, signature)
    of = whole(of, "the proposal")
    local fed = in_force()

    if state_of(of) ~= "open" then
        error("no open release by that number")
    end

    if not signer_of(fed, crypto.recover(bump_digest("chainv4-btc-replace", count("proposal_count") + 1, of, raw), signature)) then
        error("only a signer of the federation in force bumps a release")
    end

    may_bump(of)
    local root, group = root_of(of), group_of(of)
    local old, tx = parse(proposals:get(of .. ":raw")), shaped(raw)
    local same = #tx.inputs == #old.inputs and #tx.outputs == #old.outputs

    for at, input in ipairs(tx.inputs) do
        same = same and input.outpoint == old.inputs[at].outpoint
    end

    for at, output in ipairs(tx.outputs) do
        same = same and output.script == old.outputs[at].script
    end

    if not same then
        error("a replacement spends what it replaces and pays the same outputs")
    end

    local vbytes, spent = sized(raw, tx)
    local fee = spent - paid_out(tx)

    if fee <= 0 or fee > fee_cap() then
        error("a fee of nothing, or past the cap")
    end

    -- What every version Bitcoin may hold in its place pays, so this one pays past all of them.
    local most = 0

    for _, member in ipairs(tree(root)) do
        if group_of(member) == group and state_of(member) == "open" then
            most = math.max(most, number_of(member, "fee"))
        end
    end

    if fee < most + vbytes * LEAST_BUMP then
        error("a replacement pays at least " .. (most + vbytes * LEAST_BUMP) .. ": more than every version it replaces, by a satoshi a virtual byte")
    end

    local ids, cost = ids_of(of), fee
    local change = tx.outputs[#tx.outputs]

    if change.sats < LEAST_CHANGE then
        error("change under Bitcoin's dust")
    end

    if group == root and #ids > 0 then
        if change.sats ~= old.outputs[#old.outputs].sats then
            error("a replacement keeps its change: its withdrawals pay what it adds, as they paid the fee")
        end

        cost = number_of(of, "cost") + fee - number_of(of, "fee")
        pays_as_asked(tx, ids, cost, "proposed", repays_of(of, #ids))
    elseif group == root then
        cost = 0
    else
        charged_within(number_of(group, "of"), fee)
    end

    local id = count("proposal_count") + 1
    keep(id, raw, proposals:get(of .. ":ids"), proposals:get(of .. ":version"), fee, cost)
    join(root, group, of, id)

    -- What it repays is the release's, as its change is: the extra fee comes off the withdrawals alone.
    if group == root then
        local repays = repays_of(of, #ids)
        proposals:set(id .. ":repays", table.concat(repays, ","))
        proposals:set(id .. ":budget", proposals:get(of .. ":budget") or "0")

        for at, withdrawal in ipairs(ids) do
            record_fee(withdrawal, fee_share(cost, #ids, at) + repays[at])
        end
    end

    events.emit("Replaced", id, of)
    return id
end

-- Proposes a child paying for a release with all its signatures that Bitcoin has not mined, or for
-- a replacement of one, signed by a signer of the federation in force over btc_child_digest: for
-- when a replacement cannot go, as when a recipient spent their output in a child Bitcoin will not
-- let one replace, or one went unmined too. It spends that version's change first, which every
-- release keeps for this, then outputs the gateway holds unspent only should that change not pay
-- its fee and leave dust; and pays all of it less its fee to the federation in force, in one
-- output, held like change once shown. Its fee is charged to what it pays for's withdrawals within
-- what each agreed to lose, with that one's own, the two under the cap. Bitcoin mines it only with
-- what it pays for. A child is bumped by replacing it, with M.replace.
function M.child(parent, raw, signature)
    parent = whole(parent, "the proposal")
    local fed = in_force()

    if state_of(parent) ~= "open" then
        error("no open release by that number")
    end

    if group_of(parent) ~= root_of(parent) then
        error("a child pays for a release or a replacement of one: replace a child to bump it")
    end

    if not signer_of(fed, crypto.recover(bump_digest("chainv4-btc-child", count("proposal_count") + 1, parent, raw), signature)) then
        error("only a signer of the federation in force bumps a release")
    end

    may_bump(parent)
    local ids, of = ids_of(parent), parse(proposals:get(parent .. ":raw"))
    local change = hex(proposals:get(parent .. ":txid")) .. ":" .. #ids
    local state = utxos:get(change .. ":state")

    if state == "pending" or state == "spent" then
        error("a child spends that change already: replace it to bump it")
    end

    local tx = shaped(raw)

    if outpoint(tx.inputs[1]) ~= change then
        error("a child spends the change of what it pays for, first")
    end

    if #tx.outputs ~= 1 or tx.outputs[1].script ~= p2wsh(multisig(fed.threshold, fed.signers)) or tx.outputs[1].sats < LEAST_CHANGE then
        error("a child pays the federation in force alone, at least Bitcoin's dust")
    end

    -- Known to the gateway, unlisted, so its signers can sign what spends it before it is shown.
    if state == nil then
        utxos:set(change .. ":sats", tostring(of.outputs[#ids + 1].sats))
        utxos:set(change .. ":version", proposals:get(parent .. ":version"))
        utxos:set(change .. ":recipient", "")
        utxos:set(change .. ":state", "unshown")
        utxos:set(change .. ":unlisted", "1")
    end

    local spends = { [change] = true }

    for at = 2, #tx.inputs do
        local key = outpoint(tx.inputs[at])

        if spends[key] then
            error("an input spent twice")
        end

        spends[key] = true

        if utxos:get(key .. ":state") ~= "unspent" or utxos:get(key .. ":unlisted") then
            error("an input that is not an output the gateway holds unspent")
        end
    end

    local vbytes, spent = sized(raw, tx)
    local fee = spent - tx.outputs[1].sats

    if fee <= 0 then
        error("a fee of nothing, or past the cap")
    end

    if #tx.inputs > 1 and tonumber(utxos:get(change .. ":sats")) >= fee + LEAST_CHANGE then
        error("a child spends more than that change only when the change alone cannot pay its fee and leave dust")
    end

    if fee < vbytes * fee_floor() then
        error("a fee under the floor: this child pays at least " .. (vbytes * fee_floor()))
    end

    charged_within(parent, fee)
    local id = count("proposal_count") + 1
    keep(id, raw, "", fed.version, fee, fee)
    join(root_of(parent), id, parent, id)

    for _, input in ipairs(tx.inputs) do
        utxos:set(outpoint(input) .. ":state", "pending")
    end

    events.emit("Child", id, parent)
    return id
end

-- What a signer signs for one input of a release: BIP143's preimage, SIGHASH_ALL.
local function preimage(tx, index, script, sats)
    local outpoints, sequences, outputs = {}, {}, {}

    for _, input in ipairs(tx.inputs) do
        table.insert(outpoints, input.outpoint)
        table.insert(sequences, input.sequence)
    end

    for _, output in ipairs(tx.outputs) do
        table.insert(outputs, output.bytes)
    end

    local input = tx.inputs[index + 1]

    return tx.version
        .. crypto.sha256d(table.concat(outpoints))
        .. crypto.sha256d(table.concat(sequences))
        .. input.outpoint
        .. compact(#script) .. script
        .. le(sats, 8)
        .. input.sequence
        .. crypto.sha256d(table.concat(outputs))
        .. tx.locktime
        .. le(SIGHASH_ALL, 4)
end

-- A signer's signature over one input of a release, a replacement or a child: r and s, 64 bytes,
-- low s, over its BIP143 preimage. It must be one of the keys of the federation the input's output
-- was paid to.
function M.sign(id, index, signature)
    id, index = whole(id, "the proposal"), whole(index, "the input")

    if state_of(id) ~= "open" then
        error("no open release by that number")
    end

    local tx = parse(proposals:get(id .. ":raw"))
    local input = tx.inputs[index + 1] or error("no such input")
    local key = outpoint(input)
    local version = utxos:get(key .. ":version")
    local script = witness_script(version, utxos:get(key .. ":recipient"))
    local message = preimage(tx, index, script, tonumber(utxos:get(key .. ":sats")))
    local _, keys = fed_at(version)

    for at, signer in ipairs(keys) do
        if crypto.verify("bitcoin", signer, message, signature) then
            signatures:set(id .. ":" .. index .. ":" .. (at - 1), signature)
            events.emit("SignedBtc", id, index, at - 1)
            return true
        end
    end

    error("not a signature of that input by its federation")
end

-- What a signer of the federation in force signs to vote to cancel a release: this chain, this
-- gateway, the release and its transaction, so a vote says nothing anywhere else.
local function cancel_digest(id)
    return crypto.sha256d("chainv4-btc-cancel" .. le(chain.id(), 8) .. ctx.contract:encode() .. le(id, 8) .. proposals:get(id .. ":txid"))
end

-- A child's group no longer open frees what it held: what else it spent is unspent again, and the
-- change it spent too, or unshown while what it paid for is not shown.
local function free_child(group)
    for _, member in ipairs(tree(root_of(group))) do
        if group_of(member) == group and state_of(member) == "open" then
            return
        end
    end

    for at, input in ipairs(parse(proposals:get(group .. ":raw")).inputs) do
        local key = outpoint(input)

        if utxos:get(key .. ":state") == "pending" then
            utxos:set(key .. ":state", (at == 1 and utxos:get(key .. ":unlisted")) and "unshown" or "unspent")
        end
    end
end

-- A signer's vote to cancel what cannot be finished, as btc_cancel_digest has it, posted by anyone;
-- once the threshold of the federation in force voted, it is cancelled, and what was signed for it
-- counts for nothing here. What has every input at its threshold is refused: anyone can still
-- finish it.
--
-- A release's number cancels all of its tree, as no version of it holds its signatures: its inputs
-- are unspent again and its withdrawals queued. A bump's number cancels that bump alone, one its
-- signers refuse: the release, the other versions and what they hold stand, and a child's own
-- inputs are freed once none of its group is open.
--
-- Bitcoin does not know a release was cancelled, and what a signer signed is public from the moment
-- it is sent, even in a transaction that lands too late, so a release under its threshold here may
-- still be finished there. So each withdrawal it paid is anchored to one of its inputs: every release
-- paying that withdrawal from now on must spend that input as well, and conflicting, only one of
-- them can ever be mined. The anchor lapses once it is shown spent, as nothing that spends it can
-- be mined after that. It is the input of the newest federation, as a release stuck on an old
-- federation no one can reach anymore is why a cancel is needed at all. A child or a replacement
-- needs none: every version of a release spends the same inputs, and a child pays no withdrawal.
function M.cancel(id, signature)
    id = whole(id, "the proposal")
    local fed = in_force()

    if state_of(id) ~= "open" then
        error("no open release by that number")
    end

    local root = root_of(id)
    local cancelling = { id }

    if id == root then
        cancelling = {}

        for _, member in ipairs(tree(root)) do
            if state_of(member) == "open" then
                if group_of(member) == root and finished(member) then
                    error("every input of it holds its threshold of signatures: anyone can still finish it")
                end

                table.insert(cancelling, member)
            end
        end
    elseif finished(id) then
        error("every input of it holds its threshold of signatures: anyone can still finish it")
    end

    local key = crypto.recover(cancel_digest(id), signature)

    if not signer_of(fed, key) then
        error("not signed by a signer of the federation in force")
    end

    votes:set(id .. ":" .. hex(key), "1")
    events.emit("CancelVoted", id, key)
    local count = 0

    for _, signer in ipairs(fed.signers) do
        if votes:get(id .. ":" .. hex(signer)) then
            count = count + 1
        end
    end

    if count < fed.threshold then
        return false
    end

    for _, member in ipairs(cancelling) do
        proposals:set(member .. ":state", "cancelled")
    end

    for _, member in ipairs(cancelling) do
        if group_of(member) ~= root then
            free_child(group_of(member))
        end
    end

    if id ~= root then
        events.emit("Cancelled", id, proposals:get(id .. ":txid"))
        return true
    end

    local tx = parse(proposals:get(id .. ":raw"))
    local anchor, newest = nil, -1

    for _, input in ipairs(tx.inputs) do
        local spends = outpoint(input)
        local version = tonumber(utxos:get(spends .. ":version"))

        if version > newest then
            anchor, newest = spends, version
        end

        -- One shown spent since stays so.
        if utxos:get(spends .. ":state") == "pending" then
            utxos:set(spends .. ":state", "unspent")
        end
    end

    settle(id, false)

    for _, withdrawal in ipairs(ids_of(id)) do
        -- One shown paid by a release cancelled before stays so.
        if withdrawals:get(withdrawal .. ":state") == "proposed" then
            withdrawals:set(withdrawal .. ":state", "queued")
            local held = withdrawals:get(withdrawal .. ":anchor")

            if held == nil or utxos:get(held .. ":state") == "spent" then
                withdrawals:set(withdrawal .. ":anchor", anchor)
            end
        end
    end

    events.emit("Cancelled", id, proposals:get(id .. ":txid"))
    return true
end

-- Whether a signer of the federation voted to cancel a release, by its key.
function M.voted(id, key)
    return votes:get(whole(id, "the proposal") .. ":" .. hex(key)) ~= nil
end

-- The input a withdrawal's next release must spend, after a cancel; nil once Bitcoin showed it
-- spent, or if none was ever cancelled.
function M.anchor(id)
    local anchor = withdrawals:get(whole(id, "the withdrawal") .. ":anchor")

    if anchor == nil or utxos:get(anchor .. ":state") == "spent" then
        return nil
    end

    return anchor
end

-- A proposal: its unsigned transaction, its id, its withdrawals, the federation it was proposed
-- under, and its state, open, released, cancelled, or replaced once another version of it or of
-- what it pays for was shown; then its kind, release, replacement or child, what it replaced or
-- pays for, its fee, and what its withdrawals lose between them by it; then what each of its
-- withdrawals repays of what children cost the backing, separated by commas, and the budget that was
-- worked out of.
function M.proposal(id)
    id = whole(id, "the proposal")

    if proposals:get(id .. ":raw") == nil then
        return nil
    end

    return proposals:get(id .. ":raw"), proposals:get(id .. ":txid"), proposals:get(id .. ":ids"), tonumber(proposals:get(id .. ":version")), state_of(id), kind_of(id), tonumber(proposals:get(id .. ":of") or ""), number_of(id, "fee"), number_of(id, "cost"), proposals:get(id .. ":repays") or "", number_of(id, "budget")
end

-- The release a proposal is of, and every bump of it in the order proposed, separated by commas.
function M.tree(id)
    local root = root_of(whole(id, "the proposal"))

    return root, proposals:get(root .. ":members") or ""
end

-- The signatures posted for one input, as signer number and signature, in the keys' order, and
-- the witness script and threshold: what assembling its witness takes. A cancelled release's
-- count for nothing, so none are given.
function M.input_signatures(id, index)
    id, index = whole(id, "the proposal"), whole(index, "the input")
    local tx = parse(proposals:get(id .. ":raw") or error("no release by that number"))
    local input = tx.inputs[index + 1] or error("no such input")
    local key = outpoint(input)
    local version = utxos:get(key .. ":version")
    local threshold, keys = fed_at(version)
    local found = {}

    for at = 0, #keys - 1 do
        local signature = signatures:get(id .. ":" .. index .. ":" .. at)

        if signature and state_of(id) ~= "cancelled" then
            table.insert(found, at)
            table.insert(found, signature)
        end
    end

    return witness_script(version, utxos:get(key .. ":recipient")), threshold, table.unpack(found)
end

-- Shows a proposal, bitcoin_depth deep, with its block's coinbase and that one's path. A version of a
-- release, the release itself or a replacement: its inputs are spent, its withdrawals paid at its
-- shares, its change held for the federation it was proposed under, and every other version of it,
-- and every child of those, closed as replaced, as Bitcoin can never mine them now. A child: what it
-- spent is spent, its output held, the other versions of it closed, and its fee charged to what it
-- paid for's withdrawals, which the federation paid from its change. One cancelled may still be
-- shown, if it was finished anyway: Bitcoin mined it, so what it spent and paid is what the gateway
-- must know. Every output is held once, as no two of what spends the same inputs can be mined.
function M.released(id, block, raw, index, path, coinbase, coinbase_path)
    id = whole(id, "the proposal")
    local fed = in_force()
    local state = state_of(id)

    if state ~= "open" and state ~= "cancelled" then
        error("no open release by that number")
    end

    local txid = proven(block, raw, index, path, coinbase, coinbase_path, fed.bitcoin_depth)

    if txid ~= proposals:get(id .. ":txid") then
        error("not that release's transaction")
    end

    local tx = parse(proposals:get(id .. ":raw"))
    local root, group = root_of(id), group_of(id)

    -- Cancelled and proposed again as it was, it has the same id: shown once is enough.
    for _, input in ipairs(tx.inputs) do
        if utxos:get(outpoint(input) .. ":state") == "spent" then
            error("an input of it was shown spent already")
        end
    end

    for _, input in ipairs(tx.inputs) do
        utxos:set(outpoint(input) .. ":state", "spent")
    end

    -- What Bitcoin can no longer mine: for a version of the release, every other version and every
    -- child of those; for a child, every other version of it.
    local others = {}

    for _, member in ipairs(tree(root)) do
        local theirs = group_of(member)
        local closed = (group == root and (theirs == root or number_of(theirs, "of") ~= id)) or (group ~= root and theirs == group)

        if member ~= id and state_of(member) == "open" and closed then
            table.insert(others, member)
        end
    end

    for _, member in ipairs(others) do
        proposals:set(member .. ":state", "replaced")
    end

    for _, member in ipairs(others) do
        if group == root and group_of(member) ~= root then
            free_child(group_of(member))
        end
    end

    proposals:set(id .. ":state", "released")

    if group == root then
        local ids, cost = ids_of(id), number_of(id, "cost")
        local repays = repays_of(id, #ids)

        for at, withdrawal in ipairs(ids) do
            withdrawals:set(withdrawal .. ":state", "paid")
            record_fee(withdrawal, fee_share(cost, #ids, at) + repays[at])
        end

        -- Every version repays the same, the release's, so it is settled once, by the release.
        settle(root, true)

        -- A sweep pays no withdrawal, so its fee came out of what backs wBTC: owed to the backing, as
        -- a child's is, for later releases to repay.
        if #ids == 0 then
            storage.set("shortfall", tostring(count("shortfall") + number_of(id, "fee")))
        end

        show_change(hex(txid) .. ":" .. #ids, tx.outputs[#ids + 1].sats, proposals:get(id .. ":version"))

        if id ~= root then
            proposals:set(root .. ":state", "replaced")
        end
    else
        hold(hex(txid) .. ":0", tx.outputs[1].sats, proposals:get(id .. ":version"), "")
        local paid_for, fee = ids_of(number_of(group, "of")), number_of(id, "fee")

        for at, withdrawal in ipairs(paid_for) do
            local share = fee_share(fee, #paid_for, at)
            withdrawals:set(withdrawal .. ":charged", tostring(tonumber(withdrawals:get(withdrawal .. ":charged") or "0") + share))
            withdrawals:set(withdrawal .. ":fee", tostring(tonumber(withdrawals:get(withdrawal .. ":fee") or "0") + share))
        end

        -- The withdrawals have their outputs already, so what the child paid came from the
        -- federation's change: what the gateway holds falls short of wBTC by it.
        storage.set("shortfall", tostring(count("shortfall") + fee))
    end

    events.emit("Released", id, txid)
    return true
end

-- What the federation paid from its change for children and sweeps and later releases have not repaid
-- yet, in satoshis: what the gateway holds falls short of the wBTC it backs by this, as a child's fee
-- is charged to withdrawals already paid and a sweep pays none. Then how much of it open releases repay, once shown: a release
-- proposed repays only what is left past that.
function M.shortfall()
    return count("shortfall"), count("repaying")
end

-- What the gateway holds, by its place in the list: the outpoint, satoshis, federation version,
-- what it is for (an address's 33 bytes, or empty for change) and its state.
function M.utxo(n)
    local key = utxos:get("at:" .. whole(n, "the place"))

    if key == nil then
        return nil
    end

    return key, tonumber(utxos:get(key .. ":sats")), tonumber(utxos:get(key .. ":version")), utxos:get(key .. ":recipient"), utxos:get(key .. ":state")
end

function M.utxo_count()
    return count("utxo_count")
end

-- What the gateway holds at an outpoint, as utxo names it: satoshis, federation version, what it is
-- for and its state, then the witness script spending it and its threshold: what checking or
-- sizing a release takes. nil for anything else.
function M.utxo_of(key)
    local sats = utxos:get(key .. ":sats")

    if sats == nil then
        return nil
    end

    local version, recipient = utxos:get(key .. ":version"), utxos:get(key .. ":recipient")
    local threshold = fed_at(version)

    return tonumber(sats), tonumber(version), recipient, utxos:get(key .. ":state"), witness_script(version, recipient), threshold
end

-- The most fee a release may pay, in satoshis, and the least it may pay a virtual byte: the
-- federation's policy, or what the gateway was set up with.
function M.fee_limits()
    return fee_cap(), fee_floor()
end

-- A withdrawal: its output script, the satoshis it burned, its state: queued, proposed or paid; who
-- asked; the most it may lose to fees, as a release proposed now is judged by; its share of the fee
-- of the release it is in, what it arrives less of, once it is in one; and a lower limit it asked
-- for that does not hold yet, with the height it holds from while queued, or nil for none.
function M.withdrawal(id)
    id = whole(id, "the withdrawal")

    local by = withdrawals:get(id .. ":by")
    local most = by and most_fee_of(id)
    local lowered, from = withdrawals:get(id .. ":lowered"), withdrawals:get(id .. ":lowered_from")

    -- One that holds already is the limit given before it.
    if lowering_holds(id) then
        lowered, from = nil, nil
    end

    return withdrawals:get(id .. ":script"), tonumber(withdrawals:get(id .. ":sats")), withdrawals:get(id .. ":state"), by and address.parse(by), most, tonumber(withdrawals:get(id .. ":fee") or ""), tonumber(lowered or ""), tonumber(from or "")
end

-- What a withdrawal is at least, in satoshis.
function M.least_withdrawal()
    return least_withdrawal()
end

function M.withdrawal_count()
    return count("withdrawal_count")
end

function M.proposal_count()
    return count("proposal_count")
end

-- The change script of the federation in force, or of an earlier one by its version: the bare
-- multisig its change goes to.
function M.change_script(version)
    if version ~= nil then
        return witness_script(tostring(whole(version, "the version")), "")
    end

    local fed = in_force()

    return multisig(fed.threshold, fed.signers)
end

return M
