-- A decentralized exchange on chainv4: constant-product pools, as Uniswap V2 keeps them. A pool
-- holds two assets, and trades one for the other at whatever price keeps the product of what it
-- holds from falling, less a 0.3% fee that stays in the pool for whoever provided it. Each side
-- of a pool is any of:
--   the chain's coin, DEV, named by the zero contract address, contract:0x00…00, and sent with
--   the call;
--   a token following the token standard (docs/token-standard.md), moved with transfer_from;
--   an ERC-20 on the chain's EVM, by its evm: address, moved with transferFrom through evm.call.
-- Approve this contract for a token before adding or trading it: an ERC-20 at this contract's
-- EVM address, which the EVM sees it at.
--
-- Whoever adds to a pool gets its share token, a clone of contracts/lua/lp_token.lua that this
-- contract opened the pool with, and takes their part of both assets back by burning it. Every
-- change ends with a Sync event, the pool's assets after it: what prices and charts are read
-- from. Deadlines are Unix milliseconds, by ctx.time.

local M = {}

-- The coin, as a side of a pool: no contract lives at the zero contract address, and a
-- transaction may name it, as it may not the zero key.
local COIN = address.parse("contract:0x" .. string.rep("0", 64))
-- Shares never taken out, so a pool once opened is never empty, as in Uniswap V2.
local MINIMUM_LIQUIDITY = u256(1000)
local ZERO = u256(0)

local pools = storage.namespace("pools")

-- A pair's two sides in their one order, and the salt that names its pool.
local function sorted(a, b)
    if a:text() == b:text() then
        error("a pool trades two different assets")
    end

    if a:text() < b:text() then
        return a, b, a:text() .. "|" .. b:text()
    end

    return b, a, b:text() .. "|" .. a:text()
end

local function reserves(salt)
    return u256(pools:get(salt .. ":a") or "0"), u256(pools:get(salt .. ":b") or "0")
end

local function exists(salt)
    return pools:get(salt .. ":a") ~= nil
end

local function sync(salt, a, b, reserve_a, reserve_b)
    pools:set(salt .. ":a", tostring(reserve_a))
    pools:set(salt .. ":b", tostring(reserve_b))
    events.emit("Sync", a, b, reserve_a, reserve_b)
end

local function share(salt)
    return contracts.cloned_at(salt)
end

-- A deadline as a number: a plain one, or one of the integer types, such as an i64 a client
-- sends. None for no deadline.
local function before(deadline)
    if deadline == nil then
        return
    end

    if type(deadline) ~= "number" then
        local ok, number = pcall(function()
            return deadline:tonumber()
        end)

        if not ok or type(number) ~= "number" then
            error("the deadline is a time, in milliseconds since 1970")
        end

        deadline = number
    end

    if ctx.time >= deadline then
        error("its deadline passed")
    end
end

local function sqrt(y)
    if y > u256(3) then
        local z, x = y, y / u256(2) + u256(1)

        while x < z do
            z = x
            x = (y / x + x) / u256(2)
        end

        return z
    end

    if y > ZERO then
        return u256(1)
    end

    return ZERO
end

local function erc20_balance(token, owner)
    return evm.decode("(uint256)", evm.call(token, evm.encode("balanceOf(address)", owner)))
end

-- Takes an asset from the caller into the pool: the coin it sent, or a token it approved.
local function take(asset, amount, coin)
    if amount == ZERO then
        return
    end

    if asset:text() == COIN:text() then
        if coin.left < amount then
            error("send the coin with the call: " .. tostring(amount) .. " more")
        end

        coin.left = coin.left - amount
    elseif asset:kind() == "evm" then
        local held = erc20_balance(asset, ctx.contract)
        local answer = evm.call(asset, evm.encode("transferFrom(address,address,uint256)", ctx.sender, ctx.contract, amount))

        if #answer > 0 and not evm.decode("(bool)", answer) then
            error("the token would not move")
        end

        if erc20_balance(asset, ctx.contract) - held ~= amount then
            error("the token moved less than the amount: tokens that take a fee are not traded")
        end
    else
        contracts.call(asset, "transfer_from", ctx.sender, ctx.contract, amount)
    end
end

-- Pays an asset out of the pool.
local function give(asset, to, amount)
    if amount == ZERO then
        return
    end

    if asset:text() == COIN:text() then
        accounts.transfer(to, amount)
    elseif asset:kind() == "evm" then
        local answer = evm.call(asset, evm.encode("transfer(address,uint256)", to, amount))

        if #answer > 0 and not evm.decode("(bool)", answer) then
            error("the token would not move")
        end
    else
        contracts.call(asset, "transfer", to, amount)
    end
end

-- The coin a call sent, to spend from and give back what is left.
local function sent()
    return { left = ctx.value }
end

local function refund(coin)
    give(COIN, ctx.sender, coin.left)
end

-- What a trade of amount_in pays out of a pool: Uniswap V2's getAmountOut.
local function amount_out(amount_in, reserve_in, reserve_out)
    if amount_in == ZERO then
        error("trade more than nothing")
    end

    if reserve_in == ZERO or reserve_out == ZERO then
        error("the pool holds nothing to trade")
    end

    local with_fee = amount_in * u256(997)

    return with_fee * reserve_out / (reserve_in * u256(1000) + with_fee)
end

local function open(a, b, salt)
    if storage.get("template") == nil then
        error("the DEX is not set up yet")
    end

    local lp = contracts.clone(address.parse(storage.get("template")), salt)
    pools:set(salt .. ":a", "0")
    pools:set(salt .. ":b", "0")
    local count = tonumber(storage.get("count") or "0")
    storage.set("pool:" .. count, salt)
    storage.set("count", tostring(count + 1))
    events.emit("PoolCreated", a, b, lp)

    return lp
end

-- Run once, at deploy: whoever deploys sets the share token template, once.
function M.init()
    storage.set("deployer", ctx.sender:text())
end

-- The share token every pool clones: set once, by the deployer, before the first pool.
function M.setup(template)
    if ctx.sender:text() ~= storage.get("deployer") then
        error("only the deployer sets the DEX up")
    end

    if storage.get("template") ~= nil then
        error("the DEX is set up already")
    end

    storage.set("template", template:text())
end

-- Opens a pool for two assets, empty until someone adds to it. Its share token is returned.
function M.create_pool(a, b)
    local first, second, salt = sorted(a, b)

    if exists(salt) then
        error("that pool exists")
    end

    return open(first, second, salt)
end

-- Adds to a pool at its price, opening it at the price given if it is new: as much of a and b as
-- keeps the pool's ratio, at most the amounts desired and at least the least. Returns what went
-- in and the shares minted.
function M.add_liquidity(a, b, desired_a, desired_b, least_a, least_b, deadline)
    before(deadline)
    local first, _, salt = sorted(a, b)
    local flipped = first:text() ~= a:text()
    desired_a, desired_b, least_a, least_b = u256(desired_a), u256(desired_b), u256(least_a or 0), u256(least_b or 0)

    if not exists(salt) then
        if flipped then open(b, a, salt) else open(a, b, salt) end
    end

    local reserve_a, reserve_b = reserves(salt)

    if flipped then
        reserve_a, reserve_b = reserve_b, reserve_a
    end

    local amount_a, amount_b = desired_a, desired_b

    if reserve_a > ZERO or reserve_b > ZERO then
        local optimal_b = desired_a * reserve_b / reserve_a

        if optimal_b <= desired_b then
            if optimal_b < least_b then error("the pool's price moved: less b than the least") end
            amount_b = optimal_b
        else
            local optimal_a = desired_b * reserve_a / reserve_b
            if optimal_a < least_a then error("the pool's price moved: less a than the least") end
            amount_a = optimal_a
        end
    end

    local coin = sent()
    take(a, amount_a, coin)
    take(b, amount_b, coin)
    refund(coin)

    local lp = share(salt)
    local supply = contracts.call(lp, "total_supply")
    local minted

    if supply == ZERO then
        minted = sqrt(amount_a * amount_b)

        if minted <= MINIMUM_LIQUIDITY then
            error("add more: a pool starts with more than 1000 shares' worth")
        end

        minted = minted - MINIMUM_LIQUIDITY
        contracts.call(lp, "mint", ctx.contract, MINIMUM_LIQUIDITY)
    else
        minted = (amount_a * supply / reserve_a):min(amount_b * supply / reserve_b)
    end

    if minted == ZERO then
        error("too little to mint a share")
    end

    contracts.call(lp, "mint", ctx.sender, minted)
    events.emit("Mint", a, b, ctx.sender, amount_a, amount_b, minted)

    if flipped then
        sync(salt, b, a, reserve_b + amount_b, reserve_a + amount_a)
    else
        sync(salt, a, b, reserve_a + amount_a, reserve_b + amount_b)
    end

    return amount_a, amount_b, minted
end

-- Takes shares out of a pool: burns them from the caller, and pays their part of each asset,
-- at least the least.
function M.remove_liquidity(a, b, shares, least_a, least_b, deadline)
    before(deadline)
    local first, _, salt = sorted(a, b)

    if not exists(salt) then
        error("no such pool")
    end

    local flipped = first:text() ~= a:text()
    shares, least_a, least_b = u256(shares), u256(least_a or 0), u256(least_b or 0)
    local reserve_a, reserve_b = reserves(salt)

    if flipped then
        reserve_a, reserve_b = reserve_b, reserve_a
    end

    local lp = share(salt)
    local supply = contracts.call(lp, "total_supply")
    local amount_a, amount_b = shares * reserve_a / supply, shares * reserve_b / supply

    if amount_a == ZERO or amount_b == ZERO then
        error("too few shares to take anything out")
    end

    if amount_a < least_a or amount_b < least_b then
        error("the pool's price moved: less than the least")
    end

    contracts.call(lp, "burn", ctx.sender, shares)
    give(a, ctx.sender, amount_a)
    give(b, ctx.sender, amount_b)
    events.emit("Burn", a, b, ctx.sender, amount_a, amount_b, shares)

    if flipped then
        sync(salt, b, a, reserve_b - amount_b, reserve_a - amount_a)
    else
        sync(salt, a, b, reserve_a - amount_a, reserve_b - amount_b)
    end

    return amount_a, amount_b
end

-- One trade through one pool, in what the pool holds: what comes out.
local function trade(asset_in, asset_out, amount_in)
    local first, _, salt = sorted(asset_in, asset_out)

    if not exists(salt) then
        error("no pool trades " .. asset_in:text() .. " for " .. asset_out:text())
    end

    local reserve_a, reserve_b = reserves(salt)
    local flipped = first:text() ~= asset_in:text()
    local reserve_in, reserve_out = reserve_a, reserve_b

    if flipped then
        reserve_in, reserve_out = reserve_b, reserve_a
    end

    local out = amount_out(amount_in, reserve_in, reserve_out)
    events.emit("Swap", asset_in, asset_out, ctx.sender, amount_in, out)

    if flipped then
        sync(salt, asset_out, asset_in, reserve_out - out, reserve_in + amount_in)
    else
        sync(salt, asset_in, asset_out, reserve_in + amount_in, reserve_out - out)
    end

    return out
end

-- Trades amount_in of asset_in for asset_out, through one pool, or two by way of middle when it
-- is given: at least least_out, or nothing happens. The coin goes in by being sent with the call.
function M.swap(asset_in, asset_out, amount_in, least_out, deadline, middle)
    before(deadline)
    amount_in, least_out = u256(amount_in), u256(least_out or 0)
    local coin = sent()
    take(asset_in, amount_in, coin)
    refund(coin)

    local out

    if middle ~= nil then
        out = trade(middle, asset_out, trade(asset_in, middle, amount_in))
    else
        out = trade(asset_in, asset_out, amount_in)
    end

    if out < least_out then
        error("the price moved: less out than the least, " .. tostring(out))
    end

    give(asset_out, ctx.sender, out)

    return out
end

-- What a trade would pay out now, through one pool, or two by way of middle.
function M.quote(asset_in, asset_out, amount_in, middle)
    local function through(from, to, amount)
        local first, _, salt = sorted(from, to)

        if not exists(salt) then
            error("no pool trades " .. from:text() .. " for " .. to:text())
        end

        local reserve_a, reserve_b = reserves(salt)

        if first:text() ~= from:text() then
            reserve_a, reserve_b = reserve_b, reserve_a
        end

        return amount_out(amount, reserve_a, reserve_b)
    end

    amount_in = u256(amount_in)

    if middle ~= nil then
        return through(middle, asset_out, through(asset_in, middle, amount_in))
    end

    return through(asset_in, asset_out, amount_in)
end

-- A pool, by its two assets in either order: what it holds of each, in that order, and its share
-- token; nothing for no pool.
function M.pool(a, b)
    local first, _, salt = sorted(a, b)

    if not exists(salt) then
        return nil
    end

    local reserve_a, reserve_b = reserves(salt)

    if first:text() ~= a:text() then
        reserve_a, reserve_b = reserve_b, reserve_a
    end

    return reserve_a, reserve_b, share(salt)
end

-- How many pools there are, to list them with pool_at.
function M.pool_count()
    return tonumber(storage.get("count") or "0")
end

-- The pool opened index-th, from 0: its assets, what it holds of each, and its share token.
function M.pool_at(index)
    local salt = storage.get("pool:" .. tostring(index))

    if salt == nil then
        error("no pool " .. tostring(index))
    end

    local separator = string.find(salt, "|", 1, true)
    local a, b = address.parse(string.sub(salt, 1, separator - 1)), address.parse(string.sub(salt, separator + 1))
    local reserve_a, reserve_b = reserves(salt)

    return a, b, reserve_a, reserve_b, share(salt)
end

-- The share token template every pool clones.
function M.template()
    return address.parse(storage.get("template"))
end

return M
