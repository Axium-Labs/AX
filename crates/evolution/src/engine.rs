//! The `Engine`: the single mutation boundary for evolved skills.
//!
//! Every write to the ledger or to a skill package goes through one method, so
//! ownership, digests, territory guards and the audit trail cannot be bypassed
//! by a new action variant.

use std::{
    collections::{BTreeMap, HashMap},
    fs,
    io::Write,
    path::PathBuf,
};

use anyhow::{Context, Result, ensure};

use crate::{
    Action, Config, Experience, Ledger, Metadata, State, policy,
    storage::{atomic, digest, now, plain, screen},
};

/// Temporary migration control, removed when version 2 is published.
#[derive(serde::Serialize, serde::Deserialize)]
struct MigrationCheckpoint {
    original_end: u64,
    known_end: u64,
    processed: u64,
    consume_all: bool,
}

pub struct Engine {
    pub root: PathBuf,
    pub database: PathBuf,
    pub project: String,
    pub config: Config,
    pub ledger: Ledger,
    /// Bounded runtime evidence, never serialized into the control ledger.
    pub recent: Vec<Experience>,
    records: Vec<(String, u64, u64)>,
    ids: HashMap<String, usize>,
    indexed_end: u64,
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
        let value: serde_json::Value = if root.join("ledger.json").exists() {
            serde_json::from_slice(&fs::read(root.join("ledger.json"))?)?
        } else {
            serde_json::json!({"version": 2})
        };
        let ledger: Ledger = serde_json::from_value(value.clone())?;
        ensure!(ledger.version <= 2, "unsupported evolution ledger version");
        let mut engine = Self {
            root,
            database,
            project,
            config,
            ledger,
            recent: Vec::new(),
            records: Vec::new(),
            ids: HashMap::new(),
            indexed_end: 0,
        };
        engine.load_records()?;
        engine.migrate(&value)?;
        engine.observe()?;
        Ok(engine)
    }
    /// Read only bytes appended since this Engine last indexed the stream.
    fn load_records(&mut self) -> Result<()> {
        let path = self.root.join("experiences.jsonl");
        let rows = crate::storage::read_experiences(&path, self.indexed_end)?;
        for row in rows {
            let (experience, start, end) = row?;
            ensure!(
                experience.project == self.project,
                "wrong project in experience stream"
            );
            if !self.ids.contains_key(&experience.id) {
                self.ids.insert(experience.id.clone(), self.records.len());
                self.records.push((experience.id.clone(), start, end));
                self.recent.push(experience);
                self.trim_recent();
            }
            self.indexed_end = end;
        }
        ensure!(
            self.ledger.observed_cursor <= self.indexed_end
                && self.ledger.processed_cursor <= self.ledger.observed_cursor,
            "experience stream is shorter than its checkpoint"
        );
        // Validate the persisted analysis boundary without reading old records.
        crate::storage::read_experiences(&path, self.ledger.processed_cursor)?;
        Ok(())
    }

    fn trim_recent(&mut self) {
        let excess = self
            .recent
            .len()
            .saturating_sub(self.config.max_experiences);
        self.recent.drain(..excess);
    }

    fn migrate(&mut self, value: &serde_json::Value) -> Result<()> {
        if self.ledger.version == 2 && value.get("experiences").is_none() {
            return Ok(());
        }
        let legacy: Vec<Experience> = value
            .get("experiences")
            .map(|v| serde_json::from_value(v.clone()))
            .transpose()?
            .unwrap_or_default();
        for experience in &legacy {
            ensure!(
                experience.project == self.project,
                "wrong project in legacy ledger"
            );
        }
        let checkpoint: MigrationCheckpoint = if let Some(saved) = value.get("experience_migration")
        {
            serde_json::from_value(saved.clone())?
        } else {
            let original_end = self.indexed_end;
            let known_end = legacy
                .iter()
                .filter_map(|e| self.ids.get(&e.id))
                .map(|i| self.records[*i].2)
                .max()
                .unwrap_or(0);
            let old_count = self
                .records
                .partition_point(|(_, _, end)| *end <= known_end);
            let missing = legacy
                .iter()
                .filter(|e| !self.ids.contains_key(&e.id))
                .count();
            let pending = self.ledger.pending.min(old_count + missing);
            // Missing legacy copies are appended at the tail. When old pending work
            // exists, conservatively replay from the earliest possible old record.
            let missing_pending = legacy
                .iter()
                .rev()
                .take(pending)
                .filter(|e| !self.ids.contains_key(&e.id))
                .count();
            let first = old_count.saturating_sub(pending.saturating_sub(missing_pending));
            let processed = self
                .records
                .get(first)
                .map_or(known_end, |(_, start, _)| *start);
            let checkpoint = MigrationCheckpoint {
                original_end,
                known_end,
                processed,
                consume_all: pending == 0 && known_end == original_end,
            };
            // Publish only control offsets before the first legacy-only append.
            // A retry must use the original pending boundary, not infer it from
            // a stream that already contains part of this migration's appends.
            let mut checkpointed = value.clone();
            checkpointed["experience_migration"] = serde_json::to_value(&checkpoint)?;
            atomic(
                &self.root.join("ledger.json"),
                &serde_json::to_vec_pretty(&checkpointed)?,
            )?;
            checkpoint
        };
        for experience in &legacy {
            ensure!(
                experience.project == self.project,
                "wrong project in legacy ledger"
            );
            if !self.ids.contains_key(&experience.id) {
                crate::storage::append_experience(
                    &self.root.join("experiences.jsonl"),
                    experience,
                )?;
                self.load_records()?;
            }
        }
        // Records appended before an old ledger checkpoint failed are absent
        // from its recent list. Recover their telemetry rather than marking
        // them observed or analyzed during migration.
        let mut next = self.ledger.clone();
        for row in crate::storage::read_experiences_range(
            &self.root.join("experiences.jsonl"),
            checkpoint.known_end,
            checkpoint.original_end,
        )? {
            let (experience, start, _) = row?;
            if self.records[self.ids[&experience.id]].1 == start
                && !legacy.iter().any(|e| e.id == experience.id)
            {
                Self::usage(&mut next, &experience);
            }
        }
        next.processed_cursor = if checkpoint.consume_all {
            self.indexed_end
        } else {
            checkpoint.processed
        };
        next.observed_cursor = self.indexed_end;
        next.version = 2;
        self.ledger = next;
        self.save()
    }

    fn usage(ledger: &mut Ledger, experience: &Experience) {
        for name in &experience.skills_used {
            if let Some(meta) = ledger.skills.get_mut(name) {
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
    }

    /// Checkpoint telemetry independently of analysis, atomically with its cursor.
    pub(crate) fn observe(&mut self) -> Result<()> {
        let mut next = self.ledger.clone();
        for row in crate::storage::read_experiences(
            &self.root.join("experiences.jsonl"),
            next.observed_cursor,
        )? {
            let (experience, start, end) = row?;
            ensure!(
                experience.project == self.project,
                "wrong project in experience stream"
            );
            // Preexisting duplicate IDs remain in the immutable stream, but
            // only their first occurrence contributes usage or pending work.
            if self.records[self.ids[&experience.id]].1 == start {
                Self::usage(&mut next, &experience);
            }
            next.observed_cursor = end;
        }
        next.pending = self.records.len()
            - self
                .records
                .partition_point(|(_, _, end)| *end <= next.processed_cursor);
        atomic(
            &self.root.join("ledger.json"),
            &serde_json::to_vec_pretty(&next)?,
        )?;
        self.ledger = next;
        Ok(())
    }

    /// Restore control changes from another writer; retain the stream index normally.
    pub(crate) fn refresh(&mut self) -> Result<()> {
        plain(&self.root.join("ledger.json"))?;
        let ledger: Ledger = serde_json::from_slice(&fs::read(self.root.join("ledger.json"))?)?;
        ensure!(ledger.version == 2, "unsupported evolution ledger version");
        self.ledger = ledger;
        let path = self.root.join("config.json");
        plain(&path)?;
        self.config = if path.exists() {
            serde_json::from_slice(&fs::read(path)?)?
        } else {
            Config::default()
        };
        self.config.validate()?;
        self.load_records()?;
        self.trim_recent();
        self.observe()
    }

    /// Select the next bounded batch by byte cursor, retaining earlier context.
    pub(crate) fn prepare_analysis(&mut self, budget: usize) -> Result<(String, u64)> {
        let first = self
            .records
            .partition_point(|(_, _, end)| *end <= self.ledger.processed_cursor);
        ensure!(first < self.records.len(), "no pending experiences");
        let end = (first + self.config.max_experiences).min(self.records.len());
        let start = end.saturating_sub(self.config.max_experiences);
        let mut selected = Vec::new();
        for row in crate::storage::read_experiences_range(
            &self.root.join("experiences.jsonl"),
            self.records[start].1,
            self.records[end - 1].2,
        )? {
            let (mut experience, offset, _) = row?;
            if self.records[self.ids[&experience.id]].1 != offset {
                continue;
            }
            if let Some(previous) = self.recent.iter().find(|e| e.id == experience.id) {
                experience
                    .user_corrections
                    .clone_from(&previous.user_corrections);
            }
            selected.push(experience);
        }
        self.recent = selected;
        let mut input: serde_json::Value =
            serde_json::from_str(&self.bounded_analysis_input(budget)?)?;
        // bounded_analysis_input must never discard the earliest pending record.
        // If context fitting removed it, reduce the batch from its newest end.
        while !input["experiences"]
            .as_array()
            .expect("experiences")
            .iter()
            .any(|e| e["id"].as_str() == Some(self.records[first].0.as_str()))
        {
            ensure!(
                self.recent.len() > 1,
                "pending experience exceeds provider context capacity"
            );
            self.recent.pop();
            input = serde_json::from_str(&self.bounded_analysis_input(budget)?)?;
        }
        let last = input["experiences"]
            .as_array()
            .expect("experiences")
            .last()
            .expect("evidence")["id"]
            .as_str()
            .expect("id");
        let cursor = self.records[self.ids[last]].2;
        Ok((serde_json::to_string(&input)?, cursor))
    }

    pub(crate) fn complete_analysis(&mut self, cursor: u64) -> Result<()> {
        ensure!(
            cursor >= self.ledger.processed_cursor && cursor <= self.ledger.observed_cursor,
            "invalid analysis cursor"
        );
        let previous = self.ledger.clone();
        self.ledger.processed_cursor = cursor;
        self.ledger.pending =
            self.records.len() - self.records.partition_point(|(_, _, end)| *end <= cursor);
        if let Err(error) = self.save() {
            self.ledger = previous;
            return Err(error);
        }
        Ok(())
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
    #[allow(clippy::needless_pass_by_value)] // Keep the owned observation handoff API.
    pub fn record(&mut self, experience: Experience) -> Result<()> {
        ensure!(experience.project == self.project, "wrong project");
        self.load_records()?;
        self.observe()?;
        if self.ids.contains_key(&experience.id) {
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
        crate::storage::append_experience(&self.root.join("experiences.jsonl"), &experience)?;
        self.load_records()?;
        self.observe()
    }
    #[must_use]
    pub fn due(&self, ended: bool, time: u64) -> bool {
        self.config.enabled
            && self.ledger.pending > 0
            && (ended || self.ledger.pending >= self.config.batch_size)
            && time.saturating_sub(self.ledger.last_analysis) >= self.config.cooldown_secs
    }
    pub(crate) fn path(&self, name: &str, state: State) -> Result<PathBuf> {
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
    pub(crate) fn evidence<'a>(&'a self, ids: &[String]) -> Result<Vec<&'a Experience>> {
        let unique: std::collections::BTreeSet<_> = ids.iter().collect();
        ensure!(
            unique.len() == ids.len() && !ids.is_empty(),
            "invalid evidence"
        );
        ids.iter()
            .map(|id| {
                self.recent
                    .iter()
                    .find(|e| &e.id == id && e.project == self.project)
                    .ok_or_else(|| anyhow::anyhow!("unknown evidence"))
            })
            .collect()
    }
    pub(crate) fn evidence_gate(&self, ids: &[String], confidence: f64, _time: u64) -> Result<f64> {
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
    pub(crate) fn write_package(
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
    pub(crate) fn transition(&mut self, name: &str, next: State) -> Result<()> {
        self.owned(name)?;
        let previous = self.ledger.skills[name].state;
        ensure!(
            policy::transition_allowed(previous, next),
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
                    for experience in &mut self.recent {
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
                let next = policy::promoted_state(meta)?;
                self.transition(&name, next)?;
            }
            Action::Retire { name } => {
                self.owned(&name)?;
                let meta = &self.ledger.skills[&name];
                self.evidence_gate(&meta.evidence, meta.confidence, time)?;
                let next = policy::retired_state(meta, self.ledger.epoch);
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
                let store = memory::MemoryStore::open(&self.database)
                    .context(crate::storage::PersistenceFailure)?;
                let scope = memory::MemoryScope::Project;
                let existing = store
                    .scoped_memories(scope, &self.project)
                    .context(crate::storage::PersistenceFailure)?
                    .into_iter()
                    .find(|m| m.key == key);
                ensure!(
                    existing.as_ref().is_none_or(|m| m.source == "evolved"),
                    "user memory protected"
                );
                store
                    .change_fact(
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
                    )
                    .context(crate::storage::PersistenceFailure)?;
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
            &serde_json::json!({"experiences": self.recent, "metadata": self.ledger.skills, "packages": packages, "config": self.config}),
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
