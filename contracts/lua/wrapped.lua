-- A wrapped asset: a token following the chainv4 token standard (docs/token-standard.md) that a
-- bridge's gateway (contracts/lua/gateway.lua) clones for each coin or token it brings in from
-- another chain. Its owner, the gateway, alone mints it, one for one against what it holds locked
-- there, and burns it when it goes back. Held, it moves as any token.

local M = {}

local ZERO = address.from_bytes(string.rep("\0", 32))

local balances = storage.namespace("balances")

local function balance(owner)
    return u256(balances:get(owner:text()) or "0")
end

local function set_balance(owner, value)
    balances:set(owner:text(), value ~= u256(0) and tostring(value) or nil)
end

local function supply()
    return u256(storage.get("supply") or "0")
end

local function allowance_key(owner, spender)
    return "allowance:" .. owner:hex() .. ":" .. spender:hex()
end

local function only_owner()
    if ctx.sender:text() ~= storage.get("owner") then
        error("only the gateway mints and burns")
    end
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

-- Run once, by the gateway's clone: what the asset is called here, its decimals, and the gateway.
-- Deployed bare, as the template clones copy, it has no owner, so it mints for no one, and names
-- itself only for the token registry.
function M.init(name, symbol, decimals, owner)
    if owner == nil then
        storage.set("name", "Wrapped asset template")
        storage.set("symbol", "WRAP")
        storage.set("decimals", "0")
        return
    end

    storage.set("name", name)
    storage.set("symbol", symbol)
    storage.set("decimals", tostring(decimals))
    storage.set("owner", owner:text())
end

function M.name()
    return storage.get("name")
end

function M.symbol()
    return storage.get("symbol")
end

function M.decimals()
    return tonumber(storage.get("decimals"))
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
    local allowed = M.allowance(from, ctx.sender)

    if allowed < amount then
        error("insufficient allowance")
    end

    storage.set(allowance_key(from, ctx.sender), tostring(allowed - amount))
    move(from, to, amount)
    return true
end

-- The gateway mints what arrived from the other chain.
function M.mint(to, amount)
    only_owner()
    amount = u256(amount)
    storage.set("supply", tostring(supply() + amount))
    set_balance(to, balance(to) + amount)
    events.emit("Transfer", ZERO, to, amount)
    return true
end

-- The gateway burns what goes back, from whoever sends it back through the gateway.
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

-- What it wraps, for anyone to check against the other chain: the chain id it comes from and its
-- address there, as the gateway set them.
function M.origin()
    local gateway = address.parse(storage.get("owner"))

    return contracts.call(gateway, "origin_of", ctx.contract)
end

return M
