//! How mining is going, for the node's report and the explorer: each device's hashrate, measured
//! over the last stretch of a few seconds, and the workers mining for the node.

use {
    r#macro::document,
    std::sync::Mutex,
    std::sync::atomic::{AtomicU64, Ordering},
    std::time::{Duration, Instant},
};

/// How long a hashrate is measured over before it is taken again.
const STRETCH: Duration = Duration::from_secs(3);

#[document(
    summary = "A hashrate meter: miners add the nonces they tried, from any thread, and the rate is what they tried over the last few seconds.",
    category = "Consensus",
    see_also = "MiningReport",
)]
#[derive(Debug)]
pub struct Meter {
    tried: AtomicU64,
    /// When the rate was last taken, the count then, and the rate.
    taken: Mutex<(Instant, u64, f64)>,
}

impl Default for Meter {
    fn default() -> Meter {
        Meter {
            tried: AtomicU64::new(0),
            taken: Mutex::new((Instant::now(), 0, 0.0)),
        }
    }
}

impl Meter {
    #[document(parent = "Meter", summary = "Counts nonces tried.")]
    pub fn add(&self, nonces: u64) {
        self.tried.fetch_add(nonces, Ordering::Relaxed);
    }

    #[document(parent = "Meter", summary = "Every nonce tried so far.")]
    pub fn total(&self) -> u64 {
        self.tried.load(Ordering::Relaxed)
    }

    #[document(parent = "Meter", summary = "Hashes a second over the last stretch, taken again once it is a few seconds old.")]
    pub fn rate(&self) -> f64 {
        let mut taken = self.taken.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let elapsed = taken.0.elapsed();

        if elapsed >= STRETCH {
            let total = self.total();
            *taken = (Instant::now(), total, total.saturating_sub(taken.1) as f64 / elapsed.as_secs_f64());
        }

        taken.2
    }
}

#[document(summary = "One device mining: a GPU, or the CPU's threads, with its hashrate.", category = "Consensus", see_also = "MiningReport")]
#[derive(Clone, Debug, PartialEq)]
pub struct DeviceReport {
    #[document(summary = "Its name, such as \"Apple M3 Pro (Metal)\" or \"CPU, 4 threads\".")]
    pub name: String,
    #[document(summary = "Hashes a second, over the last few seconds.")]
    pub rate: f64,
}

#[document(summary = "A worker mining for the node over its mining port.", category = "Consensus", see_also = "MiningReport")]
#[derive(Clone, Debug, PartialEq)]
pub struct WorkerReport {
    #[document(summary = "The name it gave.")]
    pub name: String,
    #[document(summary = "Where it connected from.")]
    pub address: String,
    #[document(summary = "When it connected, in milliseconds since the Unix epoch.")]
    pub since: u64,
    #[document(summary = "Its devices and their hashrates, as it last said.")]
    pub devices: Vec<DeviceReport>,
    #[document(summary = "Seals it found that the node took.")]
    pub found: u64,
    #[document(summary = "Nonces it sent that did not meet the work, or came after the block was found.")]
    pub refused: u64,
}

#[document(
    summary = "How a node's mining is going: its own devices, and the workers mining for it, each with its hashrate.",
    category = "Consensus",
    see_also = "Producer::mining",
)]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MiningReport {
    #[document(summary = "The node's own devices.")]
    pub devices: Vec<DeviceReport>,
    #[document(summary = "Where workers connect, when it takes them.")]
    pub listening: Option<String>,
    #[document(summary = "The workers connected now.")]
    pub workers: Vec<WorkerReport>,
}

impl MiningReport {
    #[document(parent = "MiningReport", summary = "Every device's hashrate added up, the workers' included.")]
    pub fn rate(&self) -> f64 {
        self.devices.iter().chain(self.workers.iter().flat_map(|worker| &worker.devices)).map(|device| device.rate).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_meter_counts_from_any_thread_and_the_report_adds_up() {
        let meter = std::sync::Arc::new(Meter::default());
        let threads: Vec<_> = (0..4)
            .map(|_| {
                let meter = meter.clone();
                std::thread::spawn(move || (0..1_000).for_each(|_| meter.add(10)))
            })
            .collect();
        threads.into_iter().for_each(|thread| thread.join().unwrap());

        assert_eq!(meter.total(), 40_000);
        assert_eq!(meter.rate(), 0.0, "not a stretch yet");

        let report = MiningReport {
            devices: vec![DeviceReport { name: "CPU".to_string(), rate: 5.0 }],
            listening: None,
            workers: vec![WorkerReport {
                name: "rig".to_string(),
                address: "10.0.0.2:5000".to_string(),
                since: 0,
                devices: vec![DeviceReport { name: "GPU 0".to_string(), rate: 100.0 }, DeviceReport { name: "GPU 1".to_string(), rate: 50.0 }],
                found: 1,
                refused: 0,
            }],
        };

        assert_eq!(report.rate(), 155.0);
    }
}
