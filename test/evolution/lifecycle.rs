use std::{fs, path::PathBuf};

use super::*;
use crate::storage::screen;

struct Fixture {
    root: PathBuf,
    engine: Engine,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("ax-evolution-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let engine = Engine::open(
            root.join("evolution"),
            root.join("memory.sqlite3"),
            "project-a".into(),
        )
        .unwrap();
        Self { root, engine }
    }
    fn stable(&mut self) -> Vec<String> {
        (0..6)
            .map(|i| {
                let e = experience(i);
                let id = e.id.clone();
                self.engine.record(e).unwrap();
                id
            })
            .collect()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}
fn experience(i: usize) -> Experience {
    Experience {
        id: format!("e-{i}"),
        task: "Build release artifacts with cargo, then verify the binary".into(),
        intent: "Build release artifacts".into(),
        tools_used: vec!["shell".into()],
        skills_used: vec![],
        steps: vec![Step {
            tool: "shell".into(),
            detail: "cargo build --release".into(),
            success: Some(true),
        }],
        errors: vec![],
        retries: 0,
        user_corrections: vec![],
        success: true,
        project: "project-a".into(),
        session: format!("s-{i}"),
        at: 100_000,
    }
}
fn create(name: &str, evidence: Vec<String>) -> Action {
    Action::Create {
        name: name.into(),
        description: "Build release artifacts with cargo and verify the binary".into(),
        instructions: "Run cargo build --release. Check the resulting binary with --version."
            .into(),
        evidence,
        confidence: 1.0,
    }
}

#[test]
fn recording_is_bounded_durable_and_deduplicated() {
    let mut f = Fixture::new();
    f.engine.config.max_experiences = 3;
    for i in 0..6 {
        f.engine.record(experience(i)).unwrap();
    }
    f.engine.record(experience(5)).unwrap();
    assert_eq!(f.engine.recent.len(), 3);
    assert_eq!(f.engine.ledger.pending, 6);
    assert_eq!(
        fs::read_to_string(f.engine.root.join("experiences.jsonl"))
            .unwrap()
            .lines()
            .count(),
        6
    );
    let reopened = Engine::open(
        f.engine.root.clone(),
        f.engine.database.clone(),
        f.engine.project.clone(),
    )
    .unwrap();
    assert_eq!(reopened.ledger.pending, 6);
    assert!(reopened.due(true, 100_000));
    assert!(!reopened.due(false, 100_000));
}

#[test]
fn one_off_low_confidence_failed_and_cross_project_evidence_cannot_create() {
    let mut f = Fixture::new();
    f.engine.record(experience(0)).unwrap();
    assert!(
        f.engine
            .apply(create("release-check", vec!["e-0".into()]), 100_000)
            .is_err()
    );
    let mut wrong = experience(1);
    wrong.project = "other".into();
    assert!(f.engine.record(wrong).is_err());
    let ids = f.stable();
    for e in &mut f.engine.recent {
        e.success = false;
    }
    // Failed outcomes can justify an explicit model proposal for a corrective lesson.
    f.engine
        .apply(create("failure-lesson", ids.clone()), 100_000)
        .unwrap();
    for e in &mut f.engine.recent {
        e.success = true;
    }
    assert!(f.engine.evidence_gate(&ids, 0.0, 100_000).unwrap() < f.engine.config.create_score);
    assert!(
        f.engine
            .evidence_gate(&["invented".into()], 1.0, 100_000)
            .is_err()
    );
    assert!(
        f.engine
            .evidence_gate(&[ids[0].clone(), ids[0].clone()], 1.0, 100_000)
            .is_err()
    );
}

#[test]
fn lifecycle_routes_only_trial_and_active_and_requires_real_trial_outcomes() {
    let mut f = Fixture::new();
    let ids = f.stable();
    f.engine
        .apply(create("release-check", ids), 100_000)
        .unwrap();
    assert_eq!(
        f.engine.ledger.skills["release-check"].state,
        State::Candidate
    );
    assert!(
        skill::SkillCatalog::index(f.engine.root.join("live"))
            .unwrap()
            .is_empty()
    );
    f.engine.maintain(100_000).unwrap(); // same epoch cannot promote
    assert_eq!(
        f.engine.ledger.skills["release-check"].state,
        State::Candidate
    );
    f.engine.ledger.epoch += 1;
    f.engine
        .apply(
            Action::Promote {
                name: "release-check".into(),
                evidence: f.engine.ledger.skills["release-check"].evidence.clone(),
            },
            100_000,
        )
        .unwrap();
    assert_eq!(f.engine.ledger.skills["release-check"].state, State::Trial);
    assert_eq!(
        skill::SkillCatalog::index(f.engine.root.join("live"))
            .unwrap()
            .len(),
        1
    );
    f.engine.ledger.epoch += 1;
    f.engine.maintain(100_000).unwrap();
    assert_eq!(f.engine.ledger.skills["release-check"].state, State::Trial);
    for i in 6..12 {
        let mut e = experience(i);
        e.skills_used = vec!["release-check".into()];
        f.engine.record(e).unwrap();
    }
    f.engine
        .apply(
            Action::Promote {
                name: "release-check".into(),
                evidence: f.engine.ledger.skills["release-check"].evidence.clone(),
            },
            100_000,
        )
        .unwrap();
    assert_eq!(f.engine.ledger.skills["release-check"].state, State::Active);
    assert_eq!(f.engine.ledger.skills["release-check"].success_count, 6);
    let text = f.engine.owned("release-check").unwrap();
    assert!(!text.contains("source:") && !text.contains("confidence:") && !text.contains("state:"));
    assert!(
        f.engine
            .transition("release-check", State::Deleted)
            .is_err()
    );
}

#[test]
fn refinement_records_corrections_and_retrials_without_resetting_lifetime_counts() {
    let mut f = Fixture::new();
    let ids = f.stable();
    f.engine
        .apply(create("release-check", ids.clone()), 100_000)
        .unwrap();
    f.engine.ledger.epoch = 1;
    f.engine
        .apply(
            Action::Promote {
                name: "release-check".into(),
                evidence: f.engine.ledger.skills["release-check"].evidence.clone(),
            },
            100_000,
        )
        .unwrap();
    for i in 6..12 {
        let mut e = experience(i);
        e.skills_used.push("release-check".into());
        f.engine.record(e).unwrap();
    }
    f.engine.ledger.epoch = 2;
    f.engine
        .apply(
            Action::Promote {
                name: "release-check".into(),
                evidence: f.engine.ledger.skills["release-check"].evidence.clone(),
            },
            100_000,
        )
        .unwrap();
    f.engine.apply(Action::Refine { name: "release-check".into(), description: "Build and validate release artifacts".into(), instructions: "Build release artifacts with cargo, then verify the binary. Use --version before packaging.".into(), evidence: ids, confidence: 1.0, corrections: vec!["verify the binary".into()] }, 100_000).unwrap();
    let meta = &f.engine.ledger.skills["release-check"];
    assert_eq!(meta.state, State::Trial);
    assert_eq!(meta.use_count, 6);
    assert_eq!(meta.corrections, 1);
    assert!(
        f.engine.recent[0]
            .user_corrections
            .contains(&"verify the binary".into())
    );
    f.engine.ledger.epoch = 3;
    f.engine.maintain(100_000).unwrap();
    assert_eq!(f.engine.ledger.skills["release-check"].state, State::Trial);
}

#[test]
fn duplicate_creation_is_rejected_merge_compresses_and_archives_sources() {
    let mut f = Fixture::new();
    let ids = f.stable();
    f.engine
        .apply(create("release-check", ids.clone()), 100_000)
        .unwrap();
    // Similarity is a model hint, not a veto over an explicit merge proposal.
    // Simulate a historical duplicate from a previous analyzer configuration.
    let mut meta = f.engine.ledger.skills["release-check"].clone();
    meta.digest = f
        .engine
        .write_package(
            "release-copy",
            "Build release artifacts",
            "Run cargo build --release. Check the resulting binary with --version.",
            State::Candidate,
            false,
        )
        .unwrap();
    f.engine.ledger.skills.insert("release-copy".into(), meta);
    f.engine.config.max_skills = 2;
    f.engine
        .apply(
            Action::Merge {
                names: vec!["release-check".into(), "release-copy".into()],
                name: "release-workflow".into(),
                description: "Build and verify release artifacts".into(),
                instructions: "Build with cargo --release; verify the binary's --version.".into(),
                evidence: ids,
                confidence: 1.0,
            },
            100_000,
        )
        .unwrap();
    assert_eq!(
        f.engine.ledger.skills["release-workflow"].state,
        State::Candidate
    );
    for name in ["release-check", "release-copy"] {
        assert_eq!(f.engine.ledger.skills[name].state, State::Archived);
        assert!(f.engine.owned(name).is_ok());
    }
}

#[test]
fn low_value_archive_precedes_deletion_and_preserves_experience_history() {
    let mut f = Fixture::new();
    let ids = f.stable();
    f.engine
        .apply(create("release-check", ids), 100_000)
        .unwrap();
    let old = 100_000 + 365 * 86400;
    f.engine.ledger.epoch = 1;
    f.engine.maintain(old).unwrap();
    assert_eq!(
        f.engine.ledger.skills["release-check"].state,
        State::Candidate
    );
    f.engine
        .apply(
            Action::Retire {
                name: "release-check".into(),
            },
            old,
        )
        .unwrap();
    assert_eq!(
        f.engine.ledger.skills["release-check"].state,
        State::Archived
    );
    f.engine.ledger.epoch = 2;
    f.engine
        .apply(
            Action::Retire {
                name: "release-check".into(),
            },
            old,
        )
        .unwrap();
    assert_eq!(
        f.engine.ledger.skills["release-check"].state,
        State::Deleted
    );
    assert_eq!(
        fs::read_to_string(f.engine.root.join("experiences.jsonl"))
            .unwrap()
            .lines()
            .count(),
        6
    );
}

#[test]
fn explicit_sources_manual_edits_extra_resources_and_paths_are_protected() {
    let mut f = Fixture::new();
    let ids = f.stable();
    assert!(
        f.engine
            .apply(create("../escape", ids.clone()), 100_000)
            .is_err()
    );
    f.engine
        .apply(create("release-check", ids), 100_000)
        .unwrap();
    f.engine
        .ledger
        .skills
        .get_mut("release-check")
        .unwrap()
        .source = "user".into();
    assert!(f.engine.owned("release-check").is_err());
    f.engine
        .ledger
        .skills
        .get_mut("release-check")
        .unwrap()
        .source = "evolved".into();
    let path = f.engine.path("release-check", State::Candidate).unwrap();
    let original = fs::read(path.join("SKILL.md")).unwrap();
    fs::write(path.join("SKILL.md"), "hand edited").unwrap();
    assert!(f.engine.owned("release-check").is_err());
    fs::write(path.join("SKILL.md"), original).unwrap();
    f.engine
        .transition("release-check", State::Archived)
        .unwrap();
    let path = f.engine.path("release-check", State::Archived).unwrap();
    fs::write(path.join("user-notes.txt"), "keep this").unwrap();
    assert!(
        f.engine
            .transition("release-check", State::Deleted)
            .is_err()
    );
    assert!(path.join("user-notes.txt").exists());
}

#[test]
fn memory_classification_reuses_scoped_store_and_never_overwrites_user_facts() {
    let mut f = Fixture::new();
    let ids = f.stable();
    let action = || Action::Memory {
        key: "build.workflow".into(),
        value: "Build release artifacts with cargo".into(),
        evidence: ids.clone(),
        quote: "Build release artifacts with cargo".into(),
        confidence: 1.0,
    };
    f.engine.apply(action(), 100_000).unwrap();
    let store = memory::MemoryStore::open(&f.engine.database).unwrap();
    let mut record = store
        .scoped_memories(memory::MemoryScope::Project, "project-a")
        .unwrap()
        .remove(0);
    assert_eq!(record.source, "evolved");
    record.source = "user/session:s1".into();
    record.value = "user override".into();
    store.remember_scoped(&record).unwrap();
    assert!(f.engine.apply(action(), 100_000).is_err());
    assert_eq!(
        store
            .scoped_memories(memory::MemoryScope::Project, "project-a")
            .unwrap()[0]
            .value,
        "user override"
    );
    let bad = Action::Memory {
        key: "other".into(),
        value: "invented".into(),
        evidence: ids,
        quote: "never said".into(),
        confidence: 1.0,
    };
    assert!(f.engine.apply(bad, 100_000).is_err());
}

#[test]
fn cooldown_configuration_context_fit_and_credential_filtering() {
    let mut f = Fixture::new();
    f.stable();
    f.engine.ledger.last_analysis = 100_000;
    assert!(!f.engine.due(true, 100_001));
    assert!(f.engine.due(true, 100_000 + f.engine.config.cooldown_secs));
    let input = f.engine.bounded_analysis_input(5000).unwrap();
    assert!(input.len() <= 5000);
    assert!(f.engine.bounded_analysis_input(1).is_err());
    assert!(screen(&"valid workflow instructions ".repeat(80)).is_ok());
    assert!(screen(&format!("{}password=secret", "a".repeat(995))).is_err());
    let mut e = experience(99);
    e.task = "api_key=123".into();
    assert!(f.engine.record(e).is_err());
    let invalid = Config {
        half_life_secs: 0.0,
        ..Config::default()
    };
    assert!(invalid.validate().is_err());
    assert!(serde_json::from_str::<Action>(r#"{"action":"CREATE","path":"../../core"}"#).is_err());
}

#[test]
fn retained_package_cap_and_tombstone_compaction_bound_long_term_growth() {
    let mut f = Fixture::new();
    let ids = f.stable();
    f.engine
        .apply(create("release-check", ids.clone()), 100_000)
        .unwrap();
    f.engine.config.max_retained_skills = 1;
    assert!(
        f.engine
            .apply(create("other-workflow", ids), 100_000)
            .is_err()
    );
    let mut deleted = f.engine.ledger.skills["release-check"].clone();
    deleted.state = State::Deleted;
    for i in 0..5 {
        deleted.epoch = i;
        f.engine
            .ledger
            .skills
            .insert(format!("deleted-{i}"), deleted.clone());
    }
    f.engine.config.max_tombstones = 2;
    f.engine.maintain(100_000).unwrap();
    assert_eq!(
        f.engine
            .ledger
            .skills
            .values()
            .filter(|m| m.state == State::Deleted)
            .count(),
        2
    );
    assert!(f.engine.ledger.skills.contains_key("deleted-4"));
}

struct Analyzer {
    calls: std::sync::atomic::AtomicUsize,
    response: String,
    delay: bool,
}
#[async_trait::async_trait]
impl model::ModelProvider for Analyzer {
    fn name(&self) -> &'static str {
        "test"
    }
    fn model_id(&self) -> &'static str {
        "test"
    }
    fn context_window(&self) -> usize {
        32_000
    }
    async fn complete(
        &self,
        request: model::ModelRequest,
    ) -> Result<model::ModelResponse, model::ModelError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        assert!(request.tools.is_empty());
        assert!(request.messages[0].content.contains(CREATOR));
        if self.delay {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
        Ok(model::ModelResponse {
            content: self.response.clone(),
            tool_calls: vec![],
            finish_reason: Some("stop".into()),
            usage: None,
        })
    }
}

#[tokio::test]
async fn worker_records_without_model_calls_then_analyzes_at_session_end() {
    let f = Fixture::new();
    let ids = (0..6).map(|i| format!("e-{i}")).collect();
    let analyzer = std::sync::Arc::new(Analyzer {
        calls: std::sync::atomic::AtomicUsize::new(0),
        response: serde_json::to_string(&vec![create("release-check", ids)]).unwrap(),
        delay: false,
    });
    let handle = start(
        f.engine.root.clone(),
        f.engine.database.clone(),
        f.engine.project.clone(),
        analyzer.clone(),
        vec![],
    );
    for i in 0..6 {
        let mut e = experience(i);
        e.at = now();
        handle.record(e);
    }
    assert_eq!(analyzer.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    handle.finish().await;
    assert_eq!(analyzer.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    let engine = Engine::open(
        f.engine.root.clone(),
        f.engine.database.clone(),
        f.engine.project.clone(),
    )
    .unwrap();
    assert_eq!(
        engine.ledger.skills["release-check"].state,
        State::Candidate
    );
    assert_eq!(engine.ledger.pending, 0);
    assert_eq!(engine.recent.len(), 6);
}

#[tokio::test]
async fn malformed_and_timeout_analysis_keep_evidence_and_obey_cooldown() {
    for delay in [false, true] {
        let f = Fixture::new();
        let config = Config {
            analysis_timeout_secs: 1,
            ..Config::default()
        };
        fs::write(
            f.engine.root.join("config.json"),
            serde_json::to_vec(&config).unwrap(),
        )
        .unwrap();
        let analyzer = std::sync::Arc::new(Analyzer {
            calls: std::sync::atomic::AtomicUsize::new(0),
            response: "invalid JSON".into(),
            delay,
        });
        let handle = start(
            f.engine.root.clone(),
            f.engine.database.clone(),
            f.engine.project.clone(),
            analyzer.clone(),
            vec![],
        );
        let mut e = experience(0);
        e.at = now();
        handle.record(e);
        handle.finish().await;
        let engine = Engine::open(
            f.engine.root.clone(),
            f.engine.database.clone(),
            f.engine.project.clone(),
        )
        .unwrap();
        assert_eq!(engine.ledger.pending, 1);
        assert_eq!(engine.ledger.processed_cursor, 0);
        assert_eq!(engine.recent.len(), 1);
        assert!(!engine.due(true, now()));
        assert!(engine.ledger.skills.is_empty());
    }
}

#[tokio::test]
async fn protected_names_cannot_be_created_and_disabled_evolution_never_calls_model() {
    for enabled in [false, true] {
        let f = Fixture::new();
        fs::write(
            f.engine.root.join("config.json"),
            serde_json::to_vec(&Config {
                enabled,
                ..Config::default()
            })
            .unwrap(),
        )
        .unwrap();
        let ids = (0..6).map(|i| format!("e-{i}")).collect();
        let analyzer = std::sync::Arc::new(Analyzer {
            calls: std::sync::atomic::AtomicUsize::new(0),
            response: serde_json::to_string(&vec![create("release-check", ids)]).unwrap(),
            delay: false,
        });
        let handle = start(
            f.engine.root.clone(),
            f.engine.database.clone(),
            f.engine.project.clone(),
            analyzer.clone(),
            vec!["release-check".into()],
        );
        for i in 0..6 {
            let mut e = experience(i);
            e.at = now();
            handle.record(e);
        }
        handle.finish().await;
        let engine = Engine::open(
            f.engine.root.clone(),
            f.engine.database.clone(),
            f.engine.project.clone(),
        )
        .unwrap();
        assert!(engine.ledger.skills.is_empty());
        assert_eq!(
            analyzer.calls.load(std::sync::atomic::Ordering::SeqCst),
            usize::from(enabled)
        );
    }
}

#[cfg(unix)]
#[test]
fn symlinks_never_become_mutation_territory() {
    use std::os::unix::fs::symlink;
    let mut f = Fixture::new();
    let ids = f.stable();
    symlink(&f.root, f.engine.root.join("candidate")).unwrap();
    assert!(
        f.engine
            .apply(create("release-check", ids), 100_000)
            .is_err()
    );
}

#[path = "persistence.rs"]
mod persistence;
