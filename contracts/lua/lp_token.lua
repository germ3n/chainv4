-- A pool's share in the DEX (contracts/lua/dex.lua): a token following the chainv4 token standard
-- (docs/token-standard.md), which the DEX clones for each pool it opens. Whoever clones it owns
-- it, the DEX, and alone mints it to who adds to the pool, and burns it from who takes out. Held,
-- it moves as any token: transfer, approve, transfer_from.

local M = {}

local ZERO = address.from_bytes(string.rep("\0", 32))

local balances = storage.namespace("balances")

local function balance(owner)
    return u256(balances:get(owner:text()) or "0")
end

local function set_balance(owner, value)
    balances:set(owner:text(), tostring(value))
end

local function supply()
    return u256(storage.get("supply") or "0")
end

local function allowance_key(owner, spender)
    return "allowance:" .. owner:hex() .. ":" .. spender:hex()
end

local function move(from, to, amount)
    local available = balance(from)

    if available < amount then
        error("insufficient balance")
    end

    set_balance(from, available - amount)
    set_balance(to, balance(to) + amount)
    events.emit("Transfer", from, to, amount)
end

local function only_owner()
    if ctx.sender:hex() ~= storage.get("owner") then
        error("only the pool mints and burns its shares")
    end
end

-- Run once, by the chain when the template is deployed, or by the DEX as it clones it.
function M.init()
    storage.set("owner", ctx.sender:hex())
end

function M.name()
    return "chainv4 DEX share"
end

function M.symbol()
    return "DEX-LP"
end

function M.decimals()
    return 18
end

function M.total_supply()
    return supply()
end

function M.balance_of(owner)
    return balance(owner)
end

function M.allowance(owner, spender)
    return u256(storage.get(allowance_key(owner, spender)) or "0")
end

function M.transfer(to, amount)
    move(ctx.sender, to, u256(amount))
    return true
end

function M.approve(spender, amount)
    amount = u256(amount)
    storage.set(allowance_key(ctx.sender, spender), tostring(amount))
    events.emit("Approval", ctx.sender, spender, amount)
    return true
end

function M.transfer_from(from, to, amount)
    amount = u256(amount)
    local key = allowance_key(from, ctx.sender)
    local allowed = u256(storage.get(key) or "0")

    if allowed < amount then
        error("insufficient allowance")
    end

    storage.set(key, tostring(allowed - amount))
    move(from, to, amount)
    return true
end

-- Beyond the standard: the pool's own.
function M.mint(to, amount)
    only_owner()
    amount = u256(amount)
    storage.set("supply", tostring(supply() + amount))
    set_balance(to, balance(to) + amount)
    events.emit("Transfer", ZERO, to, amount)
    return true
end

function M.burn(from, amount)
    only_owner()
    amount = u256(amount)
    local available = balance(from)

    if available < amount then
        error("insufficient balance")
    end

    set_balance(from, available - amount)
    storage.set("supply", tostring(supply() - amount))
    events.emit("Transfer", from, ZERO, amount)
    return true
end

return M
