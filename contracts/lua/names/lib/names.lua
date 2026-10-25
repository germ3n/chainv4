-- Where the registry keeps its state: each name's holder, and how many names each address holds.
-- Included by names.lua; the layout below is the contract's, wherever it is declared.

--@namespace names text -> address-text
--@namespace held address-text -> decimal:names

local names = storage.namespace("names")
local held = storage.namespace("held")

-- Who holds a name, or nil.
local function holder(name)
    local text = names:get(name)

    return text and address.parse(text)
end

-- How many names an address holds.
local function count(who)
    return u256(held:get(who:text()) or "0")
end

-- Gives a name to an address, from the one that held it, or from nobody.
local function give(name, from, to)
    names:set(name, to:text())
    held:set(to:text(), tostring(count(to) + u256(1)))

    if from ~= nil then
        held:set(from:text(), tostring(count(from) - u256(1)))
    end
end
