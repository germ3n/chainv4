// The extension's page: asks the browser for leave to reach the node the wallet talks to. An
// extension reaches another site only with the host permission for it, which this one asks for as
// it is needed rather than at install, and from a click, as browsers require; until then the node's
// answers are blocked. The wallet says which node it talks to as window.chainv4Node.
(() => {
    const api = globalThis.browser ?? globalThis.chrome;
    const style = document.createElement("style");
    // Above the canvas, not over it, so the wallet's own top bar stays in sight.
    style.textContent = `
        body { display: flex; flex-direction: column; }
        canvas#wallet { flex: 1; min-height: 0; height: auto; }
        #access { order: -1; display: flex; gap: 10px; align-items: center; padding: 10px 14px;
            background: #1b1d22; color: #e8e2d6; font: 14px system-ui, sans-serif; border-bottom: 1px solid #2c2f36; }
        #access[hidden] { display: none; }
        #access span { flex: 1; }
        #access button { background: #d6a94f; color: #0c0d10; border: 0; border-radius: 6px; padding: 6px 12px;
            font: 600 13px system-ui, sans-serif; cursor: pointer; }`;
    document.head.append(style);

    const bar = document.createElement("div");
    bar.id = "access";
    bar.hidden = true;
    const text = document.createElement("span");
    const allow = document.createElement("button");
    allow.textContent = "Allow";
    bar.append(text, allow);
    document.body.append(bar);

    // What to ask for: the node's scheme and host, any port, as match patterns hold no port.
    let wanted = null;
    const pattern = (node) => {
        try {
            const url = new URL(node);

            return url.protocol === "http:" || url.protocol === "https:" ? `${url.protocol}//${url.hostname}/*` : null;
        } catch {
            return null;
        }
    };

    async function check() {
        const origin = pattern(globalThis.chainv4Node ?? "");
        const granted = origin === null || (await api.permissions.contains({ origins: [origin] }));
        wanted = granted ? null : origin;
        bar.hidden = granted;

        if (!granted) {
            text.textContent = `Let the wallet reach its node, ${new URL(globalThis.chainv4Node).host}`;
        }
    }

    // The request comes first in the click, before anything awaits, as Firefox wants.
    allow.addEventListener("click", () => {
        if (wanted !== null) {
            api.permissions.request({ origins: [wanted] }).then(check, (err) => console.error(err));
        }
    });

    setInterval(check, 1000);
    check();
})();
