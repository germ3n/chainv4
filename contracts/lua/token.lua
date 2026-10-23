-- An example token following the chainv4 token standard (docs/token-standard.md). Whoever
-- deploys it owns it: they receive the initial supply, may mint more, and may upgrade it.

local M = {}

local NAME = "Example Token"
local SYMBOL = "EXT"
local DECIMALS = 18
local INITIAL_SUPPLY = u256("1000000000000000000000000")
-- The other side of a mint or a burn.
local ZERO = address.from_bytes(string.rep("\0", 32))

local function get(key)
    return u256(storage.get(key) or "0")
end

local function put(key, value)
    storage.set(key, tostring(value))
end

local function balance_key(owner)
    return "balance:" .. owner:hex()
end

local function allowance_key(owner, spender)
    return "allowance:" .. owner:hex() .. ":" .. spender:hex()
end

local function move(from, to, amount)
    local available = get(balance_key(from))

    if available < amount then
        error("insufficient balance")
    end

    put(balance_key(from), available - amount)
    put(balance_key(to), get(balance_key(to)) + amount)
    events.emit("Transfer", from, to, amount)
end

local function mint(to, amount)
    put("supply", get("supply") + amount)
    put(balance_key(to), get(balance_key(to)) + amount)
    events.emit("Transfer", ZERO, to, amount)
end

-- Run once by the chain when the token is deployed.
function M.init()
    storage.set("owner", ctx.sender:hex())
    mint(ctx.sender, INITIAL_SUPPLY)
end

function M.name()
    return NAME
end

function M.symbol()
    return SYMBOL
end

function M.decimals()
    return DECIMALS
end

function M.total_supply()
    return get("supply")
end

function M.balance_of(owner)
    return get(balance_key(owner))
end

function M.allowance(owner, spender)
    return get(allowance_key(owner, spender))
end

function M.transfer(to, amount)
    move(ctx.sender, to, u256(amount))
    return true
end

function M.approve(spender, amount)
    amount = u256(amount)
    put(allowance_key(ctx.sender, spender), amount)
    events.emit("Approval", ctx.sender, spender, amount)
    return true
end

function M.transfer_from(from, to, amount)
    amount = u256(amount)
    local key = allowance_key(from, ctx.sender)
    local allowed = get(key)

    if allowed < amount then
        error("insufficient allowance")
    end

    put(key, allowed - amount)
    move(from, to, amount)
    return true
end

-- Beyond the standard: only the owner mints more.
function M.mint(to, amount)
    if ctx.sender:hex() ~= storage.get("owner") then
        error("only the owner mints")
    end

    mint(to, u256(amount))
    return true
end

-- Asked by the chain before an upgrade: only the owner may.
function M.check_perms(permission, who)
    return permission == "upgrade" and who:hex() == storage.get("owner")
end

return M
