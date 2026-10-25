-- Points an owner awards: the example docs/contract-storage.md walks through. Version 1 keeps who
-- holds how many points in a map, and its settings in a record.

--@namespace points address-text -> decimal:points
--@namespace config
--@key config owner address-text
--@key config name text

local M = {}

local points = storage.namespace("points")
local config = storage.namespace("config")

local function owner()
    return address.parse(config:get("owner"))
end

-- Whoever deploys it owns it.
function M.init()
    config:set("owner", ctx.sender:text())
    config:set("name", "Points")
end

-- Awards points to someone, and returns what they hold now. Only the owner may.
function M.award(who, amount)
    if ctx.sender ~= owner() then
        error("only the owner awards points")
    end

    amount = u256(amount) -- whatever integer type, or whole number, the caller sent
    local now = u256(points:get(who:text()) or "0") + amount
    points:set(who:text(), tostring(now))

    return now
end

-- How many points someone holds.
function M.points_of(who)
    return u256(points:get(who:text()) or "0")
end

-- What the points are called.
function M.name()
    return config:get("name")
end

-- Only the owner may upgrade it.
function M.check_perms(permission, who)
    return permission == "upgrade" and who == owner()
end

return M
