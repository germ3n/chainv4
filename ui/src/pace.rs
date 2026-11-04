//! How often the apps ask their node again, and wake to see what came due: as each page sets, or
//! with Low data on, a few times less often, for a metered connection or a phone's battery. One
//! setting for every timer, rather than one each, so that nobody has to know which page polls what.
//! What someone asks for themselves, retries after a failure and what a stream pushes are never
//! held back by it.

/// How many times longer the apps wait between asking the same thing again with Low data on.
pub const LOW_DATA_TIMES: f64 = 4.0;

/// How often an app asks and wakes: as usual, or with Low data on. Copy, so each part of an app
/// that polls keeps its own, set from the setting as it changes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pace {
    pub low_data: bool,
}

impl Pace {
    pub fn new(low_data: bool) -> Pace {
        Pace { low_data }
    }

    /// The seconds between asks of something asked every `seconds` as usual.
    pub fn every(self, seconds: f64) -> f64 {
        match self.low_data {
            true => seconds * LOW_DATA_TIMES,
            false => seconds,
        }
    }

    /// As every, in milliseconds, for what counts its time in them.
    pub fn every_ms(self, ms: u64) -> u64 {
        match self.low_data {
            true => ms * LOW_DATA_TIMES as u64,
            false => ms,
        }
    }

    /// How long an app may sleep with nothing arriving before it wakes to see what came due, where
    /// it would usually wake after `usual`: a frame drawn costs a phone's battery as a request does.
    /// What arrives wakes it at once all the same.
    pub fn wake_after(self, usual: std::time::Duration) -> std::time::Duration {
        match self.low_data {
            true => usual.mul_f64(LOW_DATA_TIMES),
            false => usual,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn low_data_stretches_every_timer_by_the_same_and_off_leaves_each_as_it_was() {
        let (usual, low) = (Pace::default(), Pace::new(true));

        for seconds in [1.0, 2.5, 10.0, 15.0] {
            assert_eq!(usual.every(seconds), seconds, "off, as before");
            assert_eq!(low.every(seconds), seconds * LOW_DATA_TIMES);
        }

        assert_eq!((usual.every_ms(15_000), low.every_ms(15_000)), (15_000, 60_000));
        let usual_wake = std::time::Duration::from_millis(500);
        assert_eq!(usual.wake_after(usual_wake), usual_wake);
        assert_eq!(low.wake_after(usual_wake), std::time::Duration::from_secs(2));
    }
}
