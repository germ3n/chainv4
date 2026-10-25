-- A guestbook: anyone signs it with a message, and anyone may tip whoever signed last. Every
-- call emits an event with the addresses in it, so the event index and the explorer have
-- something to show. Its state lives in namespaces its layout declares, so an upgrade must keep
-- them.

--@namespace signatures address-text -> decimal:signatures
--@namespace book
--@key book total decimal:signatures
--@key book last address-text

local M = {}

-- How many times each address signed, by its text form.
local signatures = storage.namespace("signatures")
-- How many signatures in all, and who signed last.
local book = storage.namespace("book")

local MAX_MESSAGE = 140

-- Signs the book with a message: emits Signed with the signer, the message and the book's count
-- so far, and returns that count.
function M.sign(message)
    if type(message) ~= "string" or #message == 0 or #message > MAX_MESSAGE then
        error("a message is 1 to " .. MAX_MESSAGE .. " bytes")
    end

    local who = ctx.sender:text()
    local mine = u256(signatures:get(who) or "0") + u256(1)
    local total = u256(book:get("total") or "0") + u256(1)
    signatures:set(who, tostring(mine))
    book:set("total", tostring(total))
    book:set("last", who)
    events.emit("Signed", ctx.sender, message, total)

    return total
end

-- Passes what the call sent on to whoever signed last, and emits Tipped with who tipped, who got
-- it and how much.
function M.tip()
    local last = book:get("last")

    if last == nil then
        error("nobody has signed yet")
    end

    if ctx.value == 0 then
        error("send something to tip")
    end

    local to = address.parse(last)
    accounts.transfer(to, ctx.value)
    events.emit("Tipped", ctx.sender, to, ctx.value)
end

-- How many times an address signed.
function M.signatures(who)
    return u256(signatures:get(who:text()) or "0")
end

-- How many signatures the book holds.
function M.total()
    return u256(book:get("total") or "0")
end

-- Who signed last, or nil before anyone did.
function M.last()
    local last = book:get("last")

    if last ~= nil then
        return address.parse(last)
    end
end

return M
