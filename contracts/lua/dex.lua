-- A decentralized exchange on chainv4: constant-product pools, as Uniswap V2 keeps them. A pool
-- holds two assets, and trades one for the other at whatever price keeps the product of what it
-- holds from falling, less its fee. Each side of a pool is any of:
--   the chain's coin, DEV, named by the zero contract address, contract:0x00…00, and sent with
--   the call;
--   a token following the token standard (docs/token-standard.md), moved with transfer_from;
--   an ERC-20 on the chain's EVM, by its evm: address, moved with transferFrom through evm.call.
-- Approve this contract for a token before adding or trading it: an ERC-20 at this contract's
-- EVM address, which the EVM sees it at.
--
-- A pool is a pair and a fee tier, in basis points, one of those the chain offers new pools
-- (chain.dex().tiers), fixed when it opens: a pair may have one pool at each tier. Of each fee a
-- trade pays, the chain's protocol_permille share is burned, the coin out of the supply, a token
-- to the zero address and an ERC-20 to 0x…dEaD, and the rest stays in the pool for whoever provided
-- it. Each pool keeps cumulative prices, as Uniswap V2's do, for a time-weighted price read from two
-- observations, and lends what it holds within one call: a flash swap.
--
-- Whoever adds to a pool gets its share token, a clone of contracts/lua/lp_token.lua that this
-- contract opened the pool with, and takes their part of both assets back by burning it. Every
-- change ends with a Sync event, the pool's assets after it and its tier: what prices and charts
-- are read from. Swap, Mint and Burn name the tier last too, so a pair's pools are told apart. Deadlines and times are Unix milliseconds, by ctx.time.

local M = {}

-- The coin, as a side of a pool: no contract lives at the zero contract address, and a
-- transaction may name it, as it may not the zero key.
local COIN = address.parse("contract:0x" .. string.rep("0", 64))
-- Where a token's burned share goes: the zero address, the token standard's "nobody".
local NOBODY = address.from_bytes(string.rep("\0", 32))
-- Where an ERC-20's burned share goes: the address Ethereum's tools burn to.
local DEAD = address.parse("evm:0x000000000000000000000000000000000000dead")
-- Shares never taken out, so a pool once opened is never empty, as in Uniswap V2.
local MINIMUM_LIQUIDITY = u256(1000)
local ZERO = u256(0)
-- The fixed point cumulative prices are kept in: a price of 1 is 10^18.
local ONE = u256("1000000000000000000")

local pools = storage.namespace("pools")

-- A fee tier as a whole number of basis points, from a number or one of the integer types.
local function tier_of(fee)
    if fee == nil then
        error("name the pool's fee tier, in basis points")
    end

    if type(fee) ~= "number" then
        fee = fee:tonumber()
    end

    return fee
end

-- A pair's two sides in their one order, and the salt that names its pool at a tier.
local function sorted(a, b, fee)
    if a:text() == b:text() then
        error("a pool trades two different assets")
    end

    local tier = tostring(tier_of(fee))

    if a:text() < b:text() then
        return a, b, a:text() .. "|" .. b:text() .. "|" .. tier
    end

    return b, a, b:text() .. "|" .. a:text() .. "|" .. tier
end

local function reserves(salt)
    return u256(pools:get(salt .. ":a") or "0"), u256(pools:get(salt .. ":b") or "0")
end

local function exists(salt)
    return pools:get(salt .. ":a") ~= nil
end

-- The pool's cumulative prices as of a time: each side's price in the other, in ONE's fixed point,
-- times the milliseconds it held, summed since the pool opened, as Uniswap V2 sums them, wrapping.
local function cumulative(salt, now)
    local price_a, price_b = u256(pools:get(salt .. ":ca") or "0"), u256(pools:get(salt .. ":cb") or "0")
    local last = tonumber(pools:get(salt .. ":t") or tostring(now))
    local reserve_a, reserve_b = reserves(salt)
    local elapsed = now - last

    -- They wrap past 2^256, as V2's do: a reader takes the difference of two, which wrapping keeps.
    if elapsed > 0 and reserve_a > ZERO and reserve_b > ZERO then
        price_a = price_a:wrapping_add((reserve_b:wrapping_mul(ONE) / reserve_a):wrapping_mul(u256(elapsed)))
        price_b = price_b:wrapping_add((reserve_a:wrapping_mul(ONE) / reserve_b):wrapping_mul(u256(elapsed)))
    end

    return price_a, price_b
end

-- What a pool holds now, after the prices it held until now are summed.
local function sync(salt, a, b, reserve_a, reserve_b)
    local price_a, price_b = cumulative(salt, ctx.time)
    pools:set(salt .. ":ca", tostring(price_a))
    pools:set(salt .. ":cb", tostring(price_b))
    pools:set(salt .. ":t", tostring(ctx.time))
    pools:set(salt .. ":a", tostring(reserve_a))
    pools:set(salt .. ":b", tostring(reserve_b))
    -- The pool's tier last, as a pair may have a pool at each.
    events.emit("Sync", a, b, reserve_a, reserve_b, tonumber(string.match(salt, "|(%d+)$")))
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

-- Takes an asset from an account into the pool: the coin the call sent, or a token it approved.
local function take_from(from, asset, amount, coin)
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
        local answer = evm.call(asset, evm.encode("transferFrom(address,address,uint256)", from, ctx.contract, amount))

        if #answer > 0 and not evm.decode("(bool)", answer) then
            error("the token would not move")
        end

        if erc20_balance(asset, ctx.contract) - held ~= amount then
            error("the token moved less than the amount: tokens that take a fee are not traded")
        end
    else
        -- What arrived, not what was asked: a token that keeps a fee, or lies, would leave the
        -- pool's reserves above what it holds.
        local held = contracts.call(asset, "balance_of", ctx.contract)
        contracts.call(asset, "transfer_from", from, ctx.contract, amount)

        if contracts.call(asset, "balance_of", ctx.contract) - held ~= amount then
            error("the token moved less than the amount: tokens that take a fee are not traded")
        end
    end
end

local function take(asset, amount, coin)
    take_from(ctx.sender, asset, amount, coin)
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

-- Burns the protocol's share of a fee: the coin out of the supply, a token to nobody.
local function burn(asset, amount)
    if amount == ZERO then
        return
    end

    if asset:text() == COIN:text() then
        accounts.burn(amount)
    elseif asset:kind() == "evm" then
        give(asset, DEAD, amount)
    else
        give(asset, NOBODY, amount)
    end

    events.emit("Burned", asset, amount)
end

-- The coin a call sent, to spend from and give back what is left.
local function sent()
    return { left = ctx.value }
end

local function refund(coin)
    give(COIN, ctx.sender, coin.left)
end

-- A fee taken in, in basis points, and the protocol's part of it, burned.
local function fee_of(amount, fee)
    local paid = amount * u256(fee) / u256(10000)

    return paid, paid * u256(chain.dex().protocol_permille) / u256(1000)
end

-- What a trade of amount_in pays out of a pool at a fee: Uniswap V2's getAmountOut.
local function amount_out(amount_in, reserve_in, reserve_out, fee)
    if amount_in == ZERO then
        error("trade more than nothing")
    end

    if reserve_in == ZERO or reserve_out == ZERO then
        error("the pool holds nothing to trade")
    end

    local with_fee = amount_in * (u256(10000) - u256(fee))

    return with_fee * reserve_out / (reserve_in * u256(10000) + with_fee)
end

local function offered(fee)
    for _, tier in ipairs(chain.dex().tiers) do
        if tier == fee then
            return true
        end
    end

    return false
end

local function open(a, b, fee, salt)
    if storage.get("template") == nil then
        error("the DEX is not set up yet")
    end

    if not offered(fee) then
        error("the chain offers no " .. tostring(fee) .. " basis point tier for new pools")
    end

    local lp = contracts.clone(address.parse(storage.get("template")), salt)
    pools:set(salt .. ":a", "0")
    pools:set(salt .. ":b", "0")
    pools:set(salt .. ":t", tostring(ctx.time))
    local count = tonumber(storage.get("count") or "0")
    storage.set("pool:" .. count, salt)
    storage.set("count", tostring(count + 1))
    events.emit("PoolCreated", a, b, fee, lp)

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

-- Opens a pool for two assets at a fee tier the chain offers, empty until someone adds to it. Its
-- share token is returned.
function M.create_pool(a, b, fee)
    fee = tier_of(fee)
    local first, second, salt = sorted(a, b, fee)

    if exists(salt) then
        error("that pool exists")
    end

    return open(first, second, fee, salt)
end

-- Adds to a pool at its price, opening it at the price given if it is new: as much of a and b as
-- keeps the pool's ratio, at most the amounts desired and at least the least. Returns what went
-- in and the shares minted.
function M.add_liquidity(a, b, fee, desired_a, desired_b, least_a, least_b, deadline)
    before(deadline)
    fee = tier_of(fee)
    local first, _, salt = sorted(a, b, fee)
    local flipped = first:text() ~= a:text()
    desired_a, desired_b, least_a, least_b = u256(desired_a), u256(desired_b), u256(least_a or 0), u256(least_b or 0)

    if not exists(salt) then
        if flipped then open(b, a, fee, salt) else open(a, b, fee, salt) end
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
    events.emit("Mint", a, b, ctx.sender, amount_a, amount_b, minted, tier_of(fee))

    if flipped then
        sync(salt, b, a, reserve_b + amount_b, reserve_a + amount_a)
    else
        sync(salt, a, b, reserve_a + amount_a, reserve_b + amount_b)
    end

    return amount_a, amount_b, minted
end

-- Takes shares out of a pool: burns them from the caller, and pays their part of each asset,
-- at least the least.
function M.remove_liquidity(a, b, fee, shares, least_a, least_b, deadline)
    before(deadline)
    local first, _, salt = sorted(a, b, fee)

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

    -- The shares burned and the reserves written down before anything is paid out, so whatever a
    -- payment calls finds the pool as it is after this, not before.
    contracts.call(lp, "burn", ctx.sender, shares)

    if flipped then
        sync(salt, b, a, reserve_b - amount_b, reserve_a - amount_a)
    else
        sync(salt, a, b, reserve_a - amount_a, reserve_b - amount_b)
    end

    give(a, ctx.sender, amount_a)
    give(b, ctx.sender, amount_b)
    events.emit("Burn", a, b, ctx.sender, amount_a, amount_b, shares, tier_of(fee))

    return amount_a, amount_b
end

-- One trade through one pool, in what the pool holds: what comes out. The protocol's part of the
-- fee leaves the pool and is burned; the rest of what came in stays.
local function trade(asset_in, asset_out, fee, amount_in)
    fee = tier_of(fee)
    local first, _, salt = sorted(asset_in, asset_out, fee)

    if not exists(salt) then
        error("no pool trades " .. asset_in:text() .. " for " .. asset_out:text() .. " at " .. tostring(fee) .. " basis points")
    end

    local reserve_a, reserve_b = reserves(salt)
    local flipped = first:text() ~= asset_in:text()
    local reserve_in, reserve_out = reserve_a, reserve_b

    if flipped then
        reserve_in, reserve_out = reserve_b, reserve_a
    end

    local out = amount_out(amount_in, reserve_in, reserve_out, fee)
    local _, protocol = fee_of(amount_in, fee)
    events.emit("Swap", asset_in, asset_out, ctx.sender, amount_in, out, fee)

    if flipped then
        sync(salt, asset_out, asset_in, reserve_out - out, reserve_in + amount_in - protocol)
    else
        sync(salt, asset_in, asset_out, reserve_in + amount_in - protocol, reserve_out - out)
    end

    burn(asset_in, protocol)

    return out
end

-- Trades amount_in of asset_in for asset_out, through the pool at a fee tier, or two by way of
-- middle when it is given, the second at middle_fee: at least least_out, or nothing happens. The
-- coin goes in by being sent with the call.
function M.swap(asset_in, asset_out, fee, amount_in, least_out, deadline, middle, middle_fee)
    before(deadline)
    amount_in, least_out = u256(amount_in), u256(least_out or 0)
    local coin = sent()
    take(asset_in, amount_in, coin)
    refund(coin)

    local out

    if middle ~= nil then
        out = trade(middle, asset_out, middle_fee or fee, trade(asset_in, middle, fee, amount_in))
    else
        out = trade(asset_in, asset_out, fee, amount_in)
    end

    if out < least_out then
        error("the price moved: less out than the least, " .. tostring(out))
    end

    give(asset_out, ctx.sender, out)

    return out
end

-- What a trade would pay out now, through one pool, or two by way of middle.
function M.quote(asset_in, asset_out, fee, amount_in, middle, middle_fee)
    local function through(from, to, tier, amount)
        local first, _, salt = sorted(from, to, tier)

        if not exists(salt) then
            error("no pool trades " .. from:text() .. " for " .. to:text() .. " at " .. tostring(tier_of(tier)) .. " basis points")
        end

        local reserve_a, reserve_b = reserves(salt)

        if first:text() ~= from:text() then
            reserve_a, reserve_b = reserve_b, reserve_a
        end

        return amount_out(amount, reserve_a, reserve_b, tier_of(tier))
    end

    amount_in = u256(amount_in)

    if middle ~= nil then
        return through(middle, asset_out, middle_fee or fee, through(asset_in, middle, fee, amount_in))
    end

    return through(asset_in, asset_out, fee, amount_in)
end

-- Lends what a pool holds within one call, as Uniswap V2's flash swaps do: amount_a of a and
-- amount_b of b go to `to`, a contract, which this calls back with on_flash(sender, a, b,
-- amount_a, amount_b, data). By the time that returns it must have repaid enough that the pool's
-- product, less the fee on what came back, is no less than before: the coin by calling repay with
-- it, while the loan is out, and a token by approving this contract, which takes paid_a and paid_b,
-- what on_flash returns, from `to`. It may repay in either asset, or both.
function M.flash(a, b, fee, amount_a, amount_b, to, data)
    fee = tier_of(fee)
    local first, _, salt = sorted(a, b, fee)

    if not exists(salt) then
        error("no such pool")
    end

    if storage.get("flash") ~= nil then
        error("a flash swap is under way")
    end

    local flipped = first:text() ~= a:text()
    amount_a, amount_b = u256(amount_a or 0), u256(amount_b or 0)
    local reserve_a, reserve_b = reserves(salt)

    if flipped then
        reserve_a, reserve_b = reserve_b, reserve_a
    end

    if amount_a >= reserve_a or amount_b >= reserve_b then
        error("a flash swap takes less than the pool holds")
    end

    storage.set("flash", to:text())
    storage.set("flash:coin", "0")
    give(a, to, amount_a)
    give(b, to, amount_b)
    local paid_a, paid_b = contracts.call(to, "on_flash", ctx.sender, a, b, amount_a, amount_b, data or "")
    paid_a, paid_b = u256(paid_a or 0), u256(paid_b or 0)
    local coin = { left = u256(storage.get("flash:coin")) }
    take_from(to, a, paid_a, coin)
    take_from(to, b, paid_b, coin)
    storage.set("flash", nil)
    storage.set("flash:coin", nil)

    if coin.left > ZERO then
        give(COIN, to, coin.left)
    end

    -- What it holds now, less the fee on what came in: no less a product than before.
    local fee_a, burn_a = fee_of(paid_a, fee)
    local fee_b, burn_b = fee_of(paid_b, fee)
    local held_a, held_b = reserve_a - amount_a + paid_a, reserve_b - amount_b + paid_b

    if (held_a - fee_a) * (held_b - fee_b) < reserve_a * reserve_b then
        error("a flash swap must repay, with the fee, as much as it took")
    end

    if flipped then
        sync(salt, b, a, held_b - burn_b, held_a - burn_a)
    else
        sync(salt, a, b, held_a - burn_a, held_b - burn_b)
    end

    burn(a, burn_a)
    burn(b, burn_b)
    events.emit("Flash", a, b, to, amount_a, amount_b, paid_a, paid_b)

    return true
end

--@reentrant
function M.repay()
    if storage.get("flash") ~= ctx.sender:text() then
        error("only the contract a flash swap lent to repays it, while it is out")
    end

    storage.set("flash:coin", tostring(u256(storage.get("flash:coin")) + ctx.value))

    return true
end

-- A pool, by its two assets in either order and its tier: what it holds of each, in that order,
-- its share token, and its cumulative prices of each side in the other as of now, in 10^18 fixed
-- point times milliseconds, for a time-weighted price: the difference between two observations
-- over the milliseconds between them. Nothing for no pool.
function M.pool(a, b, fee)
    local first, _, salt = sorted(a, b, fee)

    if not exists(salt) then
        return nil
    end

    local reserve_a, reserve_b = reserves(salt)
    local price_a, price_b = cumulative(salt, ctx.time)

    if first:text() ~= a:text() then
        reserve_a, reserve_b = reserve_b, reserve_a
        price_a, price_b = price_b, price_a
    end

    return reserve_a, reserve_b, share(salt), price_a, price_b
end

-- How many pools there are, to list them with pool_at.
function M.pool_count()
    return tonumber(storage.get("count") or "0")
end

-- The pool opened index-th, from 0: its assets, its fee tier, what it holds of each, and its
-- share token.
function M.pool_at(index)
    local salt = storage.get("pool:" .. tostring(index))

    if salt == nil then
        error("no pool " .. tostring(index))
    end

    local first = string.find(salt, "|", 1, true)
    local second = string.find(salt, "|", first + 1, true)
    local a, b = address.parse(string.sub(salt, 1, first - 1)), address.parse(string.sub(salt, first + 1, second - 1))
    local fee = tonumber(string.sub(salt, second + 1))
    local reserve_a, reserve_b = reserves(salt)

    return a, b, fee, reserve_a, reserve_b, share(salt)
end

-- The share token template every pool clones.
function M.template()
    return address.parse(storage.get("template"))
end

return M
