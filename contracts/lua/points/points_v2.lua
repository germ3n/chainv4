-- Version 2 of points_v1.lua, as docs/contract-storage.md upgrades it: badges, a cap on one
-- award, and the name kept under a new key, title. It keeps every namespace and key version 1
-- declared, and retires the one it no longer uses.

--@namespace points address-text -> decimal:points
--@namespace badges address-text -> text
--@namespace config
--@key config owner address-text
--@retired config name
--@key config title text
--@key config max_award decimal:points

local M = {}

local points = storage.namespace("points")
local badges = storage.namespace("badges")
local config = storage.namespace("config")

local function owner()
    return address.parse(config:get("owner"))
end

local function only_the_owner()
    if ctx.sender ~= owner() then
        error("only the owner may")
    end
end

-- Runs once, right after the upgrade. A retired key can still be read and cleared, so its value
-- moves to the key that replaces it.
function M.migrate()
    local name = config:get("name")

    if name ~= nil then
        config:set("title", name)
        config:set("name", nil)
    end

    config:set("max_award", "1000")
end

-- Awards points to someone, at most max_award at once, and returns what they hold now.
function M.award(who, amount)
    only_the_owner()

    amount = u256(amount) -- whatever integer type, or whole number, the caller sent
    local most = u256(config:get("max_award"))

    if amount > most then
        error("one award is at most " .. tostring(most) .. " points")
    end

    local now = u256(points:get(who:text()) or "0") + amount
    points:set(who:text(), tostring(now))

    return now
end

-- Gives someone a badge, in place of any they had.
function M.give_badge(who, badge)
    only_the_owner()
    badges:set(who:text(), badge)
end

function M.points_of(who)
    return u256(points:get(who:text()) or "0")
end

function M.badge_of(who)
    return badges:get(who:text())
end

function M.title()
    return config:get("title")
end

function M.check_perms(permission, who)
    return permission == "upgrade" and who == owner()
end

return M
