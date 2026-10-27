-- A factory for collections: anyone creates their own NFT or multi-token collection with one call,
-- a clone of contracts/lua/nft.lua or contracts/lua/multi.lua that names them its owner. Clones
-- go in the chain's collection registry as any deploy does, and the factory lists what it made.

local M = {}

local made = storage.namespace("made")

local function count()
    return u256(storage.get("count") or "0")
end

-- Run once, at deploy: whoever deploys sets the templates, once.
function M.init()
    storage.set("deployer", ctx.sender:text())
end

-- The collections every create clones: set once, by the deployer.
function M.setup(nft, multi)
    if ctx.sender:text() ~= storage.get("deployer") then
        error("only the deployer sets the templates")
    end

    if storage.get("nft") ~= nil then
        error("the templates are set already")
    end

    storage.set("nft", nft:text())
    storage.set("multi", multi:text())
    return true
end

-- A new collection of a kind, nft or multi, owned by the caller: its address.
function M.create(kind, name, symbol, base_uri)
    local template = (kind == "nft" or kind == "multi") and storage.get(kind)

    if not template then
        error(storage.get("nft") == nil and "the templates are not set" or "kind must be nft or multi")
    end

    local n = count()
    local collection = contracts.clone(address.parse(template), tostring(n), name, symbol, base_uri, ctx.sender)
    made:set(tostring(n), collection:text())
    storage.set("count", tostring(n + u256(1)))
    events.emit("Created", collection, kind, ctx.sender)
    return collection
end

-- How many collections it made, and the nth, counting from 0.
function M.count()
    return count()
end

function M.at(n)
    local text = made:get(tostring(u256(n)))

    return text and address.parse(text)
end

function M.templates()
    local nft, multi = storage.get("nft"), storage.get("multi")

    return nft and address.parse(nft), multi and address.parse(multi)
end

return M
