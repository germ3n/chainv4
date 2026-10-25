-- A name registry, split across files: anyone claims a free name, and a name's holder may hand it
-- on. Each --@include line below is replaced by that file when the contract is deployed, by
-- `chainv4 send deploy --lua names.lua` or the explorer, so the chain gets one source, and the
-- included files' locals are this file's.

local M = {}

--@include "lib/checks.lua"
--@include "lib/names.lua"

-- Claims a free name for the sender, and emits Claimed.
function M.claim(name)
    check_name(name)

    if holder(name) ~= nil then
        error(name .. " is taken")
    end

    give(name, nil, ctx.sender)
    events.emit("Claimed", ctx.sender, name)
end

-- Hands a name the sender holds to another address, and emits Handed.
function M.hand(name, to)
    check_name(name)

    if holder(name) ~= ctx.sender then
        error("only the holder of " .. name .. " may hand it on")
    end

    give(name, ctx.sender, to)
    events.emit("Handed", ctx.sender, to, name)
end

-- Who holds a name, or nil while it is free.
function M.holder(name)
    return holder(name)
end

-- How many names an address holds.
function M.held(who)
    return count(who)
end

return M
