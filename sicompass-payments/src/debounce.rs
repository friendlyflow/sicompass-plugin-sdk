//! When to upload: a while after the last change, not on every keystroke.
//!
//! A provider saves on every edit, on the UI thread. It marks the store dirty
//! there, and asks, each frame, whether an upload is due: once the store has
//! been quiet for [`DEBOUNCE_MS`]. Time is the caller's (a plugin's
//! `host::now_millis`), so this has no clock and no thread of its own.

/// Quiet time before an upload.
pub const DEBOUNCE_MS: u64 = 5_000;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Debounce {
    last_change: Option<u64>,
}

impl Debounce {
    /// The store changed at `now` (milliseconds).
    pub fn mark(&mut self, now: u64) {
        self.last_change = Some(now);
    }

    /// Whether an upload is due at `now`. Taking it clears the mark: a change
    /// after this starts a new wait.
    pub fn take_due(&mut self, now: u64) -> bool {
        match self.last_change {
            Some(t) if now.saturating_sub(t) >= DEBOUNCE_MS => {
                self.last_change = None;
                true
            }
            _ => false,
        }
    }

    /// Whether a change is waiting to be uploaded.
    pub fn is_pending(&self) -> bool {
        self.last_change.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_upload_waits_for_quiet_and_each_change_restarts_the_wait() {
        let mut d = Debounce::default();
        assert!(!d.take_due(100_000), "nothing changed");
        d.mark(1_000);
        assert!(!d.take_due(5_999));
        d.mark(4_000); // typing on
        assert!(!d.take_due(8_999));
        assert!(d.is_pending());
        assert!(d.take_due(9_000));
        assert!(!d.is_pending());
        assert!(!d.take_due(20_000), "taken once");
    }
}
