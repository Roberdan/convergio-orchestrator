//! Plan state machine — typed transitions with explicit validation.
//!
//! Replaces ad-hoc string status checks with a formal FSM.
//! Every plan status change MUST go through `PlanStatus::can_transition_to()`.

use std::fmt;
use std::str::FromStr;

/// All valid plan statuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlanStatus {
    Todo,
    Draft,
    Approved,
    InProgress,
    Paused,
    Stale,
    Failed,
    Done,
    Cancelled,
}

impl FromStr for PlanStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "todo" => Ok(Self::Todo),
            "draft" => Ok(Self::Draft),
            "approved" => Ok(Self::Approved),
            "in_progress" | "active" => Ok(Self::InProgress),
            "paused" => Ok(Self::Paused),
            "stale" => Ok(Self::Stale),
            "failed" => Ok(Self::Failed),
            "done" => Ok(Self::Done),
            "cancelled" => Ok(Self::Cancelled),
            _ => Err(format!("unknown plan status: '{s}'")),
        }
    }
}

impl PlanStatus {
    /// Convert back to database string.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Todo => "todo",
            Self::Draft => "draft",
            Self::Approved => "approved",
            Self::InProgress => "in_progress",
            Self::Paused => "paused",
            Self::Stale => "stale",
            Self::Failed => "failed",
            Self::Done => "done",
            Self::Cancelled => "cancelled",
        }
    }

    /// Whether this status is terminal (no further transitions allowed
    /// except force-ops).
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Done | Self::Cancelled)
    }

    /// Check if a transition to `target` is allowed.
    pub fn can_transition_to(&self, target: &PlanStatus) -> bool {
        matches!(
            (self, target),
            // Normal flow
            (Self::Todo, Self::Draft)
                | (Self::Todo, Self::Approved)
                | (Self::Todo, Self::InProgress)
                | (Self::Todo, Self::Cancelled)
                | (Self::Draft, Self::Approved)
                | (Self::Draft, Self::Cancelled)
                | (Self::Approved, Self::InProgress)
                | (Self::Approved, Self::Cancelled)
                // Execution
                | (Self::InProgress, Self::Paused)
                | (Self::InProgress, Self::Failed)
                | (Self::InProgress, Self::Done)
                | (Self::InProgress, Self::Stale)
                | (Self::InProgress, Self::Cancelled)
                // Recovery
                | (Self::Paused, Self::InProgress)
                | (Self::Paused, Self::Cancelled)
                | (Self::Paused, Self::Failed)
                | (Self::Stale, Self::InProgress)
                | (Self::Stale, Self::Cancelled)
                | (Self::Stale, Self::Failed)
                // Retry from failure
                | (Self::Failed, Self::Draft)
                | (Self::Failed, Self::InProgress)
                | (Self::Failed, Self::Cancelled)
        )
    }

    /// Validate and return the target status, or an error message.
    pub fn validate_transition(&self, target: &PlanStatus) -> Result<(), String> {
        if self.can_transition_to(target) {
            Ok(())
        } else {
            Err(format!(
                "invalid plan transition: '{}' → '{}' not allowed",
                self.as_str(),
                target.as_str()
            ))
        }
    }
}

impl fmt::Display for PlanStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Validate a plan status transition given raw DB strings.
/// Returns Ok(target_str) or Err with reason.
pub fn validate_plan_transition(current: &str, target: &str) -> Result<&'static str, String> {
    let from: PlanStatus = current.parse()?;
    let to: PlanStatus = target.parse()?;
    from.validate_transition(&to)?;
    Ok(to.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_flow() {
        assert!(PlanStatus::Todo.can_transition_to(&PlanStatus::Approved));
        assert!(PlanStatus::Approved.can_transition_to(&PlanStatus::InProgress));
        assert!(PlanStatus::InProgress.can_transition_to(&PlanStatus::Done));
    }

    #[test]
    fn pause_resume_cycle() {
        assert!(PlanStatus::InProgress.can_transition_to(&PlanStatus::Paused));
        assert!(PlanStatus::Paused.can_transition_to(&PlanStatus::InProgress));
    }

    #[test]
    fn stale_recovery() {
        assert!(PlanStatus::InProgress.can_transition_to(&PlanStatus::Stale));
        assert!(PlanStatus::Stale.can_transition_to(&PlanStatus::InProgress));
    }

    #[test]
    fn terminal_blocks() {
        assert!(!PlanStatus::Done.can_transition_to(&PlanStatus::InProgress));
        assert!(!PlanStatus::Cancelled.can_transition_to(&PlanStatus::InProgress));
    }

    #[test]
    fn cannot_go_backwards_from_executing() {
        assert!(!PlanStatus::InProgress.can_transition_to(&PlanStatus::Todo));
        assert!(!PlanStatus::InProgress.can_transition_to(&PlanStatus::Approved));
    }

    #[test]
    fn retry_from_failure() {
        assert!(PlanStatus::Failed.can_transition_to(&PlanStatus::Draft));
        assert!(PlanStatus::Failed.can_transition_to(&PlanStatus::InProgress));
    }

    #[test]
    fn cancel_from_anywhere_non_terminal() {
        for status in [
            PlanStatus::Todo,
            PlanStatus::Draft,
            PlanStatus::Approved,
            PlanStatus::InProgress,
            PlanStatus::Paused,
            PlanStatus::Stale,
            PlanStatus::Failed,
        ] {
            assert!(
                status.can_transition_to(&PlanStatus::Cancelled),
                "{status} should be cancellable"
            );
        }
    }

    #[test]
    fn parse_roundtrip() {
        for s in [
            "todo",
            "draft",
            "approved",
            "in_progress",
            "paused",
            "stale",
            "failed",
            "done",
            "cancelled",
        ] {
            let status = PlanStatus::from_str(s).unwrap();
            assert_eq!(status.as_str(), s);
        }
    }

    #[test]
    fn active_alias() {
        assert_eq!(
            PlanStatus::from_str("active").unwrap(),
            PlanStatus::InProgress
        );
    }

    #[test]
    fn validate_plan_transition_ok() {
        assert!(validate_plan_transition("todo", "in_progress").is_ok());
    }

    #[test]
    fn validate_plan_transition_err() {
        assert!(validate_plan_transition("done", "in_progress").is_err());
    }
}
