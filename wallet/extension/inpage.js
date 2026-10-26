// The page's provider: window.ethereum, as EIP-1193 has it, and announced by EIP-6963, which sites
// list wallets by. It runs in the page itself, where a site can reach it, and holds nothing: every
// request goes through content.js to the extension's background, which answers reads from the
// wallet's node and brings the rest to the wallet to approve. window.ethereum is set only when no
// other wallet set it first, so MetaMask beside it keeps working as before.
(() => {
    if (window.chainv4Provider) {
        return;
    }

    const TO_CONTENT = "chainv4:to-content";
    const TO_PAGE = "chainv4:to-page";
    const pending = new Map();
    const listeners = new Map();
    let next = 1;
    let chainId = null;
    let accounts = [];
    let connected = false;

    function emit(event, value) {
        for (const listener of [...(listeners.get(event) ?? [])]) {
            try {
                listener(value);
            } catch (err) {
                console.error(err);
            }
        }
    }

    // An EIP-1193 error: an Error with the code, as sites check it.
    function failure({ code, message, data }) {
        const err = new Error(message);
        err.code = code;

        if (data !== undefined) {
            err.data = data;
        }

        return err;
    }

    window.addEventListener("message", (event) => {
        if (event.source !== window || event.data?.target !== TO_PAGE) {
            return;
        }

        const { id, result, error, event: name, value } = event.data;

        if (name) {
            if (name === "accountsChanged") {
                accounts = value;
                provider.selectedAddress = value[0] ?? null;
            }

            if (name === "chainChanged") {
                chainId = value;
                provider.chainId = value;
            }

            emit(name, value);

            return;
        }

        const waiting = pending.get(id);

        if (waiting) {
            pending.delete(id);
            error ? waiting.reject(failure(error)) : waiting.resolve(result);
        }
    });

    function ask(method, params) {
        return new Promise((resolve, reject) => {
            const id = next++;
            pending.set(id, { resolve, reject });
            window.postMessage({ target: TO_CONTENT, id, method, params }, window.location.origin === "null" ? "*" : window.location.origin);
        });
    }

    async function request(args) {
        if (!args || typeof args !== "object" || typeof args.method !== "string") {
            throw failure({ code: -32602, message: "request takes { method, params }" });
        }

        const params = args.params === undefined ? [] : args.params;
        const result = await ask(args.method, params);

        if (args.method === "eth_chainId" && result !== chainId) {
            chainId = result;
            provider.chainId = result;

            if (!connected) {
                connected = true;
                emit("connect", { chainId: result });
            }
        }

        if ((args.method === "eth_requestAccounts" || args.method === "eth_accounts") && Array.isArray(result)) {
            accounts = result;
            provider.selectedAddress = result[0] ?? null;
        }

        return result;
    }

    const provider = {
        isChainv4: true,
        chainId: null,
        selectedAddress: null,
        request,
        isConnected: () => connected,
        on(event, listener) {
            listeners.set(event, [...(listeners.get(event) ?? []), listener]);

            return provider;
        },
        once(event, listener) {
            const once = (value) => {
                provider.removeListener(event, once);
                listener(value);
            };

            return provider.on(event, once);
        },
        removeListener(event, listener) {
            listeners.set(event, (listeners.get(event) ?? []).filter((other) => other !== listener));

            return provider;
        },
        off(event, listener) {
            return provider.removeListener(event, listener);
        },
        removeAllListeners(event) {
            event === undefined ? listeners.clear() : listeners.delete(event);

            return provider;
        },
        // What sites from before EIP-1193 call.
        enable: () => request({ method: "eth_requestAccounts" }),
        send(method, params) {
            if (typeof method === "string") {
                return request({ method, params });
            }

            if (method?.method === "eth_accounts") {
                return { id: method.id, jsonrpc: "2.0", result: accounts };
            }

            throw failure({ code: 4200, message: "send takes a method and its params; use request" });
        },
        sendAsync(payload, callback) {
            request(payload).then(
                (result) => callback(null, { id: payload.id, jsonrpc: "2.0", result }),
                (error) => callback(error, { id: payload.id, jsonrpc: "2.0", error }),
            );
        },
    };

    Object.freeze(provider.request);
    window.chainv4Provider = provider;

    if (window.ethereum === undefined) {
        window.ethereum = provider;
        window.dispatchEvent(new Event("ethereum#initialized"));
    }

    // EIP-6963: announced now, and again whenever a site asks.
    const info = Object.freeze({
        uuid: crypto.randomUUID(),
        name: "chainv4 wallet",
        rdns: "org.chainv4.wallet",
        icon: "data:image/svg+xml;base64," + btoa(
            '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"><rect width="64" height="64" rx="14" fill="#16181d"/>' +
            '<path d="M20 22h24M20 32h24M20 42h16" stroke="#d6a94f" stroke-width="6" stroke-linecap="round"/></svg>',
        ),
    });
    const announce = () => window.dispatchEvent(new CustomEvent("eip6963:announceProvider", { detail: Object.freeze({ info, provider }) }));

    window.addEventListener("eip6963:requestProvider", announce);
    announce();
})();
