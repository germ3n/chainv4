-- An example multi-token collection following the chainv4 multi-token standard
-- (docs/nft-standard.md): holders hold amounts of each id, as ERC-1155's do, so one collection
-- keeps one-of-a-kind items and interchangeable ones side by side. Whoever deploys it owns the
-- collection, or whoever a factory names when it clones it: they mint, set where token_uri points,
-- and may upgrade it.

local M = {}

-- The other side of a mint or a burn.
local ZERO = address.from_bytes(string.rep("\0", 32))
-- The most ids tokens_of returns at once.
local MOST = 100
local ONE = u256(1)
local NONE = u256(0)

-- Amounts live where the multi-token standard says light clients can prove them: the balances
-- namespace, by the id in decimal, a colon and the holder's address text, as decimal text.
local balances = storage.namespace("balances")
-- The ids each holder holds some of, to list them: count:<holder>, at:<holder>:<n> the nth id,
-- index:<holder>:<id> its n.
local held = storage.namespace("held")
local operators = storage.namespace("operators")
local uris = storage.namespace("uris")
local supplies = storage.namespace("supplies")

local function key(id)
    return tostring(u256(id))
end

local function same(a, b)
    return a:text() == b:text()
end

local function balance(holder, id)
    return u256(balances:get(key(id) .. ":" .. holder:text()) or "0")
end

local function count(holder)
    return u256(held:get("count:" .. holder:text()) or "0")
end

local function hold(holder, id)
    local n = count(holder)
    held:set("at:" .. holder:text() .. ":" .. tostring(n), key(id))
    held:set("index:" .. holder:text() .. ":" .. key(id), tostring(n))
    held:set("count:" .. holder:text(), tostring(n + ONE))
end

-- Takes an id out of a holder's list, moving the last one into its place.
local function release(holder, id)
    local last = tostring(count(holder) - ONE)
    local index = held:get("index:" .. holder:text() .. ":" .. key(id))
    local last_id = held:get("at:" .. holder:text() .. ":" .. last)

    if index ~= last then
        held:set("at:" .. holder:text() .. ":" .. index, last_id)
        held:set("index:" .. holder:text() .. ":" .. last_id, index)
    end

    held:set("at:" .. holder:text() .. ":" .. last, nil)
    held:set("index:" .. holder:text() .. ":" .. key(id), nil)
    held:set("count:" .. holder:text(), last ~= "0" and last or nil)
end

local function set_balance(holder, id, amount)
    local before = balance(holder, id)
    balances:set(key(id) .. ":" .. holder:text(), amount ~= NONE and tostring(amount) or nil)

    if before == NONE and amount ~= NONE then
        hold(holder, id)
    elseif before ~= NONE and amount == NONE then
        release(holder, id)
    end
end

local function supply(id)
    return u256(supplies:get(key(id)) or "0")
end

local function is_operator(holder, operator)
    return operators:get(holder:text() .. "|" .. operator:text()) ~= nil
end

-- Moves an amount of an id, a mint from ZERO or a burn to it.
local function move(from, to, id, amount)
    if same(from, ZERO) then
        supplies:set(key(id), tostring(supply(id) + amount))
    else
        local available = balance(from, id)

        if available < amount then
            error("insufficient balance")
        end

        set_balance(from, id, available - amount)
    end

    if same(to, ZERO) then
        supplies:set(key(id), tostring(supply(id) - amount))
    else
        set_balance(to, id, balance(to, id) + amount)
    end

    events.emit("TransferSingle", ctx.sender, from, to, u256(id), amount)
end

local function transfer_from(from, to, id, amount)
    if not (same(ctx.sender, from) or is_operator(from, ctx.sender)) then
        error("not the holder or an operator")
    end

    if same(to, ZERO) then
        error("transfer to the zero address; burn instead")
    end

    move(from, to, id, u256(amount))
end

-- A contract receiving tokens by a safe transfer must say it takes them: any contract address,
-- as the Rust twin judges it too, so one where nothing is deployed refuses.
local function received(from, to, id, amount)
    if to:kind() ~= "contract" then
        return
    end

    local called, answer = pcall(contracts.call, to, "on_nft_received", ctx.sender, from, u256(id), u256(amount))

    if not called or answer ~= true then
        error("the receiver did not take the tokens")
    end
end

local function only_owner()
    if ctx.sender:text() ~= storage.get("owner") then
        error("only the collection's owner may")
    end
end

-- Run once by the chain when the collection is deployed, or by a factory's clone, which names its
-- owner; every argument may be left out.
function M.init(name, symbol, base_uri, owner)
    storage.set("name", name or "Example Items")
    storage.set("symbol", symbol or "EXI")
    storage.set("base_uri", base_uri or "")
    storage.set("owner", (owner or ctx.sender):text())
end

function M.name()
    return storage.get("name")
end

function M.symbol()
    return storage.get("symbol")
end

-- How much of an id exists.
function M.total_supply(id)
    return supply(id)
end

function M.balance_of(holder, id)
    return balance(holder, id)
end

function M.token_uri(id)
    return uris:get(key(id)) or (storage.get("base_uri") .. key(id))
end

-- Up to count ids holder holds some of, from the start-th on, counting from 0: page through them
-- until fewer come back than asked for.
function M.tokens_of(holder, start, wanted)
    local from = u256(start or 0)
    local want = u256(wanted or MOST)

    if want > u256(MOST) then
        want = u256(MOST)
    end

    local upto = from + want
    local total = count(holder)
    local ids = {}

    if upto > total then
        upto = total
    end

    while from < upto do
        table.insert(ids, u256(held:get("at:" .. holder:text() .. ":" .. tostring(from))))
        from = from + ONE
    end

    return table.unpack(ids)
end

function M.transfer(to, id, amount)
    transfer_from(ctx.sender, to, id, amount)
    return true
end

function M.transfer_from(from, to, id, amount)
    transfer_from(from, to, id, amount)
    return true
end

function M.safe_transfer(to, id, amount)
    transfer_from(ctx.sender, to, id, amount)
    received(ctx.sender, to, id, amount)
    return true
end

function M.safe_transfer_from(from, to, id, amount)
    transfer_from(from, to, id, amount)
    received(from, to, id, amount)
    return true
end

-- Lets operator move every id the caller holds, now and later, or stops it.
function M.set_approval_for_all(operator, yes)
    if same(operator, ctx.sender) then
        error("a holder is their own operator already")
    end

    operators:set(ctx.sender:text() .. "|" .. operator:text(), yes and "1" or nil)
    events.emit("ApprovalForAll", ctx.sender, operator, yes == true)
    return true
end

function M.is_approved_for_all(holder, operator)
    return is_operator(holder, operator)
end

-- Beyond the standard: the owner mints an amount of an id, giving the id its own URI the first
-- time if they like.
function M.mint(to, id, amount, uri)
    only_owner()

    if same(to, ZERO) then
        error("mint to the zero address")
    end

    if uri ~= nil and uris:get(key(id)) == nil then
        uris:set(key(id), uri)
    end

    move(ZERO, to, id, u256(amount))
    return true
end

-- A holder, or their operator, burns an amount of an id.
function M.burn(from, id, amount)
    if not (same(ctx.sender, from) or is_operator(from, ctx.sender)) then
        error("not the holder or an operator")
    end

    move(from, ZERO, id, u256(amount))
    return true
end

function M.set_base_uri(base_uri)
    only_owner()
    storage.set("base_uri", base_uri)
    return true
end

-- Beyond the standard, as ERC-1155's batches: several ids moved at once, each id and its amount
-- packed in one bytes argument, 64 bytes a pair, the id then the amount, 32 bytes each,
-- big-endian, as calls take no lists. Each pair moves as transfer_from moves one, and emits its
-- own TransferSingle; the safe kind asks a receiving contract for each, after all have moved.
local MOST_BATCH = 100

local function u256_of(bytes)
    local hex = {}

    for at = 1, #bytes do
        hex[at] = string.format("%02x", string.byte(bytes, at))
    end

    return u256("0x" .. table.concat(hex))
end

local function batch(packed)
    if type(packed) ~= "string" or #packed == 0 or #packed % 64 ~= 0 then
        error("a batch is 64 bytes a pair: an id then its amount, 32 bytes each, big-endian")
    end

    if #packed > 64 * MOST_BATCH then
        error("a batch moves at most " .. MOST_BATCH .. " ids")
    end

    local found = {}

    for at = 1, #packed, 64 do
        found[#found + 1] = { u256_of(packed:sub(at, at + 31)), u256_of(packed:sub(at + 32, at + 63)) }
    end

    return found
end

function M.batch_transfer_from(from, to, packed)
    for _, pair in ipairs(batch(packed)) do
        transfer_from(from, to, pair[1], pair[2])
    end

    return true
end

function M.safe_batch_transfer_from(from, to, packed)
    local pairs = batch(packed)

    for _, pair in ipairs(pairs) do
        transfer_from(from, to, pair[1], pair[2])
    end

    for _, pair in ipairs(pairs) do
        received(from, to, pair[1], pair[2])
    end

    return true
end

-- Beyond the standard, as ERC-2981 does it: what a sale of an id owes, and to whom, out of its
-- price. The owner sets it for every id, or for one, in basis points out of 10,000; unset, nothing.
local royalties = storage.namespace("royalties")
local WHOLE = u256(10000)

function M.set_royalty(receiver, bps, id)
    only_owner()
    bps = u256(bps)

    if bps > WHOLE then
        error("a royalty is at most 10,000 basis points, the whole price")
    end

    royalties:set(id == nil and "*" or key(id), receiver:text() .. "|" .. tostring(bps))
    events.emit("RoyaltySet", receiver, bps, id == nil and NONE or u256(id))
    return true
end

-- Who a sale of id at price pays, and how much: the zero address and nothing when none is set.
function M.royalty_info(id, price)
    local kept = royalties:get(key(id)) or royalties:get("*")

    if kept == nil then
        return ZERO, NONE
    end

    local receiver, bps = kept:match("^(.*)|(%d+)$")
    return address.parse(receiver), u256(price) * u256(bps) // WHOLE
end

-- Asked by the chain before an upgrade: only the owner may.
function M.check_perms(permission, who)
    return permission == "upgrade" and who:text() == storage.get("owner")
end

return M
