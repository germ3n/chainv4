-- The bridges' Bitcoin gateway (docs/bridges.md#bitcoin): BTC deposited to the federation is
-- minted here as wBTC, on a proof no one has to vouch for, and wBTC burned here is paid out in
-- BTC by the federation, through releases proposed and signed on this contract.
--
-- Deposits: each address here has its own deposit address on Bitcoin, a P2WSH of
--   <its 33 bytes> OP_DROP <threshold> <signer keys…> <count> OP_CHECKMULTISIG
-- so any Bitcoin wallet pays it, and only the federation's threshold spends it. Once the payment
-- is bitcoin_depth deep in the Bitcoin light client contract (contracts/examples/btc_relay),
-- anyone shows it, and its value is minted to the address it was for.
--
-- Releases: a withdrawal burns wBTC for a Bitcoin script and amount. Anyone proposes a release:
-- an unsigned Bitcoin transaction this contract checks pays exactly queued withdrawals from the
-- deposits it holds, change back to the federation, its fee under the cap. The signers sign each
-- input as BIP143 does, and post their signatures here, each checked; anyone assembles the
-- witnesses and broadcasts it; and once it is deep enough, anyone shows it, and its inputs are
-- spent and its change held. A sweep is a release that pays everything an old federation holds
-- to the one in force.
--
-- Events: DepositedBtc(txid, vout, sats, recipient), WithdrawRequested(id, script, sats),
-- Proposed(proposal, txid), SignedBtc(proposal, input, signer), Released(proposal, txid).

local M = {}

local SIGHASH_ALL = 1
local MOST_WITHDRAWALS = 50
-- What a withdrawal pays at least, in satoshis: above Bitcoin's dust for any output.
local LEAST = 1000

local feds = storage.namespace("feds")
local utxos = storage.namespace("utxos")
local withdrawals = storage.namespace("withdrawals")
local proposals = storage.namespace("proposals")
local signatures = storage.namespace("signatures")

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

        if first == 0xfd then
            return r.number(2)
        elseif first == 0xfe then
            return r.number(4)
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
    return push(recipient) .. "\117" .. multisig(threshold, keys)
end

local function p2wsh(script)
    return "\0\32" .. hash.sha256(script):bytes()
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

-- The relay, the wBTC token's template (contracts/lua/wrapped.lua), the most fee a release may
-- pay, in satoshis, and how long an old federation's deposit addresses stay open, in
-- milliseconds; set once.
function M.setup(relay_address, template, fee_cap, grace_ms)
    owner_only()

    if storage.get("relay") then
        error("the gateway is set up already")
    end

    storage.set("relay", relay_address:text())
    storage.set("fee_cap", tostring(whole(fee_cap, "the fee cap")))
    storage.set("grace", tostring(whole(grace_ms, "the grace period")))
    local token = contracts.clone(template, "wbtc", "Wrapped Bitcoin", "wBTC", 8, ctx.contract)
    storage.set("wbtc", token:text())
    remember()
    return token
end

function M.wbtc()
    return wbtc()
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

    if #script < 1 or #script > 80 then
        error("an output script is 1 to 80 bytes")
    end

    contracts.call(wbtc(), "burn", ctx.sender, u256(sats))
    local id = count("withdrawal_count") + 1
    storage.set("withdrawal_count", tostring(id))
    withdrawals:set(id .. ":script", script)
    withdrawals:set(id .. ":sats", tostring(sats))
    withdrawals:set(id .. ":state", "queued")
    events.emit("WithdrawRequested", id, script, u256(sats))
    return id
end

-- Proposes a release: raw, an unsigned transaction without witnesses, spending deposits and
-- change this gateway holds, whose first outputs pay the given withdrawals in order, and whose
-- one more output, if any, is change to the federation in force. With no withdrawals it is a
-- sweep: everything it spends an old federation's, all of it to the one in force.
function M.propose(raw, ...)
    local fed = remember()
    local ids = {}

    for _, id in ipairs({ ... }) do
        table.insert(ids, whole(id, "a withdrawal"))
    end

    local tx = parse(raw)

    if #ids > MOST_WITHDRAWALS then
        error("at most " .. MOST_WITHDRAWALS .. " withdrawals a release")
    end

    if tx.locktime ~= "\0\0\0\0" then
        error("a release has no lock time")
    end

    local spent = 0

    for _, input in ipairs(tx.inputs) do
        local key = hex(input.outpoint:sub(1, 32)) .. ":" .. reader(input.outpoint:sub(33, 36)).number(4)

        if utxos:get(key .. ":state") ~= "unspent" then
            error("an input that is not an output the gateway holds unspent")
        end

        if #input.script ~= 0 then
            error("an input with a script: a release spends witness outputs alone")
        end

        if #ids == 0 and tonumber(utxos:get(key .. ":version")) >= fed.version then
            error("a sweep spends only what older federations hold")
        end

        spent = spent + tonumber(utxos:get(key .. ":sats"))
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

        paid = paid + change.sats
    end

    local fee = spent - paid

    if fee <= 0 or fee > tonumber(storage.get("fee_cap")) then
        error("a fee of nothing, or past the cap")
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
        utxos:set(hex(input.outpoint:sub(1, 32)) .. ":" .. reader(input.outpoint:sub(33, 36)).number(4) .. ":state", "pending")
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
    local key = hex(input.outpoint:sub(1, 32)) .. ":" .. reader(input.outpoint:sub(33, 36)).number(4)
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

-- A release: its unsigned transaction, its id, its withdrawals, the federation it was proposed
-- under, and its state, open or released.
function M.proposal(id)
    id = whole(id, "the proposal")

    return proposals:get(id .. ":raw"), proposals:get(id .. ":txid"), proposals:get(id .. ":ids"), tonumber(proposals:get(id .. ":version")), proposals:get(id .. ":state")
end

-- The signatures posted for one input, as signer number and signature, in the keys' order, and
-- the witness script and threshold: what assembling its witness takes.
function M.input_signatures(id, index)
    id, index = whole(id, "the proposal"), whole(index, "the input")
    local tx = parse(proposals:get(id .. ":raw") or error("no release by that number"))
    local input = tx.inputs[index + 1] or error("no such input")
    local key = hex(input.outpoint:sub(1, 32)) .. ":" .. reader(input.outpoint:sub(33, 36)).number(4)
    local version = utxos:get(key .. ":version")
    local threshold, keys = fed_at(version)
    local found = {}

    for at = 0, #keys - 1 do
        local signature = signatures:get(id .. ":" .. index .. ":" .. at)

        if signature then
            table.insert(found, at)
            table.insert(found, signature)
        end
    end

    return witness_script(version, utxos:get(key .. ":recipient")), threshold, table.unpack(found)
end

-- Shows a release, bitcoin_depth deep: its inputs are spent, its withdrawals paid, and its change,
-- if any, held for the federation it was proposed under.
function M.released(id, block, raw, index, path)
    id = whole(id, "the proposal")
    local fed = remember()

    if proposals:get(id .. ":state") ~= "open" then
        error("no open release by that number")
    end

    local txid = proven(block, raw, index, path, fed.bitcoin_depth)

    if txid ~= proposals:get(id .. ":txid") then
        error("not that release's transaction")
    end

    local tx = parse(proposals:get(id .. ":raw"))

    for _, input in ipairs(tx.inputs) do
        utxos:set(hex(input.outpoint:sub(1, 32)) .. ":" .. reader(input.outpoint:sub(33, 36)).number(4) .. ":state", "spent")
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

-- A withdrawal: its output script, satoshis and state: queued, proposed or paid.
function M.withdrawal(id)
    id = whole(id, "the withdrawal")

    return withdrawals:get(id .. ":script"), tonumber(withdrawals:get(id .. ":sats")), withdrawals:get(id .. ":state")
end

function M.withdrawal_count()
    return count("withdrawal_count")
end

function M.proposal_count()
    return count("proposal_count")
end

-- The change script of the federation in force: the bare multisig its change goes to.
function M.change_script()
    local fed = remember()

    return multisig(fed.threshold, fed.signers)
end

return M
