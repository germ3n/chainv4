-- Atomic swaps with Bitcoin, chainv4's side: coins, a token or NFTs locked under the SHA-256 of a
-- secret, for a recipient who claims them with the secret before a deadline, or back to whoever
-- locked them after it. Bitcoin's side is an HTLC under the same hash (src/swap/htlc.rs): whoever
-- claims on one chain shows the secret there, and the other side claims with it on the other.
--
-- One contract holds every swap, each by its id: the keccak256 of all its terms in Solidity's ABI,
-- the hash, locker, recipient, token, kind, NFT id, amount and deadline, as contracts/evm/Swap.sol
-- and src/swap compute it. Someone who saw the hash and locks under it first makes a swap of
-- their own, and takes nothing from the real one. Ids, hashes and secrets are written as 0x and
-- 64 hex digits. A secret is 32 bytes, as Bitcoin's script wants it, so one chain never takes a
-- secret the other refuses.
--
-- Anyone may send a claim, a withdraw or a refund: what is locked only ever goes to the recipient
-- or the locker. A claim records the swap as claimed, and keeps its secret, even when the payout
-- is refused, as a token its issuer can pause would refuse it: the secret is out once the claim is
-- sent, so the swap must be the recipient's from then on. The payout is then owed, and withdraw
-- tries it again. A claimed swap is never refunded.
--
-- Deadlines are block times, in milliseconds since 1970: leave hours of margin, as a block's time
-- may be a little off the clock.
--
-- Events: Locked(id, hash, locker, recipient, token, amount, deadline), the token being the zero
-- address for the chain's coin, or an NFT collection; LockedItem(id, kind, nft id) after it for
-- NFTs, kind nft or multi; Claimed(id, secret), which the other side reads the secret from;
-- Owed(id, why) when the claim's payout was refused, and Withdrawn(id) once withdraw paid it;
-- Refunded(id).
--
-- NFTs follow docs/nft-standard.md: one token of an NFT collection, or an amount of one id of a
-- multi-token collection, moved in with transfer_from, so approve this contract for the token, or
-- make it an operator, first.

--@namespace swaps bytes -> bytes

local M = {}

-- What locked a swap's coins: the zero address, for the chain's coin.
local COIN = address.from_bytes(string.rep("\0", 32))

-- How a swap's id numbers what it holds, as Swap.sol's Kind does.
local KINDS = { coin = 0, token = 0, nft = 1, multi = 2 }

-- Each swap's fields, under its id and the field's name.
local swaps = storage.namespace("swaps")

-- Reads 0x and 64 hex digits as the 32 bytes they write.
local function bytes32(text, what)
    if type(text) ~= "string" or #text ~= 66 or text:sub(1, 2) ~= "0x" or text:sub(3):find("[^0-9a-fA-F]") then
        error(what .. " is 32 bytes, written 0x and 64 hex digits")
    end

    return (text:sub(3):gsub("..", function(pair)
        return string.char(tonumber(pair, 16))
    end))
end

-- A deadline as a number: a plain one, or one of the integer types, such as an i64 a client sends.
local function millis(value)
    if type(value) == "number" then
        return value
    end

    local ok, number = pcall(function()
        return value:tonumber()
    end)

    if not ok or type(number) ~= "number" then
        error("the deadline is a time, in milliseconds since 1970")
    end

    return number
end

-- A number as one word of Solidity's ABI: 32 bytes, big-endian.
local function word(number)
    local digits = u256(number):hex():sub(3)

    return bytes32("0x" .. string.rep("0", 64 - #digits) .. digits, "a number")
end

-- An address in a swap's id: Solidity's static tuple (uint8 kind, bytes32 bytes), which is 31 zero
-- bytes and then the 33 Address:encode writes, so two kinds of address with the same bytes never
-- name one swap.
local function party(who)
    return string.rep("\0", 31) .. who:encode()
end

-- A swap's id from its terms, in lowercase.
local function id_of(digest, locker, recipient, token, kind, id, amount, deadline)
    local terms = bytes32(digest, "the hash")
        .. party(locker)
        .. party(recipient)
        .. party(token)
        .. word(kind)
        .. word(id)
        .. word(amount)
        .. word(deadline)

    return hash.keccak256(terms):hex()
end

-- A swap's id as a call gives it, in lowercase, the form it is kept under.
local function key_of(id)
    bytes32(id, "the swap's id")

    return string.lower(id)
end

local function field(key, name)
    return swaps:get(key .. ":" .. name)
end

local function set(key, name, value)
    swaps:set(key .. ":" .. name, value)
end

-- Refuses coin sent with a call that takes none, which nothing would ever pay back.
local function nothing_sent(why)
    if ctx.value ~= u256(0) then
        error(why)
    end
end

-- A new swap's id and its deadline, once its terms are checked: none may stand under that id yet.
local function open(digest, recipient, deadline, token, kind, id, amount)
    bytes32(digest, "the hash")
    deadline = millis(deadline)

    if amount == u256(0) then
        error("lock something")
    end

    -- A swap for the zero address could only ever be refunded.
    local bytes = recipient:bytes()
    if bytes == string.rep("\0", #bytes) then
        error("a swap pays someone: not the zero address")
    end

    if deadline <= ctx.time then
        error("the deadline must be after this block's time, in milliseconds since 1970")
    end

    -- A deadline get could not give back whole, a fraction or past what u256 holds, is refused.
    local whole, exact = pcall(u256, deadline)
    if not whole then
        error("the deadline is a time, in milliseconds since 1970")
    end

    local key = id_of(digest, ctx.sender, recipient, token, kind, id, amount, exact)

    if field(key, "state") ~= nil then
        error("a swap of these terms already exists")
    end

    return key, exact
end

-- Records a swap whose coins, token or NFTs this contract now holds.
local function record(key, digest, recipient, deadline, token, amount)
    set(key, "hash", string.lower(digest))
    set(key, "locker", ctx.sender:text())
    set(key, "recipient", recipient:text())
    set(key, "token", token:text())
    set(key, "amount", tostring(amount))
    set(key, "deadline", tostring(deadline))
    set(key, "height", tostring(ctx.height))
    set(key, "state", "locked")
    events.emit("Locked", key, string.lower(digest), ctx.sender, recipient, token, amount, deadline)
end

-- Records which NFT a swap holds, once record has the rest.
local function item(key, kind, id)
    set(key, "kind", kind)
    set(key, "id", tostring(id))
    events.emit("LockedItem", key, kind, id)
end

-- Pays a swap's coins, token or NFTs to whoever it goes to.
local function pay(key, to)
    local token = address.parse(field(key, "token"))
    local amount = u256(field(key, "amount"))
    local kind = field(key, "kind")

    if kind == "nft" then
        contracts.call(token, "transfer", to, u256(field(key, "id")))
    elseif kind == "multi" then
        contracts.call(token, "transfer", to, u256(field(key, "id")), amount)
    elseif token == COIN then
        accounts.transfer(to, amount)
    else
        contracts.call(token, "transfer", to, amount)
    end
end

-- What this contract holds of a token, or of one id of a multi-token collection.
local function held(token, id)
    if id == nil then
        return u256(contracts.call(token, "balance_of", ctx.contract))
    end

    return u256(contracts.call(token, "balance_of", ctx.contract, id))
end

-- Locks the chain's coin the call sends, for recipient to claim with the secret behind the hash
-- before deadline. Returns the swap's id.
function M.lock(digest, recipient, deadline)
    local key, exact = open(digest, recipient, deadline, COIN, KINDS.coin, u256(0), ctx.value)
    record(key, digest, recipient, exact, COIN, ctx.value)

    return key
end

-- Locks amount of a token, which the caller first approved this contract to move, as lock does
-- the coin. The call sends no coin. A token that delivers less than the amount, as one taking a
-- fee as it moves does, is refused: the swap would say it holds more than it does, and be paid
-- from another's.
function M.lock_token(digest, recipient, deadline, token, amount)
    nothing_sent("lock_token takes a token, not coins: send none")
    amount = u256(amount)
    local key, exact = open(digest, recipient, deadline, token, KINDS.token, u256(0), amount)
    local before = held(token)
    contracts.call(token, "transfer_from", ctx.sender, ctx.contract, amount)

    if held(token) ~= before + amount then
        error("the token moved less than the amount")
    end

    record(key, digest, recipient, exact, token, amount)

    return key
end

-- Locks one NFT of a collection, which the caller first approved this contract to move, as
-- lock_token does a token, once its owner is this contract.
function M.lock_nft(digest, recipient, deadline, collection, id)
    nothing_sent("lock_nft takes an NFT, not coins: send none")
    id = u256(id)
    local key, exact = open(digest, recipient, deadline, collection, KINDS.nft, id, u256(1))
    contracts.call(collection, "transfer_from", ctx.sender, ctx.contract, id)

    if contracts.call(collection, "owner_of", id) ~= ctx.contract then
        error("the NFT did not move")
    end

    record(key, digest, recipient, exact, collection, u256(1))
    item(key, "nft", id)

    return key
end

-- Locks an amount of one id of a multi-token collection, which the caller first made this
-- contract an operator of, once it holds that much more of the id.
function M.lock_multi(digest, recipient, deadline, collection, id, amount)
    nothing_sent("lock_multi takes tokens, not coins: send none")
    id, amount = u256(id), u256(amount)
    local key, exact = open(digest, recipient, deadline, collection, KINDS.multi, id, amount)
    local before = held(collection, id)
    contracts.call(collection, "transfer_from", ctx.sender, ctx.contract, id, amount)

    if held(collection, id) ~= before + amount then
        error("the tokens moved less than the amount")
    end

    record(key, digest, recipient, exact, collection, amount)
    item(key, "multi", id)

    return key
end

-- Claims the swap with an id with its secret, 0x and 64 hex digits, before its deadline, and
-- shows the secret in Claimed. The swap is the recipient's from then on: its payout is tried at
-- once, and when refused is owed, for withdraw to pay.
function M.claim(id, secret)
    nothing_sent("claim takes no coin: send none")
    local key = key_of(id)
    local revealed = bytes32(secret, "the secret")

    if field(key, "state") ~= "locked" then
        error("no swap waits under that id")
    end

    if hash.sha256(revealed):hex() ~= field(key, "hash") then
        error("no swap waits on that secret: its hash is not the swap's")
    end

    if ctx.time >= tonumber(field(key, "deadline")) then
        error("too late: the swap's deadline passed, and it goes back to its locker")
    end

    set(key, "state", "claimed")
    set(key, "secret", string.lower(secret))
    events.emit("Claimed", key, string.lower(secret))

    -- The secret is in this call whether it runs or not, so the claim stands even when the payout
    -- is refused, and the payout waits for withdraw.
    local paid, why = pcall(pay, key, address.parse(field(key, "recipient")))

    if not paid then
        set(key, "owed", "1")
        events.emit("Owed", key, tostring(why))
    end
end

-- Pays a claimed swap's recipient what its claim could not. Anyone may send it, as often as it
-- takes.
function M.withdraw(id)
    nothing_sent("withdraw takes no coin: send none")
    local key = key_of(id)

    if field(key, "state") ~= "claimed" or field(key, "owed") == nil then
        error("nothing is owed under that id")
    end

    set(key, "owed", nil)
    pay(key, address.parse(field(key, "recipient")))
    events.emit("Withdrawn", key)
end

-- Pays a swap back to its locker, once its deadline passed and nobody claimed it.
function M.refund(id)
    nothing_sent("refund takes no coin: send none")
    local key = key_of(id)

    if field(key, "state") ~= "locked" then
        error("no swap waits under that id")
    end

    if ctx.time < tonumber(field(key, "deadline")) then
        error("too early: the swap can be claimed until its deadline")
    end

    set(key, "state", "refunded")
    pay(key, address.parse(field(key, "locker")))
    events.emit("Refunded", key)
end

-- A swap: its state, locked, claimed or refunded, or none; who locked it, for whom, the token
-- (the zero address for the coin), how much, its deadline, its secret once claimed, or "", the
-- block it was locked in, for the other side to wait until that block is final, and whether its
-- payout is owed.
function M.get(id)
    local key = key_of(id)
    local state = field(key, "state")

    if state == nil then
        return "none"
    end

    return state,
        address.parse(field(key, "locker")),
        address.parse(field(key, "recipient")),
        address.parse(field(key, "token")),
        u256(field(key, "amount")),
        u256(field(key, "deadline")),
        field(key, "secret") or "",
        u256(field(key, "height")),
        field(key, "owed") ~= nil
end

-- What kind of thing a swap holds, coin, token, nft or multi, and the NFT's id, or 0; none for
-- no swap. get's token is the collection, and its amount how many.
function M.item(id)
    local key = key_of(id)

    if field(key, "state") == nil then
        return "none", u256(0)
    end

    local kind = field(key, "kind")

    if kind == nil then
        kind = field(key, "token") == COIN:text() and "coin" or "token"
    end

    return kind, u256(field(key, "id") or "0")
end

-- The id a swap of these terms has, as lock gives it back: kind is coin, token, nft or multi, and
-- the token is the token or collection, or for the coin anything at all, as a transaction cannot
-- name the zero address. For tools that hold the terms and not the id.
function M.id(digest, locker, recipient, token, kind, nft_id, amount, deadline)
    if KINDS[kind] == nil then
        error("a kind is coin, token, nft or multi")
    end

    if kind == "coin" then
        token = COIN
    end

    return id_of(digest, locker, recipient, token, KINDS[kind], u256(nft_id), u256(amount), u256(millis(deadline)))
end

return M
