//! What a plugin's `license.status(tier)` hears.
//!
//! The host cannot answer it alone: the issuer key of a tier is in the signed
//! store list and the certificates are read by the payments client, both
//! behind the SDK boundary. So the Store registers the check at startup, the
//! same way the app registers its HTTP client, and the WASM host asks here.
//! Unregistered, every tier is [`LicenseStatus::Missing`].

use std::sync::RwLock;

/// Where the user stands with one tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LicenseStatus {
    Active,
    /// Expired less than 14 days ago; the paid part stays on, with a notice.
    Grace,
    Expired,
    Missing,
}

type Checker = Box<dyn Fn(&str) -> LicenseStatus + Send + Sync>;

static CHECKER: RwLock<Option<Checker>> = RwLock::new(None);

/// Install the check. The last registration wins.
pub fn register_checker(f: impl Fn(&str) -> LicenseStatus + Send + Sync + 'static) {
    if let Ok(mut slot) = CHECKER.write() {
        *slot = Some(Box::new(f));
    }
}

/// Where the user stands with `tier`.
pub fn status(tier: &str) -> LicenseStatus {
    CHECKER
        .read()
        .ok()
        .and_then(|slot| slot.as_ref().map(|check| check(tier)))
        .unwrap_or(LicenseStatus::Missing)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unregistered_everything_is_missing_then_the_checker_decides() {
        assert_eq!(status("acme/pro"), LicenseStatus::Missing);
        register_checker(|tier| {
            if tier == "acme/pro" {
                LicenseStatus::Grace
            } else {
                LicenseStatus::Missing
            }
        });
        assert_eq!(status("acme/pro"), LicenseStatus::Grace);
        assert_eq!(status("acme/other"), LicenseStatus::Missing);
    }
}
