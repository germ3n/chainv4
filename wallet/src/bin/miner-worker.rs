//! The browser wallet's Web Worker for mining on the CPU: the page starts one per thread it mines
//! on, and hands each a chunk of nonces at a time. A worker hashes its chunk as the node checks a
//! nonce, posts back what it tried and found, and waits for the next; a chunk takes about a tenth
//! of a second, so new work reaches it soon. trunk builds it beside the wallet, and
//! miner-worker-boot.js starts it; natively it does nothing.

// The page's half, the job and the status, is the wallet's; a worker hashes chunks alone.
#[allow(dead_code)]
#[path = "../miner/web/work.rs"]
mod work;

/// Hashes each chunk posted to this worker and posts back what it found, once it said it is ready.
#[cfg(target_arch = "wasm32")]
fn main() {
    use wasm_bindgen::{JsCast, closure::Closure};

    let scope: web_sys::DedicatedWorkerGlobalScope = js_sys::global().unchecked_into();
    let answer = scope.clone();
    let hash = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |event: web_sys::MessageEvent| {
        let Some(chunk) = event.data().as_string().and_then(|text| work::Chunk::read(&text)) else {
            return;
        };

        let _ = answer.post_message(&chunk.search().to_json().into());
    });
    scope.set_onmessage(Some(hash.as_ref().unchecked_ref()));
    // The worker hashes until the page ends it.
    hash.forget();
    let _ = scope.post_message(&work::READY.into());
}

/// Natively there are no Web Workers: the wallet mines on threads of its own.
#[cfg(not(target_arch = "wasm32"))]
fn main() {}
