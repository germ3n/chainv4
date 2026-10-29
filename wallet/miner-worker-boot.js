// Starts the Web Worker that mines on the CPU: wasm-bindgen's script for it, then its wasm, both
// of which trunk writes beside the page, unhashed, as a worker's are named before it runs.
importScripts("./miner-worker.js");
wasm_bindgen({ module_or_path: "./miner-worker_bg.wasm" });
