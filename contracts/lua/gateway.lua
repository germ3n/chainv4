-- A bridge's gateway on chainv4 (docs/bridges.md): the coin, tokens and NFTs leave for other
-- chains through it, and what arrives from them is minted here as wrapped tokens and
-- collections, against what their gateways hold locked. It is also the signature board: the
-- federation's signers post their signatures over each transfer here, one transaction each; once
-- a transfer coming here has the federation's threshold, it is done, and a transfer leaving has
-- what a relayer hands the other chain's gateway.
--
-- A transfer, either way, is its two gateways and chains, a nonce its source gateway counts, what
-- moves and how much, who gets it, and what the asset is called, all hashed in one digest the
-- signers sign and both chains' gateways compute alike (contracts/evm/Gateway.sol):
--   head = keccak256(abi.encode(bytes from, bytes to, uint64 source, uint64 target, uint64 nonce,
--                               uint8 kind, uint64 origin))
--   body = keccak256(abi.encode(bytes asset, uint256 id, uint256 amount, bytes recipient,
--                               string name, string symbol, uint8 decimals, string uri))
--   digest = keccak256(abi.encode("chainv4-gateway", head, body))
-- Chains are named by their EVM chain ids; this one's is given at setup, the id its own EVM
-- answers CHAINID with (crypto::ethereum::chain_id of its genesis). Addresses here
-- are written as Address:encode writes them, 33 bytes, an EVM chain's as its 20; the coin, as no
-- bytes. kind is 0 the coin, 1 a fungible token, 2 an NFT, 3 an amount of a multi-token id; origin
-- is the chain the asset is native to, and asset its address there.
--
-- Events: Sent(nonce, target, digest, sender), Attested(digest, signer), Received(source, nonce, digest),
-- Wrapped(origin, kind, asset, here).

local M = {}

local COIN, TOKEN, NFT, MULTI = 0, 1, 2, 3
local DOMAIN = "chainv4-gateway"
local ZERO = u256(0)

local peers = storage.namespace("peers")
-- What each wrapped asset here stands for, both ways: by origin, kind and asset, and by its
-- address here.
local wrapped = storage.namespace("wrapped")
local origins = storage.namespace("origins")
-- Transfers that left, by nonce: their fields, for signers and relayers to read.
local out = storage.namespace("out")
-- Transfers that arrived and are done, by source chain and nonce.
local done = storage.namespace("done")
-- The signatures posted, by digest and signer key.
local board = storage.namespace("board")

local function hex(bytes)
    return (bytes:gsub(".", function(byte)
        return string.format("%02x", string.byte(byte))
    end))
end

local function owner_only()
    if ctx.sender:text() ~= storage.get("owner") then
        error("only the gateway's owner may")
    end
end

local function here()
    return tonumber(storage.get("chain_id"))
end

local function whole(value, what)
    local number = tonumber(tostring(value))

    if number == nil or number < 0 or number % 1 ~= 0 then
        error(what .. " is a whole number")
    end

    return number
end

-- A transfer's digest, as both chains' gateways and the signers make it.
local function digest(t)
    local head = hash.keccak256(evm.encode("(bytes,bytes,uint64,uint64,uint64,uint8,uint64)", t.from, t.to, t.source, t.target, t.nonce, t.kind, t.origin))
    local body = hash.keccak256(evm.encode("(bytes,uint256,uint256,bytes,string,string,uint8,string)", t.asset, t.id, t.amount, t.recipient, t.name, t.symbol, t.decimals, t.uri))

    return hash.keccak256(evm.encode("(string,bytes32,bytes32)", DOMAIN, head:bytes(), body:bytes())):bytes()
end

-- Text the registries keep: printable ASCII, cut to a length, or what to say instead.
local function tidy(text, most, otherwise)
    local kept = tostring(text or ""):gsub("[^%w %-%._]", ""):sub(1, most)

    return kept ~= "" and kept or otherwise
end

-- The wrapped token or collection here for an asset of another chain, cloned the first time.
local function wrapped_for(t)
    local key = t.origin .. ":" .. t.kind .. ":" .. hex(t.asset)
    local found = wrapped:get(key)

    if found then
        return address.parse(found)
    end

    local name = tidy("Wrapped " .. tidy(t.name, 56, "asset"), 64, "Wrapped asset")
    local symbol = tidy("w" .. tidy(t.symbol, 15, "X"), 16, "wX")
    local template = storage.get(t.kind == NFT and "nft" or t.kind == MULTI and "multi" or "token")
    local made

    if t.kind == NFT or t.kind == MULTI then
        made = contracts.clone(address.parse(template), key, name, symbol, "", ctx.contract)
    else
        made = contracts.clone(address.parse(template), key, name, symbol, t.decimals, ctx.contract)
    end

    wrapped:set(key, made:text())
    origins:set(made:text() .. ":key", key)
    origins:set(made:text() .. ":name", t.name)
    origins:set(made:text() .. ":symbol", t.symbol)
    origins:set(made:text() .. ":decimals", tostring(t.decimals))
    events.emit("Wrapped", u256(t.origin), t.kind, t.asset, made)

    return made
end

-- What arrives: released from what this gateway holds, when the asset is native here; minted as
-- its wrapped token or collection, when it is another chain's.
local function receive(t)
    local recipient = address.decode(t.recipient)

    if t.origin == here() then
        if t.kind == COIN then
            accounts.transfer(recipient, t.amount)
        elseif t.kind == TOKEN then
            contracts.call(address.decode(t.asset), "transfer", recipient, t.amount)
        elseif t.kind == NFT then
            contracts.call(address.decode(t.asset), "transfer", recipient, t.id)
        else
            contracts.call(address.decode(t.asset), "transfer", recipient, t.id, t.amount)
        end

        return
    end

    local made = wrapped_for(t)

    if t.kind == NFT then
        contracts.call(made, "mint", recipient, t.uri, t.id)
    elseif t.kind == MULTI then
        contracts.call(made, "mint", recipient, t.id, t.amount, t.uri)
    else
        contracts.call(made, "mint", recipient, t.amount)
    end
end

-- How many of the federation in force signed a digest.
local function signed(d)
    local fed = federation.get()
    local count = 0

    for _, key in ipairs(fed.signers) do
        if board:get(hex(d) .. ":" .. hex(key)) then
            count = count + 1
        end
    end

    return count, fed.threshold
end

-- A transfer from its fields, as attest and sent take and give them.
local function transfer(source, target, nonce, kind, origin, asset, id, amount, recipient, name, symbol, decimals, uri)
    local t = {
        source = whole(source, "the source"),
        target = whole(target, "the target"),
        nonce = whole(nonce, "the nonce"),
        kind = whole(kind, "the kind"),
        origin = whole(origin, "the origin"),
        asset = asset or "",
        id = u256(id or 0),
        amount = u256(amount or 0),
        recipient = recipient,
        name = name or "",
        symbol = symbol or "",
        decimals = whole(decimals or 0, "decimals"),
        uri = uri or "",
    }

    if t.kind > MULTI then
        error("kind is 0 the coin, 1 a token, 2 an NFT or 3 a multi-token amount")
    end

    if t.target == here() then
        t.from, t.to = peers:get(tostring(t.source)), ctx.contract:encode()
    elseif t.source == here() then
        t.from, t.to = ctx.contract:encode(), peers:get(tostring(t.target))
    else
        error("a transfer to or from another pair of chains")
    end

    if t.from == nil or t.to == nil then
        error("no gateway on that chain is this one's peer")
    end

    return t
end

-- Run once by the chain: whoever deploys owns the gateway.
function M.init()
    storage.set("owner", ctx.sender:text())
end

-- This chain's id, and the templates wrapped assets are cloned from: a token
-- (contracts/lua/wrapped.lua), an NFT collection and a multi-token one; set once.
function M.setup(chain_id, token, nft, multi)
    owner_only()

    if storage.get("token") then
        error("the gateway is set up already")
    end

    storage.set("chain_id", tostring(whole(chain_id, "the chain id")))
    storage.set("token", token:text())
    storage.set("nft", nft:text())
    storage.set("multi", multi:text())
    return true
end

-- The gateway on another chain, by its chain id: its address there, 20 bytes for an EVM chain.
function M.set_peer(chain_id, gateway)
    owner_only()
    peers:set(tostring(whole(chain_id, "the chain id")), gateway)
    return true
end

function M.peer(chain_id)
    return peers:get(tostring(whole(chain_id, "the chain id")))
end

-- Sends something to recipient on another chain: the coin the call carries; a token, NFT or
-- multi-token amount, which this gateway takes with transfer_from, so approve it first. A wrapped
-- one goes back, burned here; a native one is held here until it comes back.
function M.send(target, recipient, kind, asset, id, amount)
    kind = whole(kind, "the kind")
    id, amount = u256(id or 0), u256(amount or 0)
    local origin, asset_bytes, name, symbol, decimals, uri = here(), "", "", "", 0, ""

    if kind == COIN then
        if ctx.value == ZERO or ctx.value ~= amount then
            error("send the coin with the call, as much as amount says")
        end

        name, symbol, decimals = storage.get("coin_name") or "", storage.get("coin_symbol") or "", 18
    else
        if ctx.value ~= ZERO then
            error("this sends a token or an NFT, not coins: send none")
        end

        if asset == nil then
            error("name the token or collection to send")
        end

        if kind == NFT then
            amount = u256(1)
        end

        local key = origins:get(asset:text() .. ":key")

        if key then
            -- Wrapped here: it goes back, and what it stands for is what the message names: a
            -- wrapped coin goes back as the coin.
            local chain, original_kind, asset_hex = key:match("^(%d+):(%d+):(%x*)$")
            origin = tonumber(chain)
            asset_bytes = (asset_hex:gsub("..", function(pair)
                return string.char(tonumber(pair, 16))
            end))
            name, symbol = origins:get(asset:text() .. ":name"), origins:get(asset:text() .. ":symbol")
            decimals = tonumber(origins:get(asset:text() .. ":decimals"))

            kind = tonumber(original_kind)

            if kind == NFT then
                amount = u256(1)
            end

            if kind <= TOKEN then
                contracts.call(asset, "burn", ctx.sender, amount)
            elseif kind == NFT then
                uri = contracts.call(asset, "token_uri", id)
                contracts.call(asset, "transfer_from", ctx.sender, ctx.contract, id)
                contracts.call(asset, "burn", id)
            else
                uri = contracts.call(asset, "token_uri", id)
                contracts.call(asset, "transfer_from", ctx.sender, ctx.contract, id, amount)
                contracts.call(asset, "burn", ctx.contract, id, amount)
            end
        else
            -- Native here: held until it comes back.
            asset_bytes = asset:encode()
            local ok, answer = pcall(contracts.call, asset, "name")
            name = ok and type(answer) == "string" and answer or ""
            ok, answer = pcall(contracts.call, asset, "symbol")
            symbol = ok and type(answer) == "string" and answer or ""

            if kind == TOKEN then
                ok, answer = pcall(contracts.call, asset, "decimals")
                decimals = ok and tonumber(tostring(answer)) or 0
                contracts.call(asset, "transfer_from", ctx.sender, ctx.contract, amount)
            elseif kind == NFT then
                uri = contracts.call(asset, "token_uri", id)
                contracts.call(asset, "transfer_from", ctx.sender, ctx.contract, id)
            else
                uri = contracts.call(asset, "token_uri", id)
                contracts.call(asset, "transfer_from", ctx.sender, ctx.contract, id, amount)
            end
        end
    end

    if amount == ZERO then
        error("send something")
    end

    local nonce = whole(storage.get("nonce") or 0, "the nonce") + 1
    storage.set("nonce", tostring(nonce))
    local t = transfer(here(), target, nonce, kind, origin, asset_bytes, id, amount, recipient, name, symbol, decimals, uri)
    local d = digest(t)

    for field, value in pairs({ target = t.target, kind = kind, origin = origin, asset = asset_bytes, id = tostring(id), amount = tostring(amount), recipient = recipient, name = name, symbol = symbol, decimals = decimals, uri = uri, digest = d, from = ctx.sender:text() }) do
        out:set(nonce .. ":" .. field, tostring(value))
    end

    events.emit("Sent", nonce, t.target, d, ctx.sender)
    return nonce
end

-- How many transfers left through this gateway.
function M.sent_count()
    return whole(storage.get("nonce") or 0, "the nonce")
end

-- A transfer that left: its target, kind, origin, asset, id, amount, recipient, name, symbol,
-- decimals, URI and digest, for signers and relayers; and who sent it, for wallets.
function M.sent(nonce)
    local at = whole(nonce, "the nonce") .. ":"

    if out:get(at .. "digest") == nil then
        return nil
    end

    local field = function(name)
        return out:get(at .. name)
    end

    return tonumber(field("target")), tonumber(field("kind")), tonumber(field("origin")), field("asset"), u256(field("id")), u256(field("amount")),
        field("recipient"), field("name"), field("symbol"), tonumber(field("decimals")), field("uri"), field("digest"), address.parse(field("from"))
end

-- A signer of the federation in force posts its signature over a transfer: one coming here, which
-- is done once enough signed, or one that left, for a relayer to take to the other chain. Anyone
-- may send it; the signature says who signed.
function M.attest(source, target, nonce, kind, origin, asset, id, amount, recipient, name, symbol, decimals, uri, signature)
    local t = transfer(source, target, nonce, kind, origin, asset, id, amount, recipient, name, symbol, decimals, uri)
    local d = digest(t)

    if t.source == here() and out:get(t.nonce .. ":digest") ~= d then
        error("no such transfer left this gateway")
    end

    local fed = federation.get()

    if fed == nil then
        error("this chain has no federation")
    end

    local key = crypto.recover(d, signature)
    local known = false

    for _, signer in ipairs(fed.signers) do
        known = known or signer == key
    end

    if not known then
        error("not signed by a signer of the federation in force")
    end

    board:set(hex(d) .. ":" .. hex(key), signature)
    events.emit("Attested", d, key)

    local at = t.source .. ":" .. t.nonce

    if t.target == here() and done:get(at) == nil then
        local count, threshold = signed(d)

        if count >= threshold then
            done:set(at, hex(d))
            receive(t)
            events.emit("Received", t.source, t.nonce, d)
        end
    end

    return true
end

-- The signatures the federation in force posted over a digest, in the signers' order, and how
-- many it takes: what a relayer hands the other chain's gateway.
function M.signatures(d)
    local fed = federation.get()
    local found = {}

    for _, key in ipairs(fed.signers) do
        local signature = board:get(hex(d) .. ":" .. hex(key))

        if signature then
            table.insert(found, signature)
        end
    end

    return fed.threshold, table.unpack(found)
end

-- Whether a transfer from a chain arrived and was done.
function M.received(source, nonce)
    return done:get(whole(source, "the source") .. ":" .. whole(nonce, "the nonce")) ~= nil
end

-- What a wrapped asset here stands for: its origin chain, kind and address there; nil for any
-- other address.
function M.origin_of(asset)
    local key = origins:get(asset:text() .. ":key")

    if key == nil then
        return nil
    end

    local chain, kind, asset_hex = key:match("^(%d+):(%d+):(%x*)$")

    return tonumber(chain), tonumber(kind), (asset_hex:gsub("..", function(pair)
        return string.char(tonumber(pair, 16))
    end))
end

-- The wrapped asset here for an asset of another chain, if it was made.
function M.wrapped_of(origin, kind, asset)
    local found = wrapped:get(whole(origin, "the origin") .. ":" .. whole(kind, "the kind") .. ":" .. hex(asset or ""))

    return found and address.parse(found)
end

-- What the coin is called, for the message that carries it: set by the owner.
function M.set_coin(name, symbol)
    owner_only()
    storage.set("coin_name", name)
    storage.set("coin_symbol", symbol)
    return true
end

return M
