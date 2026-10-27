-- Atomic swaps with Bitcoin, chainv4's side: coins or a token locked under the SHA-256 of a
-- secret, for a recipient who claims them with the secret before a deadline, or back to whoever
-- locked them after it. Bitcoin's side is an HTLC under the same hash (src/swap/htlc.rs): whoever
-- claims on one chain shows the secret there, and the other side claims with it on the other.
--
-- One contract holds every swap, each by its hash, written as 0x and 64 hex digits. A secret is
-- 32 bytes, as Bitcoin's script wants it, so one chain never takes a secret the other refuses.
-- Anyone may send a claim or a refund: the coins only ever go to the recipient or the locker.
-- Deadlines are block times, in milliseconds since 1970: leave hours of margin, as a block's time
-- may be a little off the clock.
--
-- Events: Locked(hash, locker, recipient, token, amount, deadline), the token being the zero
-- address for the chain's coin; Claimed(hash, secret), which the other side reads the secret
-- from; Refunded(hash).

--@namespace swaps bytes -> bytes

local M = {}

-- What locked a swap's coins: the zero address, for the chain's coin.
local COIN = address.from_bytes(string.rep("\0", 32))

-- Each swap's fields, under its hash and the field's name.
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

local function field(hash, name)
    return swaps:get(hash .. ":" .. name)
end

local function set(hash, name, value)
    swaps:set(hash .. ":" .. name, value)
end

-- A new swap's hash, in lowercase, once its deadline is checked: none may stand under it yet.
local function fresh(hash, deadline)
    bytes32(hash, "the hash")
    hash = string.lower(hash)
    deadline = millis(deadline)

    if field(hash, "state") ~= nil then
        error("a swap under this hash already exists")
    end

    if deadline <= ctx.time then
        error("the deadline must be after this block's time, in milliseconds since 1970")
    end

    return hash, deadline
end

-- Records a swap whose coins or token this contract now holds.
local function record(hash, recipient, deadline, token, amount)
    if amount == u256(0) then
        error("lock something")
    end

    set(hash, "locker", ctx.sender:text())
    set(hash, "recipient", recipient:text())
    set(hash, "token", token:text())
    set(hash, "amount", tostring(amount))
    set(hash, "deadline", tostring(deadline))
    set(hash, "state", "locked")
    events.emit("Locked", hash, ctx.sender, recipient, token, amount, u256(deadline))
end

-- Pays a swap's coins or token to whoever it goes to.
local function pay(hash, to)
    local token = address.parse(field(hash, "token"))
    local amount = u256(field(hash, "amount"))

    if token == COIN then
        accounts.transfer(to, amount)
    else
        contracts.call(token, "transfer", to, amount)
    end
end

-- Locks the chain's coin the call sends, for recipient to claim with the secret behind hash
-- before deadline.
function M.lock(hash, recipient, deadline)
    hash, deadline = fresh(hash, deadline)
    record(hash, recipient, deadline, COIN, ctx.value)
end

-- Locks amount of a token, which the caller first approved this contract to move, as lock does
-- the coin. The call sends no coin.
function M.lock_token(hash, recipient, deadline, token, amount)
    if ctx.value ~= u256(0) then
        error("lock_token takes a token, not coins: send none")
    end

    hash, deadline = fresh(hash, deadline)
    amount = u256(amount)
    contracts.call(token, "transfer_from", ctx.sender, ctx.contract, amount)
    record(hash, recipient, deadline, token, amount)
end

-- Pays a swap to its recipient with its secret, 0x and 64 hex digits, before its deadline, and
-- shows the secret in Claimed.
function M.claim(secret)
    local key = hash.sha256(bytes32(secret, "the secret")):hex()

    if field(key, "state") ~= "locked" then
        error("no swap waits under that secret's hash")
    end

    if ctx.time >= tonumber(field(key, "deadline")) then
        error("too late: the swap's deadline passed, and it goes back to its locker")
    end

    set(key, "state", "claimed")
    set(key, "secret", string.lower(secret))
    pay(key, address.parse(field(key, "recipient")))
    events.emit("Claimed", key, string.lower(secret))
end

-- Pays a swap back to its locker, once its deadline passed and nobody claimed it.
function M.refund(hash)
    bytes32(hash, "the hash")
    hash = string.lower(hash)

    if field(hash, "state") ~= "locked" then
        error("no swap waits under that hash")
    end

    if ctx.time < tonumber(field(hash, "deadline")) then
        error("too early: the swap can be claimed until its deadline")
    end

    set(hash, "state", "refunded")
    pay(hash, address.parse(field(hash, "locker")))
    events.emit("Refunded", hash)
end

-- A swap: its state, locked, claimed or refunded, or none; who locked it, for whom, the token
-- (the zero address for the coin), how much, its deadline, and its secret once claimed, or "".
function M.get(hash)
    bytes32(hash, "the hash")
    hash = string.lower(hash)
    local state = field(hash, "state")

    if state == nil then
        return "none"
    end

    return state,
        address.parse(field(hash, "locker")),
        address.parse(field(hash, "recipient")),
        address.parse(field(hash, "token")),
        u256(field(hash, "amount")),
        u256(field(hash, "deadline")),
        field(hash, "secret") or ""
end

return M
