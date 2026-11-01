-- The bridges' Bitcoin gateway (docs/bridges.md#bitcoin): BTC deposited to the federation is
-- minted here as wBTC, on a proof no one has to vouch for, and wBTC burned here is paid out in
-- BTC by the federation, through releases proposed and signed on this contract.
--
-- Deposits: each address here has its own deposit address on Bitcoin, a P2WSH of
--   <its 33 bytes> OP_DROP <chain id, 8 bytes><this gateway's 33 bytes> OP_DROP
--   <threshold> <signer keys…> <count> OP_CHECKMULTISIG
-- so any Bitcoin wallet pays it, and only the federation's threshold spends it. Once the payment
-- is bitcoin_depth deep in the Bitcoin light client contract (contracts/examples/btc_relay),
-- anyone shows it, and its value is minted to the address it was for. Bitcoin outlives a chain,
-- and a gateway deployed by the same keys on a new one has the same address and holds nothing yet,
-- so without the chain's id in the script the same proof would mint again there.
--
-- Releases: a withdrawal burns wBTC for a Bitcoin script and amount. Anyone proposes a release:
-- an unsigned Bitcoin transaction this contract checks pays exactly queued withdrawals from the
-- deposits it holds, change back to the federation, its fee under the cap. The signers sign each
-- input as BIP143 does, and post their signatures here, each checked; anyone assembles the
-- witnesses and broadcasts it; and once it is deep enough, anyone shows it, and its inputs are
-- spent and its change held. A sweep is a release that pays everything an old federation holds
-- to the one in force. A release that cannot be finished is cancelled by the federation in force:
-- see M.cancel for why that cannot pay anyone twice.
--
-- Events: DepositedBtc(txid, vout, sats, recipient), WithdrawRequested(id, script, sats, sender),
-- Proposed(proposal, txid), SignedBtc(proposal, input, signer), Released(proposal, txid),
-- CancelVoted(proposal, signer), Cancelled(proposal, txid).

local M = {}

local SIGHASH_ALL = 1
local MOST_WITHDRAWALS = 50
-- What a withdrawal pays at least, in satoshis: above Bitcoin's dust for any output.
local LEAST = 1000
-- What change is at least: Bitcoin's nodes relay nothing with an output under its dust.
local LEAST_CHANGE = 546
-- What a release's transaction is, and each of its inputs' sequence: fixed, so none of them can
-- carry a BIP68 delay that keeps it out of a block while it holds what it spends.
local VERSION = "\2\0\0\0"
local SEQUENCE = "\255\255\255\255"
-- The most virtual bytes a release may be: past 100,000 Bitcoin's nodes do not relay it.
local MOST_VBYTES = 100000
-- The most a release spending one input can be, under the largest federation there is, 20 of 20,
-- paying a withdrawal and change. The fee cap has to pay this at the floor, or a floor could leave
-- no release at all it allows.
local ONE_INPUT_VBYTES = 710

local feds = storage.namespace("feds")
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

-- The federations this gateway has seen, by version: their threshold and keys, and when the next
-- took over, after which their deposit addresses stay open for the grace period.
local function remember()
    local fed = federation.get()

    if fed == nil then
        error("this chain has no federation")
    end

    local version = tostring(fed.version)

    if feds:get(version .. ":threshold") == nil then
        local latest = storage.get("fed_latest")

        if latest then
            feds:set(latest .. ":retired", tostring(ctx.time))
        end

        feds:set(version .. ":threshold", tostring(fed.threshold))
        feds:set(version .. ":keys", table.concat(fed.signers))
        storage.set("fed_latest", version)
    end

    return fed
end

local function fed_at(version)
    local threshold = feds:get(version .. ":threshold")

    if threshold == nil then
        error("a federation this gateway never saw")
    end

    local joined, keys = feds:get(version .. ":keys"), {}

    for at = 1, #joined, 33 do
        table.insert(keys, joined:sub(at, at + 32))
    end

    return tonumber(threshold), keys, tonumber(feds:get(version .. ":retired") or "")
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

-- Whether a transaction is in a Bitcoin block deep enough, by the relay: its id.
local function proven(block, raw, index, path, depth)
    local txid = crypto.sha256d(raw)

    if tonumber(tostring(contracts.call(relay(), "confirmations", block))) < depth then
        error("not deep enough yet in the relay's main chain")
    end

    if not contracts.call(relay(), "includes", block, txid, index, path) then
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

-- Keeps an output this gateway holds, and lists it.
local function hold(key, sats, version, recipient)
    utxos:set(key .. ":sats", tostring(sats))
    utxos:set(key .. ":version", tostring(version))
    utxos:set(key .. ":recipient", recipient)
    utxos:set(key .. ":state", "unspent")
    local n = count("utxo_count")
    utxos:set("at:" .. n, key)
    storage.set("utxo_count", tostring(n + 1))
end

-- Run once by the chain: whoever deploys owns the gateway.
function M.init()
    storage.set("owner", ctx.sender:text())
end

-- The relay, started and synced to Bitcoin's tip, the wBTC token's template
-- (contracts/lua/wrapped.lua), the most fee a release may pay, in satoshis, the least it may pay a
-- virtual byte, and how long an old federation's deposit addresses stay open, in milliseconds; set
-- once. A chain restarted from the identical genesis has the same id and this gateway the same
-- address, so what is at or under the relay's tip now, before this gateway existed, is never a
-- deposit to it.
function M.setup(relay_address, template, fee_cap, fee_floor, grace_ms)
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
    local token = contracts.clone(template, "wbtc", "Wrapped Bitcoin", "wBTC", 8, ctx.contract)
    storage.set("wbtc", token:text())
    remember()
    return token
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
    local fed = remember()
    local script = deposit_script(recipient:encode(), fed.threshold, fed.signers)

    return script, p2wsh(script), fed.version
end

-- Shows a payment to a deposit address, bitcoin_depth deep, and mints its value as wBTC to the
-- address it was for. raw is the transaction without its witness; index and path are its place
-- and Merkle path in the block; vout the output; version the federation the address was made
-- under, the one in force or one retired within the grace period.
function M.deposit(block, raw, index, path, vout, recipient, version)
    local fed = remember()
    version = tostring(whole(version or fed.version, "the version"))
    local threshold, keys, retired = fed_at(version)

    if retired and ctx.time > retired + tonumber(storage.get("grace")) then
        error("that federation's deposit addresses closed")
    end

    local txid = proven(block, raw, index, path, fed.bitcoin_depth)
    local depth = tonumber(tostring(contracts.call(relay(), "confirmations", block)))

    if relay_height(relay()) - depth + 1 < tonumber(storage.get("first_height")) then
        error("mined before this gateway was set up")
    end

    vout = whole(vout, "the output")
    local output = parse(raw).outputs[vout + 1] or error("no such output")

    if output.script ~= p2wsh(deposit_script(recipient:encode(), threshold, keys)) then
        error("the output does not pay that address's deposit address")
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

-- Burns wBTC to be paid out in BTC to a Bitcoin output script, such as a bc1q… address's.
function M.withdraw(script, sats)
    sats = whole(sats, "the amount")

    if sats < LEAST then
        error("a withdrawal pays at least " .. LEAST .. " satoshis")
    end

    if not relayed(script) then
        error("an output script Bitcoin's nodes relay: P2PKH, P2SH or a witness program")
    end

    contracts.call(wbtc(), "burn", ctx.sender, u256(sats))
    local id = count("withdrawal_count") + 1
    storage.set("withdrawal_count", tostring(id))
    withdrawals:set(id .. ":script", script)
    withdrawals:set(id .. ":sats", tostring(sats))
    withdrawals:set(id .. ":state", "queued")
    withdrawals:set(id .. ":by", ctx.sender:text())
    events.emit("WithdrawRequested", id, script, u256(sats), ctx.sender)
    return id
end

-- Proposes a release: raw, an unsigned transaction without witnesses, spending deposits and
-- change this gateway holds, whose first outputs pay the given withdrawals in order, and whose
-- one more output, if any, is change to the federation in force. With no withdrawals it is a
-- sweep: everything it spends an old federation's, all of it to the one in force. Whatever it
-- proposes Bitcoin must be able to mine, as its inputs and withdrawals wait on it until it is
-- shown mined or cancelled.
function M.propose(raw, ...)
    local fed = remember()
    local ids, named = {}, {}

    for _, id in ipairs({ ... }) do
        local number = whole(id, "a withdrawal")

        if named[number] then
            error("a withdrawal named twice")
        end

        named[number] = true
        table.insert(ids, number)
    end

    local tx = parse(raw)

    if #ids > MOST_WITHDRAWALS then
        error("at most " .. MOST_WITHDRAWALS .. " withdrawals a release")
    end

    if tx.version ~= VERSION then
        error("a release is a version 2 transaction")
    end

    if tx.locktime ~= "\0\0\0\0" then
        error("a release has no lock time")
    end

    local spent, weight, spends = 0, 4 * #raw + 2, {}

    for _, input in ipairs(tx.inputs) do
        local key = outpoint(input)

        if spends[key] then
            error("an input spent twice")
        end

        spends[key] = true

        if utxos:get(key .. ":state") ~= "unspent" then
            error("an input that is not an output the gateway holds unspent")
        end

        if #input.script ~= 0 then
            error("an input with a script: a release spends witness outputs alone")
        end

        if input.sequence ~= SEQUENCE then
            error("an input with another sequence than a release's")
        end

        local version = utxos:get(key .. ":version")

        if #ids == 0 and tonumber(version) >= fed.version then
            error("a sweep spends only what older federations hold")
        end

        spent = spent + tonumber(utxos:get(key .. ":sats"))
        weight = weight + witness_weight(fed_at(version), witness_script(version, utxos:get(key .. ":recipient")))
    end

    local paid = 0

    for at, id in ipairs(ids) do
        local output = tx.outputs[at] or error("a withdrawal the transaction does not pay")

        if withdrawals:get(id .. ":state") ~= "queued" then
            error("a withdrawal that is not queued")
        end

        if output.script ~= withdrawals:get(id .. ":script") or output.sats ~= tonumber(withdrawals:get(id .. ":sats")) then
            error("an output that does not pay its withdrawal as asked")
        end

        -- See M.cancel: every release paying it must conflict with the ones cancelled before,
        -- until Bitcoin showed they can no longer be mined.
        local anchor = withdrawals:get(id .. ":anchor")

        if anchor and utxos:get(anchor .. ":state") ~= "spent" and not spends[anchor] then
            error("a withdrawal from a cancelled release, which must spend " .. anchor)
        end

        paid = paid + output.sats
    end

    local change = tx.outputs[#ids + 1]

    if #tx.outputs > #ids + 1 or (#ids == 0 and change == nil) then
        error("outputs past the withdrawals and one change")
    end

    if change then
        if change.script ~= p2wsh(multisig(fed.threshold, fed.signers)) then
            error("change goes to the federation in force alone")
        end

        if change.sats < LEAST_CHANGE then
            error("change under Bitcoin's dust: leave it to the fee")
        end

        paid = paid + change.sats
    end

    local fee = spent - paid

    if fee <= 0 or fee > tonumber(storage.get("fee_cap")) then
        error("a fee of nothing, or past the cap")
    end

    local vbytes = math.ceil(weight / 4)

    if vbytes > MOST_VBYTES then
        error("a release larger than Bitcoin's nodes relay: pay fewer, from fewer")
    end

    if fee < vbytes * tonumber(storage.get("fee_floor")) then
        error("a fee under the floor: this release pays at least " .. (vbytes * tonumber(storage.get("fee_floor"))))
    end

    local id = count("proposal_count") + 1
    storage.set("proposal_count", tostring(id))
    local txid = crypto.sha256d(raw)
    proposals:set(id .. ":raw", raw)
    proposals:set(id .. ":txid", txid)
    proposals:set(id .. ":ids", table.concat(ids, ","))
    proposals:set(id .. ":version", tostring(fed.version))
    proposals:set(id .. ":state", "open")

    for _, input in ipairs(tx.inputs) do
        utxos:set(outpoint(input) .. ":state", "pending")
    end

    for _, withdrawal in ipairs(ids) do
        withdrawals:set(withdrawal .. ":state", "proposed")
    end

    events.emit("Proposed", id, txid)
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

-- A signer's signature over one input of a release: r and s, 64 bytes, low s, over its BIP143
-- preimage. It must be one of the keys of the federation the input's output was paid to.
function M.sign(id, index, signature)
    id, index = whole(id, "the proposal"), whole(index, "the input")

    if proposals:get(id .. ":state") ~= "open" then
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

-- Whether a release is short of what finishing it takes: an input under its threshold of
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

-- A signer's vote to cancel a release that cannot be finished, as btc_cancel_digest has it, posted
-- by anyone; once the threshold of the federation in force voted, its inputs are unspent again, its
-- withdrawals queued, and what was signed for it counts for nothing here. A release with every
-- input at its threshold is refused: anyone can still finish it.
--
-- Bitcoin does not know a release was cancelled, and what a signer signed is public from the moment
-- it is sent, even in a transaction that lands too late, so a release under its threshold here may
-- still be finished there. So each withdrawal it paid is anchored to one of its inputs: every release
-- paying that withdrawal from now on must spend that input as well, and conflicting, only one of
-- them can ever be mined. The anchor lapses once it is shown spent, as nothing that spends it can
-- be mined after that. It is the input of the newest federation, as a release stuck on an old
-- federation no one can reach anymore is why a cancel is needed at all.
function M.cancel(id, signature)
    id = whole(id, "the proposal")
    local fed = remember()

    if proposals:get(id .. ":state") ~= "open" then
        error("no open release by that number")
    end

    local tx = parse(proposals:get(id .. ":raw"))

    if not unfinished(id, tx) then
        error("every input of it holds its threshold of signatures: anyone can still finish it")
    end

    local key = crypto.recover(cancel_digest(id), signature)
    local known = false

    for _, signer in ipairs(fed.signers) do
        known = known or signer == key
    end

    if not known then
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

    for withdrawal in proposals:get(id .. ":ids"):gmatch("%d+") do
        -- One shown paid by a release cancelled before stays so.
        if withdrawals:get(withdrawal .. ":state") == "proposed" then
            withdrawals:set(withdrawal .. ":state", "queued")
            local held = withdrawals:get(withdrawal .. ":anchor")

            if held == nil or utxos:get(held .. ":state") == "spent" then
                withdrawals:set(withdrawal .. ":anchor", anchor)
            end
        end
    end

    proposals:set(id .. ":state", "cancelled")
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

-- A release: its unsigned transaction, its id, its withdrawals, the federation it was proposed
-- under, and its state, open, released or cancelled.
function M.proposal(id)
    id = whole(id, "the proposal")

    return proposals:get(id .. ":raw"), proposals:get(id .. ":txid"), proposals:get(id .. ":ids"), tonumber(proposals:get(id .. ":version")), proposals:get(id .. ":state")
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

        if signature and proposals:get(id .. ":state") ~= "cancelled" then
            table.insert(found, at)
            table.insert(found, signature)
        end
    end

    return witness_script(version, utxos:get(key .. ":recipient")), threshold, table.unpack(found)
end

-- Shows a release, bitcoin_depth deep: its inputs are spent, its withdrawals paid, and its change,
-- if any, held for the federation it was proposed under. One cancelled may still be shown, if it
-- was finished anyway: Bitcoin mined it, so what it spent and paid is what the gateway must know.
function M.released(id, block, raw, index, path)
    id = whole(id, "the proposal")
    local fed = remember()
    local state = proposals:get(id .. ":state")

    if state ~= "open" and state ~= "cancelled" then
        error("no open release by that number")
    end

    local txid = proven(block, raw, index, path, fed.bitcoin_depth)

    if txid ~= proposals:get(id .. ":txid") then
        error("not that release's transaction")
    end

    local tx = parse(proposals:get(id .. ":raw"))

    -- Cancelled and proposed again as it was, it has the same id: shown once is enough.
    for _, input in ipairs(tx.inputs) do
        if utxos:get(outpoint(input) .. ":state") == "spent" then
            error("an input of it was shown spent already")
        end
    end

    for _, input in ipairs(tx.inputs) do
        utxos:set(outpoint(input) .. ":state", "spent")
    end

    local ids = proposals:get(id .. ":ids")
    local paid = 0

    for withdrawal in ids:gmatch("%d+") do
        withdrawals:set(withdrawal .. ":state", "paid")
        paid = paid + 1
    end

    local change = tx.outputs[paid + 1]

    if change then
        hold(hex(txid) .. ":" .. paid, change.sats, proposals:get(id .. ":version"), "")
    end

    proposals:set(id .. ":state", "released")
    events.emit("Released", id, txid)
    return true
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

-- The most fee a release may pay, in satoshis, and the least it may pay a virtual byte.
function M.fee_limits()
    return tonumber(storage.get("fee_cap")), tonumber(storage.get("fee_floor"))
end

-- A withdrawal: its output script, satoshis and state: queued, proposed or paid.
function M.withdrawal(id)
    id = whole(id, "the withdrawal")

    local by = withdrawals:get(id .. ":by")

    return withdrawals:get(id .. ":script"), tonumber(withdrawals:get(id .. ":sats")), withdrawals:get(id .. ":state"), by and address.parse(by)
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

    local fed = remember()

    return multisig(fed.threshold, fed.signers)
end

return M
