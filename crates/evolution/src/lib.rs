//! Low-frequency personal learning. No runtime/tool dependency or executable output.
#![allow(clippy::missing_errors_doc, clippy::missing_panics_doc)]
#[cfg(test)]
mod tests;
mod worker;
pub use worker::{Handle, RecordSink, start};

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

pub const CREATOR: &str = include_str!("../../../skills/skill-creator/SKILL.md");

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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Step {
    pub tool: String,
    pub detail: String,
    pub success: Option<bool>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Experience {
    pub id: String,
    pub task: String,
    pub intent: String,
    pub tools_used: Vec<String>,
    pub skills_used: Vec<String>,
    pub steps: Vec<Step>,
    pub errors: Vec<String>,
    pub retries: usize,
    /// Exact user text; semantic correction attribution happens only in analysis.
    pub user_corrections: Vec<String>,
    pub success: bool,
    pub project: String,
    pub session: String,
    pub at: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Candidate,
    Trial,
    Active,
    Archived,
    Deleted,
}
impl State {
    fn bucket(self) -> &'static str {
        match self {
            Self::Candidate => "candidate",
            Self::Trial | Self::Active => "live",
            Self::Archived => "archived",
            Self::Deleted => "deleted",
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Metadata {
    pub source: String,
    pub created_at: u64,
    pub last_used_at: u64,
    pub use_count: u64,
    pub success_count: u64,
    pub failure_count: u64,
    pub corrections: u64,
    pub confidence: f64,
    pub state: State,
    pub project: String,
    pub digest: String,
    pub epoch: u64,
    pub low_value_reviews: u64,
    pub evidence: Vec<String>,
    #[serde(default)]
    pub trial_baseline: u64,
    #[serde(default)]
    pub trial_success_baseline: u64,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Ledger {
    pub skills: BTreeMap<String, Metadata>,
    pub experiences: Vec<Experience>,
    pub pending: usize,
    pub last_analysis: u64,
    pub epoch: u64,
}

/// The analyzer proposes data only. Paths, code, tools and permissions are absent.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "UPPERCASE", deny_unknown_fields)]
pub enum Action {
    Create {
        name: String,
        description: String,
        instructions: String,
        evidence: Vec<String>,
        confidence: f64,
    },
    Refine {
        name: String,
        description: String,
        instructions: String,
        evidence: Vec<String>,
        confidence: f64,
        corrections: Vec<String>,
    },
    Merge {
        names: Vec<String>,
        name: String,
        description: String,
        instructions: String,
        evidence: Vec<String>,
        confidence: f64,
    },
    Retire {
        name: String,
    },
    Promote {
        name: String,
        evidence: Vec<String>,
    },
    Memory {
        key: String,
        value: String,
        evidence: Vec<String>,
        quote: String,
        confidence: f64,
    },
    Ignore,
}

pub struct Engine {
    pub root: PathBuf,
    pub database: PathBuf,
    pub project: String,
    pub config: Config,
    pub ledger: Ledger,
}
#[must_use]
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn digest(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}
/// Reuse credential screening without imposing Memory's 1000-character fact limit.
fn screen(text: &str) -> Result<()> {
    ensure!(!text.trim().is_empty(), "empty learning content");
    let chars: Vec<_> = text.chars().collect();
    for start in (0..chars.len()).step_by(800) {
        let chunk: String = chars[start..(start + 1000).min(chars.len())]
            .iter()
            .collect();
        memory::validate_fact("evolution.content", &chunk)?;
    }
    Ok(())
}
fn plain(path: &Path) -> Result<()> {
    if path.exists() || fs::symlink_metadata(path).is_ok() {
        let metadata = fs::symlink_metadata(path)?;
        ensure!(
            !metadata.file_type().is_symlink(),
            "symlinks are not evolution territory"
        );
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            ensure!(
                metadata.file_attributes() & 0x400 == 0,
                "reparse points are not evolution territory"
            );
        }
    }
    Ok(())
}
fn atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    plain(path)?;
    let temp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    fs::write(&temp, bytes)?;
    fs::rename(&temp, path)?;
    Ok(())
}
impl Engine {
    pub fn open(root: PathBuf, database: PathBuf, project: String) -> Result<Self> {
        // Check every ancestor, including junctions on Windows.
        for path in root.ancestors() {
            plain(path)?;
        }
        fs::create_dir_all(&root)?;
        let config_path = root.join("config.json");
        plain(&config_path)?;
        let config: Config = if config_path.exists() {
            serde_json::from_slice(&fs::read(config_path)?)?
        } else {
            Config::default()
        };
        config.validate()?;
        plain(&root.join("ledger.json"))?;
        let ledger = if root.join("ledger.json").exists() {
            serde_json::from_slice(&fs::read(root.join("ledger.json"))?)?
        } else {
            Ledger::default()
        };
        Ok(Self {
            root,
            database,
            project,
            config,
            ledger,
        })
    }
    pub fn save(&self) -> Result<()> {
        atomic(
            &self.root.join("ledger.json"),
            &serde_json::to_vec_pretty(&self.ledger)?,
        )
    }
    pub fn audit(&self, event: &str, result: &str) -> Result<()> {
        let path = self.root.join("decisions.jsonl");
        plain(&path)?;
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        writeln!(
            file,
            "{}",
            serde_json::json!({"at": now(), "epoch": self.ledger.epoch, "event": event, "result": result})
        )?;
        Ok(())
    }
    pub fn record(&mut self, experience: Experience) -> Result<()> {
        ensure!(experience.project == self.project, "wrong project");
        if self
            .ledger
            .experiences
            .iter()
            .any(|e| e.id == experience.id)
        {
            return Ok(());
        }
        // Do not propagate recognized credential material into learning artifacts.
        screen(&experience.task)?;
        for step in &experience.steps {
            screen(&step.detail)?;
        }
        for text in experience.errors.iter().chain(&experience.user_corrections) {
            screen(text)?;
        }
        let path = self.root.join("experiences.jsonl");
        plain(&path)?;
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        writeln!(file, "{}", serde_json::to_string(&experience)?)?;
        for name in &experience.skills_used {
            if let Some(meta) = self.ledger.skills.get_mut(name) {
                if meta.source != "evolved" || !matches!(meta.state, State::Trial | State::Active) {
                    continue;
                }
                meta.last_used_at = experience.at;
                meta.use_count += 1;
                if experience.success && experience.errors.is_empty() {
                    meta.success_count += 1;
                } else {
                    meta.failure_count += 1;
                }
            }
        }
        self.ledger.experiences.push(experience);
        if self.ledger.experiences.len() > self.config.max_experiences {
            self.ledger.experiences.remove(0);
        }
        self.ledger.pending += 1;
        self.save()
    }
    #[must_use]
    pub fn due(&self, ended: bool, time: u64) -> bool {
        self.config.enabled
            && self.ledger.pending > 0
            && (ended || self.ledger.pending >= self.config.batch_size)
            && time.saturating_sub(self.ledger.last_analysis) >= self.config.cooldown_secs
    }
    fn path(&self, name: &str, state: State) -> Result<PathBuf> {
        ensure!(
            !name.is_empty()
                && name.len() <= 64
                && name
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
            "invalid skill name"
        );
        let bucket = self.root.join(state.bucket());
        plain(&bucket)?;
        let directory = bucket.join(name);
        plain(&directory)?;
        plain(&directory.join("SKILL.md"))?;
        Ok(directory)
    }
    pub fn owned(&self, name: &str) -> Result<String> {
        let meta = self
            .ledger
            .skills
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("unowned skill"))?;
        ensure!(
            meta.source == "evolved"
                && meta.project == self.project
                && meta.state != State::Deleted,
            "protected skill"
        );
        let text = fs::read_to_string(self.path(name, meta.state)?.join("SKILL.md"))?;
        ensure!(
            digest(&text) == meta.digest,
            "skill edited externally; automatic mutation refused"
        );
        Ok(text)
    }
    fn evidence<'a>(&'a self, ids: &[String]) -> Result<Vec<&'a Experience>> {
        let unique: std::collections::BTreeSet<_> = ids.iter().collect();
        ensure!(
            unique.len() == ids.len() && !ids.is_empty(),
            "invalid evidence"
        );
        ids.iter()
            .map(|id| {
                self.ledger
                    .experiences
                    .iter()
                    .find(|e| &e.id == id && e.project == self.project)
                    .ok_or_else(|| anyhow::anyhow!("unknown evidence"))
            })
            .collect()
    }
    fn evidence_gate(&self, ids: &[String], confidence: f64, _time: u64) -> Result<f64> {
        ensure!(
            confidence.is_finite() && (0.0..=1.0).contains(&confidence),
            "invalid confidence"
        );
        let evidence = self.evidence(ids)?;
        ensure!(
            evidence
                .iter()
                .all(|e| !e.session.trim().is_empty() && !e.task.trim().is_empty()),
            "missing provenance"
        );
        let sessions: std::collections::BTreeSet<_> = evidence.iter().map(|e| &e.session).collect();
        ensure!(
            evidence.len() >= self.config.minimum_evidence
                && sessions.len() >= self.config.independent_sessions,
            "insufficient independent evidence"
        );
        Ok(confidence) // telemetry only
    }
    fn write_package(
        &self,
        name: &str,
        description: &str,
        instructions: &str,
        state: State,
        replace: bool,
    ) -> Result<String> {
        ensure!(
            description.len() + instructions.len() <= self.config.max_skill_bytes,
            "skill exceeds compression budget"
        );
        screen(instructions)?;
        screen(description)?;
        let target = self.path(name, state)?;
        let staging = self.root.join(format!("stage-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&staging)?;
        let result = (|| {
            let package = skill::create_skill_directory(&staging, name, description, instructions)?;
            let valid = skill::validate_skill_directory(&package)?;
            ensure!(valid.metadata.name == name, "name mismatch");
            let text = fs::read_to_string(package.join("SKILL.md"))?;
            fs::create_dir_all(target.parent().expect("bucket"))?;
            if replace {
                self.owned(name)?;
                atomic(&target.join("SKILL.md"), text.as_bytes())?;
            } else {
                ensure!(!target.exists(), "destination exists");
                fs::rename(package, &target)?;
            }
            Ok(digest(&text))
        })();
        // Only a freshly created, private staging directory is recursively removed.
        fs::remove_dir_all(staging)?;
        result
    }
    fn transition(&mut self, name: &str, next: State) -> Result<()> {
        self.owned(name)?;
        let previous = self.ledger.skills[name].state;
        ensure!(
            matches!(
                (previous, next),
                (State::Candidate, State::Trial)
                    | (State::Trial, State::Active)
                    | (
                        State::Candidate | State::Trial | State::Active,
                        State::Archived
                    )
                    | (State::Archived, State::Deleted)
            ),
            "invalid lifecycle transition"
        );
        let source = self.path(name, previous)?;
        if next == State::Deleted {
            self.audit(
                &format!("{name}: deletion metadata"),
                &serde_json::to_string(&self.ledger.skills[name])?,
            )?;
            // Never delete resources added by a user: only our original single file.
            ensure!(
                fs::read_dir(&source)?.count() == 1,
                "extra resources protect this package"
            );
            fs::remove_file(source.join("SKILL.md"))?;
            fs::remove_dir(source)?;
        } else if previous.bucket() != next.bucket() {
            let target = self.path(name, next)?;
            ensure!(!target.exists(), "destination exists");
            fs::create_dir_all(target.parent().expect("bucket"))?;
            fs::rename(source, target)?;
        }
        let meta = self.ledger.skills.get_mut(name).expect("owned");
        meta.state = next;
        if next == State::Archived {
            meta.low_value_reviews += 1;
        }
        if next == State::Trial {
            meta.trial_baseline = meta.use_count;
            meta.trial_success_baseline = meta.success_count;
        }
        meta.epoch = self.ledger.epoch;
        self.audit(&format!("{name}: {previous:?} -> {next:?}"), "applied")?;
        self.save()
    }
    pub fn maintain(&mut self, _time: u64) -> Result<()> {
        // No semantic lifecycle decisions in maintenance.
        let mut deleted: Vec<_> = self
            .ledger
            .skills
            .iter()
            .filter(|(_, m)| m.state == State::Deleted)
            .map(|(name, m)| (m.epoch, name.clone()))
            .collect();
        deleted.sort();
        let excess = deleted.len().saturating_sub(self.config.max_tombstones);
        for (_, name) in deleted.into_iter().take(excess) {
            self.ledger.skills.remove(&name);
        }
        self.save()
    }
    #[allow(clippy::too_many_lines)] // All action guards are kept at the single mutation boundary.
    pub fn apply(&mut self, action: Action, time: u64) -> Result<()> {
        if matches!(action, Action::Create { .. } | Action::Merge { .. }) {
            ensure!(
                self.ledger
                    .skills
                    .values()
                    .filter(|m| m.state != State::Deleted)
                    .count()
                    < self.config.max_retained_skills,
                "retained package cap: refine/compress or retire first"
            );
        }
        match action {
            Action::Create {
                name,
                description,
                instructions,
                evidence,
                confidence,
            } => {
                ensure!(
                    !self.ledger.skills.contains_key(&name),
                    "name already tracked"
                );
                ensure!(
                    self.ledger
                        .skills
                        .values()
                        .filter(|m| matches!(
                            m.state,
                            State::Candidate | State::Trial | State::Active
                        ))
                        .count()
                        < self.config.max_skills,
                    "growth cap: merge/retire first"
                );
                let score = self.evidence_gate(&evidence, confidence, time)?;
                let digest = self.write_package(
                    &name,
                    &description,
                    &instructions,
                    State::Candidate,
                    false,
                )?;
                self.ledger.skills.insert(
                    name,
                    Metadata {
                        source: "evolved".into(),
                        created_at: time,
                        last_used_at: time,
                        use_count: 0,
                        success_count: 0,
                        failure_count: 0,
                        corrections: 0,
                        confidence: score,
                        state: State::Candidate,
                        project: self.project.clone(),
                        digest,
                        epoch: self.ledger.epoch,
                        low_value_reviews: 0,
                        evidence,
                        trial_baseline: 0,
                        trial_success_baseline: 0,
                    },
                );
            }
            Action::Refine {
                name,
                description,
                instructions,
                evidence,
                confidence,
                corrections,
            } => {
                self.owned(&name)?;
                let score = self.evidence_gate(&evidence, confidence, time)?;
                let records = self.evidence(&evidence)?;
                for quote in &corrections {
                    ensure!(
                        !quote.is_empty() && records.iter().any(|e| e.task.contains(quote)),
                        "unsubstantiated correction"
                    );
                }
                let state = self.ledger.skills[&name].state;
                ensure!(
                    matches!(state, State::Trial | State::Active | State::Candidate),
                    "archived skill"
                );
                let digest = self.write_package(&name, &description, &instructions, state, true)?;
                let meta = self.ledger.skills.get_mut(&name).expect("owned");
                meta.digest = digest;
                meta.confidence = score;
                meta.evidence = evidence;
                meta.corrections += corrections.len() as u64;
                // Changed active instructions must earn confidence in trial again.
                if matches!(meta.state, State::Active | State::Trial) {
                    meta.state = State::Trial;
                    meta.trial_baseline = meta.use_count;
                    meta.trial_success_baseline = meta.success_count;
                }
                meta.epoch = self.ledger.epoch;
                // Attribute semantic corrections only during low-frequency analysis.
                for quote in corrections {
                    for experience in &mut self.ledger.experiences {
                        if meta.evidence.contains(&experience.id)
                            && experience.task.contains(&quote)
                            && !experience.user_corrections.contains(&quote)
                        {
                            experience.user_corrections.push(quote.clone());
                        }
                    }
                }
            }
            Action::Merge {
                names,
                name,
                description,
                instructions,
                evidence,
                confidence,
            } => {
                ensure!(
                    names.len() >= 2
                        && names
                            .iter()
                            .collect::<std::collections::BTreeSet<_>>()
                            .len()
                            == names.len(),
                    "merge needs distinct sources"
                );
                ensure!(
                    !self.ledger.skills.contains_key(&name),
                    "merge target exists"
                );
                let texts: Vec<_> = names.iter().map(|n| self.owned(n)).collect::<Result<_>>()?;
                ensure!(
                    description.len() + instructions.len()
                        < texts.iter().map(String::len).sum::<usize>(),
                    "merge must compress"
                );
                let score = self.evidence_gate(&evidence, confidence, time)?;
                for n in &names {
                    ensure!(
                        matches!(
                            self.ledger.skills[n].state,
                            State::Candidate | State::Trial | State::Active
                        ),
                        "merge source unavailable"
                    );
                }
                let hash = self.write_package(
                    &name,
                    &description,
                    &instructions,
                    State::Candidate,
                    false,
                )?;
                self.ledger.skills.insert(
                    name,
                    Metadata {
                        source: "evolved".into(),
                        created_at: time,
                        last_used_at: time,
                        use_count: 0,
                        success_count: 0,
                        failure_count: 0,
                        corrections: 0,
                        confidence: score,
                        state: State::Candidate,
                        project: self.project.clone(),
                        digest: hash,
                        epoch: self.ledger.epoch,
                        low_value_reviews: 0,
                        evidence,
                        trial_baseline: 0,
                        trial_success_baseline: 0,
                    },
                );
                self.save()?;
                for source in names {
                    self.transition(&source, State::Archived)?;
                }
            }
            Action::Promote { name, evidence } => {
                self.owned(&name)?;
                self.evidence_gate(&evidence, 1.0, time)?;
                let meta = &self.ledger.skills[&name];
                let next = match meta.state {
                    State::Candidate => State::Trial,
                    State::Trial
                        if meta.use_count > meta.trial_baseline
                            && meta.success_count > meta.trial_success_baseline =>
                    {
                        State::Active
                    }
                    _ => anyhow::bail!("promotion requires trial usage telemetry"),
                };
                self.transition(&name, next)?;
            }
            Action::Retire { name } => {
                self.owned(&name)?;
                let meta = &self.ledger.skills[&name];
                self.evidence_gate(&meta.evidence, meta.confidence, time)?;
                let next = if meta.state == State::Archived && meta.epoch < self.ledger.epoch {
                    State::Deleted
                } else {
                    State::Archived
                };
                self.transition(&name, next)?;
            }
            Action::Memory {
                key,
                value,
                evidence,
                quote,
                confidence,
            } => {
                self.evidence_gate(&evidence, confidence, time)?;
                ensure!(
                    !quote.is_empty()
                        && self
                            .evidence(&evidence)?
                            .iter()
                            .any(|e| e.task.contains(&quote)),
                    "missing user provenance"
                );
                memory::validate_fact(&key, &value)?;
                let store = memory::MemoryStore::open(&self.database)?;
                let scope = memory::MemoryScope::Project;
                let existing = store
                    .scoped_memories(scope, &self.project)?
                    .into_iter()
                    .find(|m| m.key == key);
                ensure!(
                    existing.as_ref().is_none_or(|m| m.source == "evolved"),
                    "user memory protected"
                );
                store.change_fact(
                    &memory::MemoryRecord {
                        key,
                        value,
                        scope,
                        owner: self.project.clone(),
                        source: "evolved".into(),
                        updated_at: 0,
                        memory_type: memory::MemoryType::Experience,
                        always_include: false,
                        ..Default::default()
                    },
                    existing.as_ref().map(|m| m.value.as_str()),
                    false,
                )?;
            }
            Action::Ignore => {}
        }
        self.save()
    }
    pub fn analysis_input(&self) -> Result<String> {
        let mut packages = BTreeMap::new();
        for name in self.ledger.skills.keys() {
            if let Ok(text) = self.owned(name) {
                packages.insert(name, text);
            }
        }
        Ok(serde_json::to_string(
            &serde_json::json!({"experiences": self.ledger.experiences, "metadata": self.ledger.skills, "packages": packages, "config": self.config}),
        )?)
    }

    /// Fit data before requesting a model; old raw experiences remain in JSONL.
    pub fn bounded_analysis_input(&self, max_bytes: usize) -> Result<String> {
        let mut input: serde_json::Value = serde_json::from_str(&self.analysis_input()?)?;
        let metadata = input["metadata"].as_object_mut().expect("metadata");
        metadata.retain(|_, m| m["state"] != "deleted");
        for experience in input["experiences"].as_array_mut().expect("experiences") {
            for step in experience["steps"].as_array_mut().expect("steps") {
                step["detail"] = serde_json::Value::String(
                    step["detail"]
                        .as_str()
                        .unwrap_or_default()
                        .chars()
                        .take(240)
                        .collect(),
                );
            }
        }
        loop {
            let encoded = serde_json::to_string(&input)?;
            if encoded.len() <= max_bytes {
                return Ok(encoded);
            }
            let experiences = input["experiences"].as_array_mut().expect("experiences");
            if experiences.len() > 1 {
                experiences.remove(0);
                continue;
            }
            let packages = input["packages"].as_object_mut().expect("packages");
            if let Some(name) = packages.keys().next().cloned() {
                packages.remove(&name);
                continue;
            }
            ensure!(false, "analysis metadata exceeds provider context capacity");
        }
    }
}
