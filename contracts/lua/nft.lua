-- An example NFT collection following the chainv4 NFT standard (docs/nft-standard.md): each token
-- has one owner. Whoever deploys it owns the collection, or whoever a factory names when it clones
-- it (contracts/lua/nft_factory.lua): they mint, set where token_uri points, and may upgrade it.

local M = {}

-- The other side of a mint or a burn.
local ZERO = address.from_bytes(string.rep("\0", 32))
local NONE = u256(0)
-- The most ids tokens_of returns at once.
local MOST = 100
local ONE = u256(1)

-- Owners live where the NFT standard says light clients can prove them: the owners namespace, by
-- the token id in decimal, as the owner's address text.
local owners = storage.namespace("owners")
-- What each holder holds, to list it: count:<holder>, at:<holder>:<n> the nth id, index:<id> its n.
local held = storage.namespace("held")
-- Who may move a token besides its owner, by id; and the owners' operators, by owner|operator.
local approvals = storage.namespace("approvals")
local operators = storage.namespace("operators")
-- A token's own URI, when it was minted with one.
local uris = storage.namespace("uris")

local function key(id)
    return tostring(u256(id))
end

local function owner_of(id)
    local text = owners:get(key(id))

    if text == nil then
        error("no such token")
    end

    return address.parse(text)
end

local function same(a, b)
    return a:text() == b:text()
end

local function count(holder)
    return u256(held:get("count:" .. holder:text()) or "0")
end

local function hold(holder, id)
    local n = count(holder)
    held:set("at:" .. holder:text() .. ":" .. tostring(n), key(id))
    held:set("index:" .. key(id), tostring(n))
    held:set("count:" .. holder:text(), tostring(n + ONE))
end

-- Takes an id out of a holder's list, moving the last one into its place.
local function release(holder, id)
    local last = tostring(count(holder) - ONE)
    local index = held:get("index:" .. key(id))
    local last_id = held:get("at:" .. holder:text() .. ":" .. last)

    if index ~= last then
        held:set("at:" .. holder:text() .. ":" .. index, last_id)
        held:set("index:" .. last_id, index)
    end

    held:set("at:" .. holder:text() .. ":" .. last, nil)
    held:set("index:" .. key(id), nil)
    held:set("count:" .. holder:text(), last ~= "0" and last or nil)
end

local function is_operator(holder, operator)
    return operators:get(holder:text() .. "|" .. operator:text()) ~= nil
end

local function approved(id)
    local text = approvals:get(key(id))

    return text and address.parse(text)
end

local function may_move(spender, holder, id)
    local allowed = approved(id)

    return same(spender, holder) or is_operator(holder, spender) or (allowed ~= nil and same(allowed, spender))
end

local function supply()
    return u256(storage.get("supply") or "0")
end

-- Moves a token, a mint from ZERO or a burn to it, forgetting who was approved for it.
local function move(from, to, id)
    approvals:set(key(id), nil)

    if same(from, ZERO) then
        storage.set("supply", tostring(supply() + ONE))
    else
        release(from, id)
    end

    if same(to, ZERO) then
        owners:set(key(id), nil)
        storage.set("supply", tostring(supply() - ONE))
    else
        owners:set(key(id), to:text())
        hold(to, id)
    end

    events.emit("Transfer", from, to, u256(id))
end

local function transfer_from(from, to, id)
    local holder = owner_of(id)

    if not same(holder, from) then
        error("from does not own the token")
    end

    if not may_move(ctx.sender, holder, id) then
        error("not the owner, approved or an operator")
    end

    if same(to, ZERO) then
        error("transfer to the zero address; burn instead")
    end

    move(from, to, id)
end

-- A contract receiving a token by a safe transfer must say it takes it: any contract address,
-- as the Rust twin judges it too, so one where nothing is deployed refuses.
local function received(from, to, id)
    if to:kind() ~= "contract" then
        return
    end

    local called, answer = pcall(contracts.call, to, "on_nft_received", ctx.sender, from, u256(id), ONE)

    if not called or answer ~= true then
        error("the receiver did not take the token")
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
    storage.set("name", name or "Example Collection")
    storage.set("symbol", symbol or "EXC")
    storage.set("base_uri", base_uri or "")
    storage.set("owner", (owner or ctx.sender):text())
end

function M.name()
    return storage.get("name")
end

function M.symbol()
    return storage.get("symbol")
end

function M.total_supply()
    return supply()
end

function M.balance_of(holder)
    return count(holder)
end

function M.owner_of(id)
    return owner_of(id)
end

function M.token_uri(id)
    owner_of(id)

    return uris:get(key(id)) or (storage.get("base_uri") .. key(id))
end

-- Up to count ids holder holds, from the start-th on, counting from 0: page through them until
-- fewer come back than asked for.
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

function M.transfer(to, id)
    transfer_from(ctx.sender, to, id)
    return true
end

function M.transfer_from(from, to, id)
    transfer_from(from, to, id)
    return true
end

function M.safe_transfer(to, id)
    transfer_from(ctx.sender, to, id)
    received(ctx.sender, to, id)
    return true
end

function M.safe_transfer_from(from, to, id)
    transfer_from(from, to, id)
    received(from, to, id)
    return true
end

-- Lets approved move one token, until it moves; the zero address approves nobody.
function M.approve(spender, id)
    local holder = owner_of(id)

    if not (same(ctx.sender, holder) or is_operator(holder, ctx.sender)) then
        error("not the owner or an operator")
    end

    approvals:set(key(id), not same(spender, ZERO) and spender:text() or nil)
    events.emit("Approval", holder, spender, u256(id))
    return true
end

function M.get_approved(id)
    owner_of(id)

    return approved(id)
end

-- Lets operator move every token the caller holds, now and later, or stops it.
function M.set_approval_for_all(operator, yes)
    if same(operator, ctx.sender) then
        error("an owner is their own operator already")
    end

    operators:set(ctx.sender:text() .. "|" .. operator:text(), yes and "1" or nil)
    events.emit("ApprovalForAll", ctx.sender, operator, yes == true)
    return true
end

function M.is_approved_for_all(holder, operator)
    return is_operator(holder, operator)
end

-- Beyond the standard: the owner mints the next id to whoever they like, or an id of their
-- choosing, as a bridge's wrapped collection mints the ids the other chain's tokens have, with
-- its own URI or the base one, and returns the id.
function M.mint(to, uri, chosen)
    only_owner()

    if same(to, ZERO) then
        error("mint to the zero address")
    end

    local id = u256(chosen or storage.get("next") or "1")

    if owners:get(key(id)) ~= nil then
        error("that token exists already")
    end

    if id >= u256(storage.get("next") or "1") then
        storage.set("next", tostring(id + ONE))
    end

    if uri ~= nil then
        uris:set(key(id), uri)
    end

    move(ZERO, to, id)
    return id
end

-- Its owner, or whoever may move it, burns a token.
function M.burn(id)
    local holder = owner_of(id)

    if not may_move(ctx.sender, holder, id) then
        error("not the owner, approved or an operator")
    end

    uris:set(key(id), nil)
    move(holder, ZERO, id)
    return true
end

-- The owner points token_uri somewhere else: base_uri followed by the id, for tokens minted
-- without their own.
function M.set_base_uri(base_uri)
    only_owner()
    storage.set("base_uri", base_uri)
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
