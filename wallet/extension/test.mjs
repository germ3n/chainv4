// The extension's scripts, run against a browser of mocks: a site's page asking through inpage.js
// and content.js, the background answering reads from a node and bringing the rest to a wallet
// page, and the wallet answering. Run with node wallet/extension/test.mjs.
import { readFileSync } from "node:fs";
import test from "node:test";
import assert from "node:assert/strict";
import vm from "node:vm";

const here = new URL(".", import.meta.url);
const source = (name) => readFileSync(new URL(name, here), "utf8");
const settle = () => new Promise((resolve) => setTimeout(resolve, 5));

/// A port, as the browser gives one to each side: what one side posts, the other's listeners hear.
function pair(name, sender) {
    const ends = [0, 1].map(() => ({ heard: [], listeners: [], gone: [] }));
    const make = (me, other) => ({
        name,
        sender,
        postMessage: (message) => {
            const copy = structuredClone(message);
            me.heard.push(copy);
            queueMicrotask(() => other.listeners.forEach((listener) => listener(copy)));
        },
        onMessage: { addListener: (listener) => me.listeners.push(listener) },
        onDisconnect: { addListener: (listener) => me.gone.push(listener) },
        disconnect: () => other.gone.forEach((listener) => listener()),
    });

    // What the near side heard: everything the far side posted.
    return { near: make(ends[0], ends[1]), far: make(ends[1], ends[0]), inbox: ends[1].heard };
}

/// The background, with a browser of mocks: storage, windows, scripts, permissions and a node.
function background({ node = "http://node.test", chainId = "0x22cc", accounts = ["0x" + "ab".repeat(20)] } = {}) {
    const stored = { wallet: { node, chainId, accounts, unlocked: false } };
    const browser = {
        connect: [],
        windows: [],
        removed: [],
        registered: [],
        granted: true,
        asked: [],
    };
    const chrome = {
        runtime: { onConnect: { addListener: (listener) => browser.connect.push(listener) }, onStartup: { addListener() {} }, getURL: (path) => `chrome-extension://wallet/${path}` },
        sidePanel: { setPanelBehavior: async () => {} },
        storage: {
            local: {
                get: async (key) => ({ [key]: structuredClone(stored[key]) }),
                set: async (values) => Object.assign(stored, structuredClone(values)),
            },
        },
        windows: {
            create: async (options) => (browser.windows.push(options), { id: 41 }),
            onRemoved: { addListener: (listener) => browser.removed.push(listener) },
        },
        scripting: {
            getRegisteredContentScripts: async () => browser.registered.map((id) => ({ id })),
            registerContentScripts: async (scripts) => browser.registered.push(...scripts.map((script) => script.id)),
            unregisterContentScripts: async ({ ids }) => (browser.registered = browser.registered.filter((id) => !ids.includes(id))),
        },
        permissions: { contains: async () => browser.granted, remove: async () => (browser.granted = false) },
    };
    const fetch = async (url, init) => {
        const body = JSON.parse(init.body);
        browser.asked.push({ url, ...body });

        return { status: 200, statusText: "OK", json: async () => ({ jsonrpc: "2.0", id: body.id, result: body.method === "eth_blockNumber" ? "0x10" : null }) };
    };
    vm.runInNewContext(source("background.js"), { chrome, fetch, console, URL, structuredClone, setTimeout, BigInt, Date, JSON });

    const connect = (name, sender) => {
        const port = pair(name, sender);
        browser.connect.forEach((listener) => listener(port.far));

        return port;
    };

    return { stored, browser, connect };
}

/// A site's page: inpage.js and content.js on one window, talking to the background through a port.
function site(bg, origin = "https://app.example") {
    const window = new EventTarget();
    window.location = { origin };
    let page;
    window.postMessage = (data) => queueMicrotask(() => {
        const event = new Event("message");
        event.data = structuredClone(data);
        event.source = window;
        window.dispatchEvent(event);
    });
    const chrome = { runtime: { connect: ({ name }) => (page = bg.connect(name, { origin, url: `${origin}/` }), page.near) } };
    const context = { window, chrome, console, crypto, btoa, Event, CustomEvent, structuredClone, queueMicrotask, Promise, Error, Object };
    window.addEventListener = window.addEventListener.bind(window);
    vm.runInNewContext(source("content.js"), context);
    vm.runInNewContext(source("inpage.js"), context);

    return { window, provider: window.ethereum, port: () => page };
}

/// The wallet's page, unlocked or not, answering what it is handed.
function wallet(bg, unlocked = true) {
    const port = bg.connect("chainv4-wallet", { url: "chrome-extension://wallet/index.html" });
    const requests = () => port.inbox.filter((message) => message.type === "requests").at(-1)?.requests ?? [];
    port.near.postMessage({ type: "state", node: "http://node.test", chainId: "0x22cc", accounts: ["0x" + "ab".repeat(20)], unlocked });

    return {
        requests,
        answer: (id, result, error) => port.near.postMessage({ type: "answer", id, result, error }),
        port,
    };
}

test("reads go to the wallet's node, and the chain id is the wallet's", async () => {
    const bg = background();
    const { provider } = site(bg);

    assert.equal(await provider.request({ method: "eth_chainId" }), "0x22cc");
    assert.equal(await provider.request({ method: "net_version" }), String(0x22cc));
    assert.equal(await provider.request({ method: "eth_blockNumber" }), "0x10");
    assert.deepEqual(bg.browser.asked.map((asked) => [asked.url, asked.method]), [["http://node.test/rpc", "eth_blockNumber"]]);
    assert.deepEqual(await provider.request({ method: "eth_accounts" }), [], "nothing until it connects");
    assert.equal(provider.isConnected(), true);
});

test("a site connects through the wallet, and then signs only with the accounts it was given", async () => {
    const bg = background();
    const { provider } = site(bg);
    const open = wallet(bg);
    const events = [];
    provider.on("accountsChanged", (accounts) => events.push(accounts));
    const asked = provider.request({ method: "eth_requestAccounts" });
    await settle();

    const [request] = open.requests();
    assert.equal(request.method, "eth_requestAccounts");
    assert.equal(request.origin, "https://app.example", "the browser's word for the site, not the page's");
    assert.equal(bg.browser.windows.length, 0, "an unlocked wallet is open: no window");

    open.answer(request.id, ["0x" + "ab".repeat(20)]);
    assert.deepEqual(await asked, ["0x" + "ab".repeat(20)]);
    await settle();
    assert.deepEqual(events, [["0x" + "ab".repeat(20)]]);
    assert.deepEqual(await provider.request({ method: "eth_accounts" }), ["0x" + "ab".repeat(20)]);
    assert.deepEqual(await provider.request({ method: "eth_requestAccounts" }), ["0x" + "ab".repeat(20)], "asked again, it knows");
    assert.equal(provider.selectedAddress, "0x" + "ab".repeat(20));

    // A signature, approved in the wallet.
    const signing = provider.request({ method: "personal_sign", params: ["0x6869", "0x" + "AB".repeat(20)] });
    await settle();
    const sign = open.requests().find((request) => request.method === "personal_sign");
    open.answer(sign.id, "0x" + "11".repeat(65));
    assert.equal(await signing, "0x" + "11".repeat(65));

    // Another account, or another site, is refused before the wallet hears.
    await assert.rejects(provider.request({ method: "eth_sendTransaction", params: [{ from: "0x" + "cd".repeat(20), to: "0x" + "cd".repeat(20) }] }), { code: 4100 });
    const other = site(bg, "https://other.example");
    await assert.rejects(other.provider.request({ method: "personal_sign", params: ["0x6869", "0x" + "ab".repeat(20)] }), { code: 4100 });
    assert.deepEqual(await other.provider.request({ method: "eth_accounts" }), []);

    // Disconnected, it sees nothing.
    await provider.request({ method: "wallet_revokePermissions", params: [{ eth_accounts: {} }] });
    assert.deepEqual(await provider.request({ method: "eth_accounts" }), []);
});

test("with no wallet unlocked, a window opens, and closing it rejects what waits", async () => {
    const bg = background();
    const { provider } = site(bg);
    const asked = provider.request({ method: "eth_requestAccounts" });
    await settle();

    assert.equal(bg.browser.windows.length, 1);
    assert.match(bg.browser.windows[0].url, /index\.html\?approve$/);
    bg.browser.removed.forEach((listener) => listener(41));
    await assert.rejects(asked, { code: 4001 });
});

test("a rejection reaches the site with its code, and the rest is refused or kept to one chain", async () => {
    const bg = background();
    const { provider } = site(bg);
    const open = wallet(bg);
    const asked = provider.request({ method: "eth_requestAccounts" });
    await settle();
    open.answer(open.requests()[0].id, undefined, { code: 4001, message: "User rejected the request." });
    await assert.rejects(asked, { code: 4001 });

    await assert.rejects(provider.request({ method: "eth_sign", params: [] }), { code: 4200 });
    // A switch to the chain followed already is answered here; any other, and a token to show,
    // go to the wallet, which says what came of them.
    assert.equal(await provider.request({ method: "wallet_switchEthereumChain", params: [{ chainId: "0x22CC" }] }), null);
    const switching = provider.request({ method: "wallet_switchEthereumChain", params: [{ chainId: "0x1" }] });
    await settle();
    const asked_switch = open.requests().find((request) => request.method === "wallet_switchEthereumChain");
    assert.deepEqual(asked_switch.params, [{ chainId: "0x1" }]);
    open.answer(asked_switch.id, undefined, { code: 4902, message: "This wallet has not followed chain 1." });
    await assert.rejects(switching, { code: 4902 });
    const watching = provider.request({ method: "wallet_watchAsset", params: { type: "ERC20", options: { address: "0x" + "12".repeat(20), symbol: "TKN", decimals: 18 } } });
    await settle();
    const asked_watch = open.requests().find((request) => request.method === "wallet_watchAsset");
    open.answer(asked_watch.id, true);
    assert.equal(await watching, true);
    await assert.rejects(provider.request({ method: "wallet_nonsense" }), { code: 4200 });
    await assert.rejects(provider.request("not an object"), { code: -32602 });
});

test("a page that is not the extension's is no wallet", async () => {
    const bg = background();
    const fake = bg.connect("chainv4-wallet", { url: "https://evil.example/" });
    const { provider } = site(bg);
    provider.request({ method: "eth_requestAccounts" });
    await settle();

    assert.equal(fake.inbox.length, 0, "it hears nothing");
    assert.equal(bg.browser.windows.length, 1, "the real wallet's window opens instead");
});

test("sites get the provider only once the wallet lets them, and lose it when it stops", async () => {
    const bg = background();
    const open = wallet(bg);
    open.port.near.postMessage({ type: "sites", on: true });
    await settle();

    assert.deepEqual(bg.browser.registered, ["chainv4-inpage", "chainv4-content"]);
    assert.equal(bg.stored.sitesOn, true);

    open.port.near.postMessage({ type: "sites", on: false });
    await settle();

    assert.deepEqual(bg.browser.registered, []);
    assert.equal(bg.browser.granted, false, "the leave for every site goes too");
});

test("EIP-6963 announces the provider, and window.ethereum is left to a wallet that came first", async () => {
    const window = new EventTarget();
    window.location = { origin: "https://app.example" };
    window.postMessage = () => {};
    const theirs = { isMetaMask: true };
    window.ethereum = theirs;
    const announced = [];
    window.addEventListener("eip6963:announceProvider", (event) => announced.push(event.detail));
    vm.runInNewContext(source("inpage.js"), { window, console, crypto, btoa, Event, CustomEvent, Object, Promise, Error });

    assert.equal(window.ethereum, theirs);
    assert.equal(announced.length, 1);
    assert.equal(announced[0].info.rdns, "org.chainv4.wallet");
    assert.equal(announced[0].provider.isChainv4, true);

    window.dispatchEvent(new Event("eip6963:requestProvider"));
    assert.equal(announced.length, 2, "announced again when a site asks");
});

test("an opaque-origin page gets no wallet, so it cannot share a grant with another", async () => {
    const bg = background();
    // A sandboxed page's origin is "null"; the background must serve it no provider.
    const port = bg.connect("chainv4-provider", { origin: "null", url: "null/" });
    port.near.postMessage({ id: 1, method: "eth_chainId" });
    await settle();

    assert.equal(port.inbox.some((message) => message.id === 1), false, "a refused port is never answered");
});

test("a page cannot reach a private node through the extension until it connects", async () => {
    const bg = background({ node: "http://127.0.0.1:8545" });
    const { provider } = site(bg);

    // Unconnected, a read to a loopback node is refused: the extension is no way past the browser.
    await assert.rejects(provider.request({ method: "eth_blockNumber" }), { code: 4100 });

    // A wallet that keeps the loopback node, and the site connects.
    const port = bg.connect("chainv4-wallet", { url: "chrome-extension://wallet/index.html" });
    port.near.postMessage({ type: "state", node: "http://127.0.0.1:8545", chainId: "0x22cc", accounts: ["0x" + "ab".repeat(20)], unlocked: true });
    await settle();
    const asked = provider.request({ method: "eth_requestAccounts" });
    await settle();
    const requests = port.inbox.filter((message) => message.type === "requests").at(-1)?.requests ?? [];
    port.near.postMessage({ type: "answer", id: requests[0].id, result: ["0x" + "ab".repeat(20)] });
    await asked;

    assert.equal(await provider.request({ method: "eth_blockNumber" }), "0x10", "connected, the read goes through");
});

test("a site cannot pile up more than a few requests waiting", async () => {
    const bg = background();
    const { provider } = site(bg);
    const open = wallet(bg);
    const connected = provider.request({ method: "eth_requestAccounts" });
    await settle();
    open.answer(open.requests()[0].id, ["0x" + "ab".repeat(20)]);
    await connected;

    // The four that are taken stay pending (the wallet never answers them here); the extras reject
    // at once. Counting in a handler rather than awaiting all of them keeps the pending ones from
    // hanging the test.
    let rejected = 0;
    Array.from({ length: 6 }, () => provider.request({ method: "personal_sign", params: ["0x6869", "0x" + "ab".repeat(20)] }).then(() => {}, (err) => err.code === 4001 && rejected++));
    await settle();

    assert.ok(open.requests().filter((request) => request.method === "personal_sign").length <= 4, "at most four wait");
    assert.ok(rejected >= 2, "the rest are turned away");
});

test("a page that goes drops what it left waiting", async () => {
    const bg = background();
    const { provider, port } = site(bg);
    const open = wallet(bg);
    const asked = provider.request({ method: "eth_requestAccounts" }).catch((err) => err);
    await settle();

    assert.equal(open.requests().length, 1);

    // The page navigates away: its port drops, and the waiting request goes with it.
    port().near.disconnect();
    await settle();

    assert.equal(open.requests().length, 0, "the wallet no longer shows it");
    assert.equal((await asked).code, 4001);
});
