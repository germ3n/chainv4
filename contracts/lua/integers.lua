-- Integer math past what a number holds, unsigned and signed: contracts/examples/integers in Lua,
-- with the same layout, so either engine can take over the other's storage. Arguments arrive as
-- their own types, so amount is a U256 and change an I256 when the caller sends those.

--@namespace ledger
--@key ledger owner address
--@key ledger balance i256
--@key ledger small i128
--@key ledger big u128
--@key ledger supply u256

local M = {}

local ledger = storage.namespace("ledger")

function M.init()
    ledger:set_address("owner", ctx.sender)
end

function M.check_perms(permission, who)
    return permission == "upgrade" and ledger:get_address("owner") == who
end

-- A fee in basis points, rounding down: amount * bps / 10000.
function M.fee(amount, bps)
    return amount * bps / 10000
end

-- Adds a change, above zero or below it, to the balance, and returns the balance.
function M.settle(change)
    local balance = (ledger:get_i256("balance") or i256.zero) + change
    ledger:set_i256("balance", balance)

    return balance
end

-- Adds to the supply, failing with a message of its own rather than "u256 overflow".
function M.mint(amount)
    local supply = (ledger:get_u256("supply") or u256.zero):checked_add(amount)

    if not supply then
        error("the supply would overflow")
    end

    ledger:set_u256("supply", supply)

    return supply
end

-- Keeps an i128 and a u128, and reads them back.
function M.keep(small, big)
    ledger:set_i128("small", small)
    ledger:set_u128("big", big)

    return ledger:get_i128("small") or i128.zero, ledger:get_u128("big") or u128.zero
end

-- What everything kept holds: the balance, the supply and the two small ones.
function M.held()
    return ledger:get_i256("balance") or i256.zero, ledger:get_u256("supply") or u256.zero,
        ledger:get_i128("small") or i128.zero, ledger:get_u128("big") or u128.zero
end

-- The edges: checked gives nil, saturating stops, wrapping wraps, and / and % round toward zero,
-- as Rust's do.
function M.edges()
    return u256.max:checked_add(1), u256.zero:saturating_sub(1), u256.max:wrapping_add(1),
        i256.min:checked_neg(), i256.max:wrapping_add(1), i256(-7) / 2, i256(-7) % 2, u256(10) ^ 30
end

-- Comparisons with plain numbers, either way round.
function M.compare(amount, change)
    return amount > 1000, 1000 < amount, change < 0, amount == 5
end

-- The operators never wrap: this fails with "u256 overflow".
function M.overflow()
    return u256.max + 1
end

return M
