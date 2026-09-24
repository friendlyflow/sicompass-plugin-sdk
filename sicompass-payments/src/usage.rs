//! What the backup service uses: bytes kept against the storage cap, and bytes
//! moved this calendar month against the transfer cap. Every backup reply
//! carries it, and the server's `GET /usage` answers it on its own.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    pub stored: i64,
    pub storage_cap: i64,
    pub transferred: i64,
    pub transfer_cap: i64,
    /// `YYYY-MM` (UTC) the transfer is counted in.
    pub month: String,
}

impl Usage {
    /// The `usage` object of a reply, if it has a well-formed one.
    pub fn from_reply(reply: &serde_json::Value) -> Option<Usage> {
        serde_json::from_value(reply.get("usage")?.clone()).ok()
    }
}

/// A byte count in decimal units, as the server words its refusals: `2.1 GB`.
pub fn human(bytes: i64) -> String {
    let b = bytes.max(0) as f64;
    if b >= 1e9 {
        format!("{:.1} GB", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.0} MB", b / 1e6)
    } else if b >= 1e3 {
        format!("{:.0} kB", b / 1e3)
    } else {
        format!("{bytes} bytes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reply_with_usage_parses_and_one_without_does_not() {
        let reply = serde_json::json!({
            "stored": true,
            "usage": {
                "stored": 2_100_000_000i64, "storage_cap": 10_000_000_000i64,
                "transferred": 340_000_000, "transfer_cap": 5_000_000_000i64,
                "month": "2026-09"
            }
        });
        let u = Usage::from_reply(&reply).unwrap();
        assert_eq!(u.month, "2026-09");
        assert_eq!(u.stored, 2_100_000_000);
        assert!(Usage::from_reply(&serde_json::json!({ "stored": true })).is_none());
    }

    #[test]
    fn sizes_read_naturally() {
        assert_eq!(human(0), "0 bytes");
        assert_eq!(human(1_500), "2 kB");
        assert_eq!(human(340_000_000), "340 MB");
        assert_eq!(human(2_100_000_000), "2.1 GB");
    }
}
