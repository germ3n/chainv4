-- An example token following the chainv4 token standard (docs/token-standard.md). Whoever
-- deploys it owns it: they receive the initial supply, may mint more, and may upgrade it.

local M = {}

local NAME = "Example Token"
local SYMBOL = "EXT"
local DECIMALS = 18
local INITIAL_SUPPLY = u256("1000000000000000000000000")
-- The other side of a mint or a burn.
local ZERO = address.from_bytes(string.rep("\0", 32))

local function get(key)
    return u256(storage.get(key) or "0")
end

local function put(key, value)
    storage.set(key, tostring(value))
end

-- Balances live where the token standard says light clients can prove them: the balances
-- namespace, by the holder's address text, as decimal text.
local balances = storage.namespace("balances")

local function balance(owner)
    return u256(balances:get(owner:text()) or "0")
end

local function set_balance(owner, value)
    balances:set(owner:text(), tostring(value))
end

local function allowance_key(owner, spender)
    return "allowance:" .. owner:hex() .. ":" .. spender:hex()
end

local function move(from, to, amount)
    local available = balance(from)

    if available < amount then
        error("insufficient balance")
    end

    set_balance(from, available - amount)
    set_balance(to, balance(to) + amount)
    events.emit("Transfer", from, to, amount)
end

local function mint(to, amount)
    put("supply", get("supply") + amount)
    set_balance(to, balance(to) + amount)
    events.emit("Transfer", ZERO, to, amount)
end

-- Run once by the chain when the token is deployed.
function M.init()
    storage.set("owner", ctx.sender:hex())
    mint(ctx.sender, INITIAL_SUPPLY)
end

function M.name()
    return NAME
end

function M.symbol()
    return SYMBOL
end

function M.decimals()
    return DECIMALS
end

function M.total_supply()
    return get("supply")
end

function M.balance_of(owner)
    return balance(owner)
end

function M.allowance(owner, spender)
    return get(allowance_key(owner, spender))
end

function M.transfer(to, amount)
    move(ctx.sender, to, u256(amount))
    return true
end

function M.approve(spender, amount)
    amount = u256(amount)
    put(allowance_key(ctx.sender, spender), amount)
    events.emit("Approval", ctx.sender, spender, amount)
    return true
end

function M.transfer_from(from, to, amount)
    amount = u256(amount)
    local key = allowance_key(from, ctx.sender)
    local allowed = get(key)

    if allowed < amount then
        error("insufficient allowance")
    end

    put(key, allowed - amount)
    move(from, to, amount)
    return true
end

-- Signed by an owner off the chain, and sent by anyone (docs/token-standard.md, "Signed
-- authorizations"): the owner's key, which must be the one the owner's address is of, and its
-- signature over the text below, by its scheme's own rules.
local function signed_by(owner, key, message, signature)
    local scheme = owner:kind()

    if scheme == "contract" or scheme == "evm" then
        error("only a key's account signs")
    end

    if crypto.address(scheme, key) ~= owner then
        error("the key is not the owner's")
    end

    if not crypto.verify(scheme, key, message, signature) then
        error("the signature does not hold")
    end
end

local function hex(bytes)
    return (bytes:gsub(".", function(byte) return string.format("%02x", string.byte(byte)) end))
end

local function permit_key(owner)
    return "permit:" .. owner:hex()
end

local function used_key(owner, nonce)
    return "used:" .. owner:hex() .. ":" .. hex(nonce)
end

-- What an owner signs to let a spender move its tokens: the chain, this token, both parties, the
-- allowance, the owner's next permit nonce and when it lapses, in milliseconds.
function M.permit_message(owner, spender, amount, deadline)
    return "chainv4 token permit\nchain: " .. tostring(chain.id()) .. "\ntoken: " .. ctx.contract:text() .. "\nowner: " .. owner:text()
        .. "\nspender: " .. spender:text() .. "\namount: " .. tostring(u256(amount)) .. "\nnonce: " .. tostring(M.nonces(owner))
        .. "\ndeadline: " .. tostring(deadline)
end

-- What an owner signs to send tokens a relayer submits: the chain, this token, both parties, the
-- amount, the window it holds in, after one time and before another, and a nonce of 32 bytes the
-- owner picks, used once.
function M.transfer_message(from, to, amount, valid_after, valid_before, nonce)
    return "chainv4 token transfer\nchain: " .. tostring(chain.id()) .. "\ntoken: " .. ctx.contract:text() .. "\nfrom: " .. from:text()
        .. "\nto: " .. to:text() .. "\namount: " .. tostring(u256(amount)) .. "\nvalid after: " .. tostring(valid_after)
        .. "\nvalid before: " .. tostring(valid_before) .. "\nnonce: 0x" .. hex(nonce)
end

-- How many permits an owner has had taken: the nonce its next one signs.
function M.nonces(owner)
    return u256(storage.get(permit_key(owner)) or "0")
end

-- Sets an allowance an owner signed, as approve would from the owner.
function M.permit(owner, spender, amount, deadline, key, signature)
    amount = u256(amount)

    if ctx.time > deadline then
        error("the permit has lapsed")
    end

    signed_by(owner, key, M.permit_message(owner, spender, amount, deadline), signature)
    put(permit_key(owner), M.nonces(owner) + 1)
    put(allowance_key(owner, spender), amount)
    events.emit("Approval", owner, spender, amount)
    return true
end

-- Whether a transfer's nonce was used, or not yet.
function M.authorization_state(owner, nonce)
    return storage.get(used_key(owner, nonce)) ~= nil
end

-- Sends tokens their owner signed for, as transfer would from the owner, once.
function M.transfer_with_authorization(from, to, amount, valid_after, valid_before, nonce, key, signature)
    amount = u256(amount)

    if #nonce ~= 32 then
        error("a nonce is 32 bytes")
    end

    if ctx.time <= valid_after then
        error("the authorization is not valid yet")
    end

    if ctx.time >= valid_before then
        error("the authorization has lapsed")
    end

    if M.authorization_state(from, nonce) then
        error("the authorization was used")
    end

    signed_by(from, key, M.transfer_message(from, to, amount, valid_after, valid_before, nonce), signature)
    storage.set(used_key(from, nonce), "1")
    events.emit("AuthorizationUsed", from, nonce)
    move(from, to, amount)
    return true
end

-- Beyond the standard: only the owner mints more.
function M.mint(to, amount)
    if ctx.sender:hex() ~= storage.get("owner") then
        error("only the owner mints")
    end

    mint(to, u256(amount))
    return true
end

-- Asked by the chain before an upgrade: only the owner may.
function M.check_perms(permission, who)
    return permission == "upgrade" and who:hex() == storage.get("owner")
end

return M
