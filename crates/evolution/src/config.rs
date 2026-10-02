//! Scheduling, scoring and growth limits have one configurable home.

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

/// Scheduling, scoring and growth limits have one configurable home.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub enabled: bool,
    pub minimum_evidence: usize,
    pub independent_sessions: usize,
    pub batch_size: usize,
    pub cooldown_secs: u64,
    pub analysis_timeout_secs: u64,
    pub max_experiences: usize,
    pub max_skills: usize,
    pub max_retained_skills: usize,
    pub max_tombstones: usize,
    pub max_skill_bytes: usize,
    pub max_actions: usize,
    pub similarity: f64,
    pub create_score: f64,
    pub trial_score: f64,
    pub active_score: f64,
    pub retire_score: f64,
    pub half_life_secs: f64,
    pub weights: [f64; 6],
}
impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: true,
            minimum_evidence: 2,
            independent_sessions: 2,
            batch_size: 12,
            cooldown_secs: 1800,
            analysis_timeout_secs: 30,
            max_experiences: 64,
            max_skills: 24,
            max_retained_skills: 48,
            max_tombstones: 24,
            max_skill_bytes: 12_000,
            max_actions: 4,
            similarity: 0.86,
            create_score: 0.90,
            trial_score: 0.65,
            active_score: 0.78,
            retire_score: 0.25,
            half_life_secs: 30.0 * 86400.0,
            weights: [0.25, 0.20, 0.15, 0.10, 0.10, 0.20],
        }
    }
}
impl Config {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.minimum_evidence > 0 && self.independent_sessions > 0,
            "invalid evidence gates"
        );
        ensure!(
            self.batch_size > 0
                && self.max_experiences >= self.batch_size
                && self.max_experiences <= 256,
            "invalid experience limits"
        );
        ensure!(
            self.max_skills > 0
                && self.max_skills <= 128
                && self.max_retained_skills >= self.max_skills
                && self.max_retained_skills <= 256
                && self.max_tombstones > 0
                && self.max_tombstones <= 128
                && self.max_actions > 0
                && self.max_actions <= 16
                && self.max_skill_bytes > 0
                && self.max_skill_bytes <= 32_000,
            "invalid growth limits"
        );
        ensure!(
            self.cooldown_secs > 0
                && self.analysis_timeout_secs > 0
                && self.half_life_secs.is_finite()
                && self.half_life_secs > 0.0,
            "invalid timing"
        );
        for score in [
            self.similarity,
            self.create_score,
            self.trial_score,
            self.active_score,
            self.retire_score,
        ] {
            ensure!(
                score.is_finite() && (0.0..=1.0).contains(&score),
                "invalid score"
            );
        }
        ensure!(
            self.weights.iter().all(|w| w.is_finite() && *w >= 0.0)
                && (self.weights.iter().sum::<f64>() - 1.0).abs() < 0.001,
            "weights must sum to one"
        );
        Ok(())
    }
}
