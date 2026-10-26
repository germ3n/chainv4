// The toolbar button opens the wallet beside the page: Chrome's side panel, Firefox's sidebar. It
// stays open as you browse, so a prompt, a camera scan or MetaMask does not close it, as they would
// a popup.
const api = globalThis.browser ?? globalThis.chrome;

if (api.sidePanel) {
    api.sidePanel.setPanelBehavior({ openPanelOnActionClick: true }).catch((err) => console.error(err));
} else if (api.sidebarAction) {
    api.action.onClicked.addListener(() => api.sidebarAction.toggle());
}
