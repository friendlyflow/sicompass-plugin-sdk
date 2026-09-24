//! What a cloud-backed plugin says about its backup, on the row at the top of
//! its list.
//!
//! The wording is the plugin's (its own Fluent ids, in its own languages), so
//! this only decides which message applies and with which number: a plugin
//! builds the id as `<plugin>-cloud-<key>` and passes `days` as `$days`. The
//! standing comes from the host (`license.standing` for the plugin's service
//! tier), which verified the certificate; the plugin never sees one.
//!
//! The paywall is on the service, never on the data: whatever this says, the
//! plugin keeps showing and saving the user's own notes or board.

/// Where the user stands with the service's tier, as the host reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    Active {
        renews_in_days: i32,
    },
    /// Expired, but within the grace period: backup still runs.
    Grace {
        days_left: i32,
    },
    Expired {
        days_ago: i32,
    },
    Missing,
}

impl Standing {
    /// Whether the backup service is on: active, or in its grace period.
    pub fn backs_up(self) -> bool {
        matches!(self, Standing::Active { .. } | Standing::Grace { .. })
    }
}

/// The message for the row: the id suffix (`active`, `grace`, `expired`,
/// `needs-payment`) and the day count for `$days`.
pub fn row_message(standing: Standing) -> (&'static str, i32) {
    match standing {
        Standing::Active { renews_in_days } => ("active", renews_in_days),
        Standing::Grace { days_left } => ("grace", days_left),
        Standing::Expired { days_ago } => ("expired", days_ago),
        Standing::Missing => ("needs-payment", 0),
    }
}

/// Every suffix [`row_message`] can produce, for a plugin's test that its
/// bundles carry `<plugin>-cloud-<suffix>` for each.
pub const ROW_MESSAGES: [&str; 4] = ["active", "grace", "expired", "needs-payment"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_standing_has_its_message_and_number() {
        assert_eq!(
            row_message(Standing::Active {
                renews_in_days: 200
            }),
            ("active", 200)
        );
        assert_eq!(row_message(Standing::Grace { days_left: 3 }), ("grace", 3));
        assert_eq!(
            row_message(Standing::Expired { days_ago: 20 }),
            ("expired", 20)
        );
        assert_eq!(row_message(Standing::Missing), ("needs-payment", 0));
        for s in [
            Standing::Active { renews_in_days: 1 },
            Standing::Grace { days_left: 1 },
            Standing::Expired { days_ago: 1 },
            Standing::Missing,
        ] {
            assert!(ROW_MESSAGES.contains(&row_message(s).0));
        }
    }

    #[test]
    fn backup_runs_while_active_or_in_grace() {
        assert!(Standing::Active { renews_in_days: 1 }.backs_up());
        assert!(Standing::Grace { days_left: 1 }.backs_up());
        assert!(!Standing::Expired { days_ago: 1 }.backs_up());
        assert!(!Standing::Missing.backs_up());
    }
}
