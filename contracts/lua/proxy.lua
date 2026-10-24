-- A proxy that is upgradable without the built-in upgrade. Every function it does not export
-- itself reaches fallback, which runs its implementation's code on the proxy's own storage,
-- balance and events: callers only ever see the proxy's address. Upgrading is pointing it at a
-- new implementation. The proxy has no check_perms, so the built-in upgrade can never touch it.

local M = {}

-- Kept in the proxy's own space, apart from every key an implementation uses. Implementations
-- may read them, but only the proxy's own code can change them.
local OWNER = "owner"
local IMPLEMENTATION = "implementation"

-- Run once by the chain when the proxy is deployed.
function M.init()
    storage.own:set(OWNER, ctx.sender:text())
end

-- Points the proxy at new code. Storage stays, so the new code must keep the old code's storage
-- layout, as an upgrade's would.
function M.set_implementation(implementation)
    if ctx.sender:text() ~= storage.own:get(OWNER) then
        error("only the owner")
    end

    local current = storage.own:get(IMPLEMENTATION)

    if current ~= nil then
        contracts.check_layout(address.parse(current), implementation)
    end

    storage.own:set(IMPLEMENTATION, implementation:text())
    events.emit("Upgraded", implementation)
end

function M.implementation()
    local text = storage.own:get(IMPLEMENTATION)

    if text == nil then
        error("no implementation set")
    end

    return address.parse(text)
end

-- Run by the chain for every function the proxy does not export. ctx.args are the caller's
-- arguments exactly as they came, and what the implementation returns goes back the same way,
-- so no value changes type passing through Lua.
function M.fallback(selector)
    return contracts.delegate(M.implementation(), selector, ctx.args)
end

return M
