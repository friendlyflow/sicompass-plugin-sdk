//! What a plugin's `license` import hears.
//!
//! The host cannot answer it alone: the issuer key of a tier is in the signed
//! store list, and the certificates and redeem tokens are the Store's, behind
//! the SDK boundary. So the Store registers the answers at startup, the same
//! way the app registers its HTTP client, and the WASM host asks here.
//! Unregistered, every tier is missing and there is no token.

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

/// A status with its day count: until renewal when active, left when in
/// grace, since expiry when expired, 0 when missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Standing {
    pub status: LicenseStatus,
    pub days: i32,
}

impl Standing {
    pub const MISSING: Standing = Standing {
        status: LicenseStatus::Missing,
        days: 0,
    };
}

type Checker = Box<dyn Fn(&str) -> Standing + Send + Sync>;
type TokenSource = Box<dyn Fn(&str) -> Option<String> + Send + Sync>;

static CHECKER: RwLock<Option<Checker>> = RwLock::new(None);
static TOKENS: RwLock<Option<TokenSource>> = RwLock::new(None);

/// Install the check. The last registration wins.
pub fn register_checker(f: impl Fn(&str) -> Standing + Send + Sync + 'static) {
    if let Ok(mut slot) = CHECKER.write() {
        *slot = Some(Box::new(f));
    }
}

/// Install where redeem tokens come from. The host hands one to a plugin only
/// for the tier its manifest names as its service.
pub fn register_token_source(f: impl Fn(&str) -> Option<String> + Send + Sync + 'static) {
    if let Ok(mut slot) = TOKENS.write() {
        *slot = Some(Box::new(f));
    }
}

/// Where the user stands with `tier`.
pub fn standing(tier: &str) -> Standing {
    CHECKER
        .read()
        .ok()
        .and_then(|slot| slot.as_ref().map(|check| check(tier)))
        .unwrap_or(Standing::MISSING)
}

/// Where the user stands with `tier`, without the days.
pub fn status(tier: &str) -> LicenseStatus {
    standing(tier).status
}

/// The redeem token for `tier`, if the user has one.
pub fn token(tier: &str) -> Option<String> {
    TOKENS
        .read()
        .ok()
        .and_then(|slot| slot.as_ref().and_then(|source| source(tier)))
        .filter(|t| !t.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unregistered_everything_is_missing_then_the_registrations_decide() {
        assert_eq!(status("acme/pro"), LicenseStatus::Missing);
        assert_eq!(token("acme/pro"), None);
        register_checker(|tier| {
            if tier == "acme/pro" {
                Standing {
                    status: LicenseStatus::Grace,
                    days: 3,
                }
            } else {
                Standing::MISSING
            }
        });
        register_token_source(|tier| match tier {
            "acme/pro" => Some("tok".to_owned()),
            "acme/empty" => Some(String::new()),
            _ => None,
        });
        assert_eq!(status("acme/pro"), LicenseStatus::Grace);
        assert_eq!(standing("acme/pro").days, 3);
        assert_eq!(standing("acme/other"), Standing::MISSING);
        assert_eq!(token("acme/pro").as_deref(), Some("tok"));
        assert_eq!(token("acme/empty"), None, "an empty token is no token");
    }
}
