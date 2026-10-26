// Between a site's page and the extension: what inpage.js asks goes to the background over a port,
// and what comes back, answers and events, goes to the page. Which site asks, the background reads
// from the browser's own record of the port, never from what the page says.
(() => {
    const api = globalThis.browser ?? globalThis.chrome;
    const TO_CONTENT = "chainv4:to-content";
    const TO_PAGE = "chainv4:to-page";
    const origin = window.location.origin === "null" ? "*" : window.location.origin;
    let port = null;
    // Requests out on the port, so a port that drops fails them rather than leaving them waiting.
    const out = new Set();

    function toPage(message) {
        window.postMessage({ target: TO_PAGE, ...message }, origin);
    }

    function connect() {
        port = api.runtime.connect({ name: "chainv4-provider" });
        port.onMessage.addListener((message) => {
            out.delete(message.id);
            toPage(message);
        });
        port.onDisconnect.addListener(() => {
            port = null;

            for (const id of out) {
                toPage({ id, error: { code: 4900, message: "The wallet's extension restarted; ask again." } });
            }

            out.clear();
        });
    }

    window.addEventListener("message", (event) => {
        if (event.source !== window || event.data?.target !== TO_CONTENT) {
            return;
        }

        const { id, method, params } = event.data;

        try {
            if (port === null) {
                connect();
            }

            out.add(id);
            port.postMessage({ id, method, params });
        } catch {
            // The extension was reloaded or removed under the page.
            toPage({ id, error: { code: 4900, message: "The chainv4 wallet is not available; reload the page." } });
        }
    });
})();
