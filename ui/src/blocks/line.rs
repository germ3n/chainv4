//! The way a reading's requests go to the node: through the client the app talks to it by, so through
//! the proxy set and never around it, with answers read on the thread they arrive on.

use {
    super::{Asked, Came},
    crate::net::Client,
    std::sync::{Arc, Mutex, PoisonError},
};

/// What wakes the app when an answer arrives.
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// Requests for blocks, and what came of them.
pub struct RangeLine {
    base: String,
    client: Result<Client, String>,
    came: Arc<Mutex<Vec<(Asked, Came)>>>,
    wake: Wake,
}

impl RangeLine {
    /// A line to the node at this address, through this client, or none, for the reason given.
    pub fn new(base: &str, client: Result<Client, String>, wake: Wake) -> RangeLine {
        RangeLine { base: base.trim().trim_end_matches('/').to_string(), client, came: Arc::default(), wake }
    }

    /// Sends a request. What comes of it is in `came`.
    pub fn ask(&self, asked: Asked) {
        let (came, wake) = (self.came.clone(), self.wake.clone());
        let done = move |answer| {
            let read = Came::of(answer);

            came.lock().unwrap_or_else(PoisonError::into_inner).push((asked, read));
            wake();
        };

        match &self.client {
            Ok(client) => client.fetch(format!("{}{}", self.base, asked.path()), None, done),
            Err(why) => done(Err(format!("the proxy set does not work: {why}"))),
        }
    }

    /// What came of what was asked, since last time.
    pub fn came(&self) -> Vec<(Asked, Came)> {
        std::mem::take(&mut *self.came.lock().unwrap_or_else(PoisonError::into_inner))
    }
}
