// The extension's page, between the wallet and the background: what sites ask comes in here for the
// wallet to show, and the wallet's answers go back. The wallet reads and writes window.chainv4Dapp
// each frame, as it says its node through window.chainv4Node. Also where the wallet's Settings let
// sites in: the browser gives leave to every site only from a click, so a bar above the wallet asks
// for it, as access.js does for the node.
(() => {
    const api = globalThis.browser ?? globalThis.chrome;
    const SITES = ["http://*/*", "https://*/*"];
    // Opened by the background to approve something: it closes once nothing is left.
    const approving = new URLSearchParams(window.location.search).has("approve");
    let port = null;
    let requests = [];
    // Whether sites are in, as the background keeps it.
    let on = false;
    let sent = "";
    let lastSent = 0;
    let connected = {};
    let answered = false;
    let wanted = false;

    function connect() {
        port = api.runtime.connect({ name: "chainv4-wallet" });
        port.onMessage.addListener((message) => {
            if (message.type === "requests") {
                requests = message.requests;

                if (approving && answered && requests.length === 0) {
                    window.close();
                }
            }

            if (message.type === "settled") {
                requests = requests.filter((request) => request.id !== message.id);
            }

            if (message.type === "connected") {
                connected = message.sites;
            }
        });
        port.onDisconnect.addListener(() => {
            port = null;
            setTimeout(connect, 500);
        });
        sent = "";
        port.postMessage({ type: "connected" });
    }

    function post(message) {
        if (port === null) {
            connect();
        }

        port.postMessage(message);
    }

    // The bar that asks for every site, shown while the wallet wants sites in and lacks the leave.
    const style = document.createElement("style");
    style.textContent = `
        #sites { order: -1; display: flex; gap: 10px; align-items: center; padding: 10px 14px;
            background: #1b1d22; color: #e8e2d6; font: 14px system-ui, sans-serif; border-bottom: 1px solid #2c2f36; }
        #sites[hidden] { display: none; }
        #sites span { flex: 1; }
        #sites button { background: #d6a94f; color: #0c0d10; border: 0; border-radius: 6px; padding: 6px 12px;
            font: 600 13px system-ui, sans-serif; cursor: pointer; }
        #sites button.quiet { background: transparent; color: #e8e2d6; border: 1px solid #2c2f36; }`;
    document.head.append(style);
    const bar = document.createElement("div");
    bar.id = "sites";
    bar.hidden = true;
    const text = document.createElement("span");
    text.textContent = "Let sites ask to connect to the wallet: the browser asks once, for every site.";
    const allow = document.createElement("button");
    allow.textContent = "Allow";
    const cancel = document.createElement("button");
    cancel.textContent = "Not now";
    cancel.className = "quiet";
    bar.append(text, allow, cancel);
    document.body.append(bar);

    // The request comes first in the click, before anything awaits, as Firefox wants.
    allow.addEventListener("click", () => {
        api.permissions.request({ origins: SITES }).then((granted) => {
            wanted = false;
            bar.hidden = true;

            if (granted) {
                post({ type: "sites", on: true });
            }
        }, (err) => console.error(err));
    });
    cancel.addEventListener("click", () => {
        wanted = false;
        bar.hidden = true;
    });

    window.chainv4Dapp = {
        // What waits for the wallet, as JSON: [{ id, origin, method, params }].
        pending: () => JSON.stringify(requests),
        // The wallet's answer: { result } or { error: { code, message } }, as JSON.
        answer(id, json) {
            const { result, error } = JSON.parse(json);
            answered = true;
            requests = requests.filter((request) => request.id !== id);
            post({ type: "answer", id, result, error });
        },
        // What the wallet is: { node, chainId, accounts, unlocked }, as JSON. Sent on when it
        // changes, and every few seconds anyway, which keeps Chrome's background awake while
        // something waits.
        report(json) {
            const now = Date.now();

            if (json !== sent || now - lastSent > 3000) {
                sent = json;
                lastSent = now;
                post({ type: "state", ...JSON.parse(json) });
            }
        },
        // Whether sites may ask, and which are connected, as JSON.
        sites: () => JSON.stringify({ on, wanted, connected }),
        // Sites in: the bar asks for the leave. Sites out: at once.
        letSites(on) {
            if (on) {
                wanted = true;
                bar.hidden = false;
            } else {
                wanted = false;
                bar.hidden = true;
                post({ type: "sites", on: false });
            }

            post({ type: "connected" });
        },
        disconnect(origin) {
            post({ type: "disconnect", origin });
        },
        refresh() {
            post({ type: "connected" });
        },
        approving,
    };

    api.storage.local.get("sitesOn").then(({ sitesOn }) => {
        on = Boolean(sitesOn);
    });
    api.storage.onChanged.addListener((changes) => {
        if (changes.sitesOn) {
            on = Boolean(changes.sitesOn.newValue);
        }
    });

    connect();
})();
