use crate::dto::transfer::TransferStatus;

/// Validates state-machine edges centrally so commands cannot silently put a
/// job in an impossible state. The transfer worker owns the actual side
/// effects; this type only models the transition contract.
pub fn can_transition(from: TransferStatus, to: TransferStatus) -> bool {
    use TransferStatus::*;
    match from {
        Queued => matches!(to, Planning | Cancelling | Cancelled | Failed | Interrupted),
        Planning => matches!(
            to,
            WaitingForUser | Running | Cancelling | Cancelled | Failed | Interrupted
        ),
        WaitingForUser => matches!(to, Running | Cancelling | Cancelled | Failed | Interrupted),
        Running => matches!(
            to,
            Retrying
                | Pausing
                | Paused
                | Cancelling
                | Completed
                | CompletedWithWarnings
                | Failed
                | Interrupted
        ),
        Pausing => matches!(
            to,
            Paused | Running | Cancelling | Cancelled | Failed | Interrupted
        ),
        Paused => matches!(to, Running | Cancelling | Cancelled | Failed | Interrupted),
        Retrying => matches!(to, Running | Cancelling | Failed | Interrupted),
        Cancelling => matches!(to, Cancelled | Failed | Interrupted),
        Completed | CompletedWithWarnings | Failed | Cancelled | Interrupted => false,
    }
}

pub fn transition(from: TransferStatus, to: TransferStatus) -> Result<TransferStatus, String> {
    if can_transition(from, to) {
        Ok(to)
    } else {
        Err(format!(
            "invalid transfer transition: {} -> {}",
            from.as_str(),
            to.as_str()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_happy_path() {
        assert_eq!(
            transition(TransferStatus::Queued, TransferStatus::Planning),
            Ok(TransferStatus::Planning)
        );
        assert_eq!(
            transition(TransferStatus::Planning, TransferStatus::Running),
            Ok(TransferStatus::Running)
        );
        assert_eq!(
            transition(TransferStatus::Running, TransferStatus::Completed),
            Ok(TransferStatus::Completed)
        );
    }

    #[test]
    fn rejects_terminal_and_skips_pause_edges() {
        assert!(transition(TransferStatus::Completed, TransferStatus::Running).is_err());
        assert!(transition(TransferStatus::Queued, TransferStatus::Completed).is_err());
        assert!(transition(TransferStatus::Pausing, TransferStatus::Completed).is_err());
    }
}
