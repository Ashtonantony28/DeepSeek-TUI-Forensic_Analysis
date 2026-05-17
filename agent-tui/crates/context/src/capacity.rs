//! Capacity controller: maps context-usage + tool volatility to a
//! `RiskBand` and a `GuardrailAction`.
//!
//! Matches ANALYSIS.md §5 — three bands (Low/Medium/High) and the
//! existing `VerifyAndReplan` action on `High` after the cooldown.

use agent_tui_protocol::{GuardrailAction, RiskBand};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapacityConfig {
    pub enabled: bool,
    pub low_risk_max: f32,
    pub medium_risk_max: f32,
    pub cooldown_turns: u32,
}

impl Default for CapacityConfig {
    fn default() -> Self {
        Self {
            // Disabled by default (matches upstream since v0.8.11) to
            // protect prefix-cache stability.
            enabled: false,
            low_risk_max: 0.50,
            medium_risk_max: 0.62,
            cooldown_turns: 5,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CapacityObservation {
    pub context_used_ratio: f32,
    /// Tool calls in the recent window (e.g. last 5 turns).
    pub tool_calls_recent: u32,
    /// Number of consecutive errored tool calls.
    pub consecutive_tool_errors: u32,
}

#[derive(Debug, Default)]
pub struct CapacityController {
    pub config: CapacityConfig,
    pub last_action_turn: Option<u32>,
}

impl CapacityController {
    pub fn new(config: CapacityConfig) -> Self {
        Self {
            config,
            last_action_turn: None,
        }
    }

    pub fn observe(&self, obs: CapacityObservation) -> RiskBand {
        // Bump the band if there have been consecutive tool errors.
        let bump = match obs.consecutive_tool_errors {
            0 => 0.0,
            1 => 0.05,
            2 => 0.10,
            _ => 0.20,
        };
        let effective = obs.context_used_ratio + bump;
        if effective <= self.config.low_risk_max {
            RiskBand::Low
        } else if effective <= self.config.medium_risk_max {
            RiskBand::Medium
        } else {
            RiskBand::High
        }
    }

    pub fn decide(&mut self, turn_index: u32, band: RiskBand) -> GuardrailAction {
        if !self.config.enabled {
            return GuardrailAction::NoIntervention;
        }
        let cooldown_ok = self
            .last_action_turn
            .map(|t| turn_index.saturating_sub(t) >= self.config.cooldown_turns)
            .unwrap_or(true);
        match (band, cooldown_ok) {
            (RiskBand::High, true) => {
                self.last_action_turn = Some(turn_index);
                GuardrailAction::VerifyAndReplan
            }
            (RiskBand::Medium, true) => GuardrailAction::TargetedContextRefresh,
            _ => GuardrailAction::NoIntervention,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_disabled_so_no_intervention() {
        let mut c = CapacityController::default();
        let band = c.observe(CapacityObservation {
            context_used_ratio: 0.9,
            tool_calls_recent: 5,
            consecutive_tool_errors: 5,
        });
        assert_eq!(band, RiskBand::High);
        assert_eq!(c.decide(10, band), GuardrailAction::NoIntervention);
    }

    #[test]
    fn enabled_high_triggers_verify_and_replan() {
        let mut c = CapacityController::new(CapacityConfig {
            enabled: true,
            ..Default::default()
        });
        let band = c.observe(CapacityObservation {
            context_used_ratio: 0.7,
            tool_calls_recent: 1,
            consecutive_tool_errors: 0,
        });
        assert_eq!(band, RiskBand::High);
        assert_eq!(c.decide(0, band), GuardrailAction::VerifyAndReplan);
    }

    #[test]
    fn cooldown_blocks_repeat() {
        let mut c = CapacityController::new(CapacityConfig {
            enabled: true,
            cooldown_turns: 5,
            ..Default::default()
        });
        let band = RiskBand::High;
        assert_eq!(c.decide(10, band), GuardrailAction::VerifyAndReplan);
        assert_eq!(c.decide(12, band), GuardrailAction::NoIntervention);
        assert_eq!(c.decide(15, band), GuardrailAction::VerifyAndReplan);
    }
}
