-- Trades across both of chainv4's DEXes at once: part of an amount through the native DEX,
-- contracts/lua/dex.lua, and the rest through the EVM's, Dex.sol's DexRouter, in one transaction,
-- all or nothing, with the least out checked on what both paid together. The apps work out the
-- split that pays the most (src/dex.rs, best_split) and send it here.
--
-- Each side of the trade is the coin, contract:0x00…00, sent with the call, or any asset the
-- native DEX trades; the EVM's side takes the coin, which goes in and out as WDEV, and ERC-20s.
-- Approve this contract for a token before trading it. Deadlines are Unix milliseconds.

local M = {}

local COIN = address.parse("contract:0x" .. string.rep("0", 64))
local ZERO = u256(0)

local function is_coin(asset)
    return asset:text() == COIN:text()
end

-- Run once, at deploy: whoever deploys names the DEXes, once.
function M.init()
    storage.set("deployer", ctx.sender:text())
end

-- The native DEX, the EVM's DexRouter and its WDEV, named once by the deployer.
function M.setup(dex, evm_router, wdev)
    if ctx.sender:text() ~= storage.get("deployer") then
        error("only the deployer sets the router up")
    end

    if storage.get("dex") ~= nil then
        error("the router is set up already")
    end

    storage.set("dex", dex:text())
    storage.set("evm_router", evm_router:text())
    storage.set("wdev", wdev:text())
end

local function named(key)
    local text = storage.get(key)

    if text == nil then
        error("the router is not set up yet")
    end

    return address.parse(text)
end

local function erc20_call(token, signature, ...)
    local answer = evm.call(token, evm.encode(signature, ...))

    if #answer > 0 and not evm.decode("(bool)", answer) then
        error("the token would not move")
    end
end

-- Takes amount of an asset from the caller: the coin it sent, or a token it approved.
local function take(asset, amount)
    if is_coin(asset) then
        if ctx.value ~= amount then
            error("send exactly the amount of coin traded with the call")
        end
    elseif asset:kind() == "evm" then
        erc20_call(asset, "transferFrom(address,address,uint256)", ctx.sender, ctx.contract, amount)
    else
        contracts.call(asset, "transfer_from", ctx.sender, ctx.contract, amount)
    end
end

local function give(asset, to, amount)
    if amount == ZERO then
        return
    end

    if is_coin(asset) then
        accounts.transfer(to, amount)
    elseif asset:kind() == "evm" then
        erc20_call(asset, "transfer(address,uint256)", to, amount)
    else
        contracts.call(asset, "transfer", to, amount)
    end
end

-- Lets a spender take amount of a token from this contract.
local function approve(asset, spender, amount)
    if asset:kind() == "evm" then
        erc20_call(asset, "approve(address,uint256)", spender, amount)
    else
        contracts.call(asset, "approve", spender, amount)
    end
end

local function whole(value)
    if value == nil then
        return nil
    end

    if type(value) ~= "number" then
        return value:tonumber()
    end

    return value
end

-- Trades amount_in of asset_in for asset_out, native_amount of it through the native DEX at
-- fee, by way of middle at middle_fee when middle is given, and evm_amount through the EVM's
-- DexRouter along evm_route, the ABI encoding of (address[] path, uint256[] fees), WDEV standing
-- for the coin at either end. The two amounts add up to amount_in; either may be nothing. What
-- both pay comes to the caller, at least least_out, or nothing happens.
function M.route(asset_in, asset_out, amount_in, least_out, deadline, native_amount, fee, middle, middle_fee, evm_amount, evm_route)
    deadline = whole(deadline)

    if deadline ~= nil and ctx.time >= deadline then
        error("its deadline passed")
    end

    amount_in, least_out = u256(amount_in), u256(least_out or 0)
    native_amount, evm_amount = u256(native_amount or 0), u256(evm_amount or 0)

    if native_amount + evm_amount ~= amount_in then
        error("the two parts add up to the amount traded")
    end

    take(asset_in, amount_in)
    local out = ZERO

    if native_amount > ZERO then
        local dex = named("dex")
        local value = ZERO

        if is_coin(asset_in) then
            value = native_amount
        else
            approve(asset_in, dex, native_amount)
        end

        out = out + contracts.call_value(dex, value, "swap", asset_in, asset_out, fee, native_amount, 0, deadline, middle, middle_fee)
    end

    if evm_amount > ZERO then
        local router, wdev = named("evm_router"), named("wdev")
        local path, fees = evm.decode("(address[],uint256[])", evm_route)
        -- DexRouter's deadlines are Unix seconds.
        local seconds = (deadline or ctx.time + 60000) // 1000
        local answer

        if #path < 2 or (is_coin(asset_in) and path[1]:text() ~= wdev:text()) or (is_coin(asset_out) and path[#path]:text() ~= wdev:text()) then
            error("the EVM's path runs from asset_in to asset_out, WDEV for the coin")
        end

        if is_coin(asset_in) then
            answer = evm.call(router, evm.encode("swapExactDEVForTokens(uint256,address[],uint256[],address,uint256)", 0, path, fees, ctx.contract, seconds), evm_amount)
        else
            approve(asset_in, router, evm_amount)

            if is_coin(asset_out) then
                answer = evm.call(router, evm.encode("swapExactTokensForDEV(uint256,uint256,address[],uint256[],address,uint256)", evm_amount, 0, path, fees, ctx.contract, seconds))
            else
                answer = evm.call(router, evm.encode("swapExactTokensForTokens(uint256,uint256,address[],uint256[],address,uint256)", evm_amount, 0, path, fees, ctx.contract, seconds))
            end
        end

        local amounts = evm.decode("(uint256[])", answer)
        out = out + u256(amounts[#amounts])
    end

    if out < least_out then
        error("the price moved: less out than the least, " .. tostring(out))
    end

    give(asset_out, ctx.sender, out)
    events.emit("Routed", asset_in, asset_out, ctx.sender, amount_in, native_amount, evm_amount, out)

    return out
end

return M
