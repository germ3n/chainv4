// The toolbar button opens the wallet beside the page: Chrome's side panel, Firefox's sidebar. It
// stays open as you browse, so a prompt, a camera scan or MetaMask does not close it, as they would
// a popup.
//
// It is also where sites' requests meet the wallet. inpage.js gives a site window.ethereum, and
// content.js brings what it asks here over a port. Reads go to the wallet's node, at POST /rpc,
// whether the wallet is open or not. Connecting, sending and signing wait for the wallet: any page
// of it that is open and unlocked, or else a small window of it opened for the purpose, which
// closes once nothing is left to approve. Which accounts each site may see is kept here, in the
// extension's storage; the keys never leave the wallet.
const api = globalThis.browser ?? globalThis.chrome;

if (api.sidePanel) {
    api.sidePanel.setPanelBehavior({ openPanelOnActionClick: true }).catch((err) => console.error(err));
} else if (api.sidebarAction) {
    api.action.onClicked.addListener(() => api.sidebarAction.toggle());
}

// Every site's pages, for http and https alike.
const SITES = ["http://*/*", "https://*/*"];
const SCRIPTS = [
    { id: "chainv4-inpage", js: ["inpage.js"], matches: SITES, runAt: "document_start", world: "MAIN", allFrames: false },
    { id: "chainv4-content", js: ["content.js"], matches: SITES, runAt: "document_start", allFrames: false },
];

// Asked of the wallet, which shows each to approve.
const APPROVED = new Set(["eth_requestAccounts", "wallet_requestPermissions", "eth_sendTransaction", "personal_sign", "eth_signTypedData_v4"]);
// Never: they sign what a person cannot read, or signed data no wallet should hand a site.
const REFUSED = new Set(["eth_sign", "eth_signTransaction", "eth_signTypedData", "eth_signTypedData_v1", "eth_signTypedData_v3", "eth_getEncryptionPublicKey", "eth_decrypt"]);

const error = (code, message) => ({ code, message });
const UNAUTHORIZED = error(4100, "Connect the site to the chainv4 wallet first.");
const REJECTED = error(4001, "User rejected the request.");

// What the wallet last said: its node, chain id, Ethereum accounts and whether it is unlocked.
async function walletState() {
    const { wallet } = await api.storage.local.get("wallet");

    return wallet ?? { node: "", chainId: null, accounts: [], unlocked: false };
}

async function sites() {
    const { sites } = await api.storage.local.get("sites");

    return sites ?? {};
}

// The accounts a site may see: those it was given that the wallet still has.
async function accountsOf(origin) {
    const [wallet, all] = [await walletState(), await sites()];
    const given = all[origin]?.accounts ?? [];

    return wallet.accounts.length === 0 ? given : given.filter((account) => wallet.accounts.includes(account));
}

// Pages of sites, by port, each with its origin.
const pages = new Map();
// Pages of the wallet, by port, each with what it last said.
const wallets = new Map();
// Requests waiting for the wallet, by id.
const waiting = new Map();
let nextId = 1;
let approvalWindow = null;

function broadcast(event, value, origin) {
    for (const [port, page] of pages) {
        if (origin === undefined || page.origin === origin) {
            port.postMessage({ event, value });
        }
    }
}

async function rpc(method, params) {
    const wallet = await walletState();

    if (!wallet.node) {
        throw error(4900, "Open the chainv4 wallet once, so it can say which node it uses.");
    }

    let response;

    try {
        response = await fetch(`${wallet.node}/rpc`, {
            method: "POST",
            headers: { "content-type": "application/json" },
            body: JSON.stringify({ jsonrpc: "2.0", id: 1, method, params }),
        });
    } catch (err) {
        throw error(4900, `The wallet's node did not answer: ${err.message}. Is the wallet let reach it?`);
    }

    const answer = await response.json().catch(() => null);

    if (!answer) {
        throw error(-32603, `The node answered ${response.status} ${response.statusText}.`);
    }

    if (answer.error) {
        throw answer.error;
    }

    return answer.result;
}

async function chainId() {
    const wallet = await walletState();

    return wallet.chainId ?? (await rpc("eth_chainId", []));
}

// Hands a request to the wallet: every page of it open now, and a window of it when none is
// unlocked. The first to answer settles it.
function approve(origin, method, params) {
    return new Promise((resolve, reject) => {
        const id = nextId++;
        waiting.set(id, { id, origin, method, params, resolve, reject });
        deliver();
    });
}

async function deliver() {
    const requests = [...waiting.values()].map(({ id, origin, method, params }) => ({ id, origin, method, params }));

    for (const port of wallets.keys()) {
        port.postMessage({ type: "requests", requests });
    }

    const unlocked = [...wallets.values()].some((wallet) => wallet.unlocked);

    if (requests.length > 0 && !unlocked && approvalWindow === null) {
        approvalWindow = -1;

        try {
            const opened = await api.windows.create({ url: api.runtime.getURL("index.html?approve"), type: "popup", width: 420, height: 720 });
            approvalWindow = opened.id;
        } catch (err) {
            approvalWindow = null;
            console.error(err);
        }
    }
}

api.windows?.onRemoved.addListener((closed) => {
    if (closed !== approvalWindow) {
        return;
    }

    approvalWindow = null;

    // Closed with nothing else of the wallet open: what it held is rejected.
    if (![...wallets.values()].some((wallet) => wallet.unlocked)) {
        for (const request of waiting.values()) {
            request.reject(REJECTED);
        }

        waiting.clear();
    }
});

async function settle(id, result, failed) {
    const request = waiting.get(id);

    if (!request) {
        return;
    }

    waiting.delete(id);

    if (failed) {
        request.reject(failed);
    } else {
        request.resolve(await after(request, result));
    }

    for (const port of wallets.keys()) {
        port.postMessage({ type: "settled", id });
    }

    deliver();
}

// What an approval does here before the site hears: a connection kept, with the accounts given.
async function after(request, result) {
    if (request.method !== "eth_requestAccounts" && request.method !== "wallet_requestPermissions") {
        return result;
    }

    const all = await sites();
    all[request.origin] = { accounts: result, since: Date.now() };
    await api.storage.local.set({ sites: all });
    broadcast("accountsChanged", result, request.origin);

    return request.method === "wallet_requestPermissions" ? permissions(result) : result;
}

function permissions(accounts) {
    return accounts.length === 0 ? [] : [{ parentCapability: "eth_accounts", caveats: [{ type: "restrictReturnedAccounts", value: accounts }] }];
}

// The account a signing request names, and whether the site may use it.
function signer(method, params) {
    switch (method) {
        case "eth_sendTransaction":
            return params?.[0]?.from;
        case "personal_sign":
            // Sites send [message, address]; a few, [address, message].
            return [params?.[1], params?.[0]].find((value) => /^0x[0-9a-fA-F]{40}$/.test(value ?? ""));
        case "eth_signTypedData_v4":
            return params?.[0];
        default:
            return undefined;
    }
}

async function answer(origin, method, params) {
    if (REFUSED.has(method)) {
        throw error(4200, `${method} is not supported: it signs what cannot be read. Use personal_sign or eth_signTypedData_v4.`);
    }

    switch (method) {
        case "eth_chainId":
            return chainId();
        case "net_version":
            return String(BigInt(await chainId()));
        case "eth_accounts":
            return accountsOf(origin);
        case "eth_coinbase":
            return (await accountsOf(origin))[0] ?? null;
        case "wallet_getPermissions":
            return permissions(await accountsOf(origin));
        case "wallet_revokePermissions": {
            const all = await sites();
            delete all[origin];
            await api.storage.local.set({ sites: all });
            broadcast("accountsChanged", [], origin);

            return null;
        }
        case "wallet_switchEthereumChain":
        case "wallet_addEthereumChain": {
            const wanted = String(params?.[0]?.chainId ?? "").toLowerCase();
            const current = String(await chainId()).toLowerCase();

            if (wanted !== "" && BigInt(wanted) === BigInt(current)) {
                return null;
            }

            throw error(4902, `The chainv4 wallet follows the chain of its node, ${current}, alone; switch its node under Settings.`);
        }
        case "wallet_watchAsset":
            throw error(4200, "Tokens show in the chainv4 wallet as the chain's token registry lists them.");
    }

    if (method === "eth_requestAccounts" || method === "wallet_requestPermissions") {
        const given = await accountsOf(origin);

        return given.length > 0 && method === "eth_requestAccounts" ? given : approve(origin, method, params);
    }

    if (APPROVED.has(method)) {
        const account = String(signer(method, params) ?? "").toLowerCase();

        if (!(await accountsOf(origin)).includes(account)) {
            throw UNAUTHORIZED;
        }

        return approve(origin, method, params);
    }

    if (/^(eth|net|web3)_/.test(method)) {
        return rpc(method, params);
    }

    throw error(4200, `${method} is not supported.`);
}

api.runtime.onConnect.addListener((port) => {
    if (port.name === "chainv4-provider") {
        // The page's origin, as the browser knows it.
        const origin = port.sender?.origin ?? new URL(port.sender?.url ?? "about:blank").origin;
        pages.set(port, { origin });
        port.onDisconnect.addListener(() => pages.delete(port));
        port.onMessage.addListener(async ({ id, method, params }) => {
            try {
                port.postMessage({ id, result: await answer(origin, method, params) });
            } catch (failed) {
                port.postMessage({ id, error: failed?.code ? failed : error(-32603, String(failed?.message ?? failed)) });
            }
        });
    }

    // Only the extension's own pages are the wallet.
    if (port.name === "chainv4-wallet" && port.sender?.url?.startsWith(api.runtime.getURL(""))) {
        wallets.set(port, { unlocked: false });
        port.onDisconnect.addListener(() => {
            wallets.delete(port);
            deliver();
        });
        port.onMessage.addListener(async (message) => {
            if (message.type === "state") {
                const before = await walletState();
                wallets.set(port, { unlocked: message.unlocked });
                const state = { node: message.node, chainId: message.chainId ?? before.chainId, accounts: message.accounts ?? before.accounts, unlocked: message.unlocked };
                await api.storage.local.set({ wallet: state });

                if (state.chainId && state.chainId !== before.chainId) {
                    broadcast("chainChanged", state.chainId);
                }

                if (message.accounts && JSON.stringify(message.accounts) !== JSON.stringify(before.accounts)) {
                    for (const page of new Set([...pages.values()].map((page) => page.origin))) {
                        broadcast("accountsChanged", await accountsOf(page), page);
                    }
                }

                deliver();
            }

            if (message.type === "answer") {
                settle(message.id, message.result, message.error);
            }

            if (message.type === "sites") {
                await sitesSwitch(message.on);
            }

            if (message.type === "connected") {
                port.postMessage({ type: "connected", sites: await sites() });
            }

            if (message.type === "disconnect") {
                const all = await sites();
                delete all[message.origin];
                await api.storage.local.set({ sites: all });
                broadcast("accountsChanged", [], message.origin);
                port.postMessage({ type: "connected", sites: all });
            }
        });
        port.postMessage({ type: "requests", requests: [...waiting.values()].map(({ id, origin, method, params }) => ({ id, origin, method, params })) });
    }
});

// Sites get window.ethereum once the wallet's Settings let them, and the browser gave leave to
// every site, from the wallet's own click; turned off, the scripts go and so does that leave.
async function sitesSwitch(on) {
    const registered = (await api.scripting.getRegisteredContentScripts()).map((script) => script.id);
    const ids = SCRIPTS.map((script) => script.id).filter((id) => registered.includes(id));

    if (ids.length > 0) {
        await api.scripting.unregisterContentScripts({ ids });
    }

    if (on && (await api.permissions.contains({ origins: SITES }))) {
        await api.scripting.registerContentScripts(SCRIPTS);
    }

    if (!on) {
        await api.permissions.remove({ origins: SITES }).catch(() => {});
    }

    await api.storage.local.set({ sitesOn: on });
}

// As the browser starts: the scripts back, if they were on and the browser keeps the leave.
api.runtime.onStartup?.addListener(async () => {
    const { sitesOn } = await api.storage.local.get("sitesOn");

    if (sitesOn) {
        await sitesSwitch(true).catch((err) => console.error(err));
    }
});
