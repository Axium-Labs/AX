use super::*;
use serde_json::Value;

struct Fixture {
    root: PathBuf,
    engine: Option<Engine>,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("ax-r4-regression-{}", uuid::Uuid::new_v4()));
        let mut engine =
            Engine::open(root.clone(), root.join("memory.db"), "project".into()).unwrap();
        engine.config.cooldown_secs = 1;
        engine.config.batch_size = 1;
        engine.config.analysis_timeout_secs = 1;
        fs::write(
            root.join("config.json"),
            serde_json::to_vec(&engine.config).unwrap(),
        )
        .unwrap();
        Self {
            root,
            engine: Some(engine),
        }
    }

    fn engine(&mut self) -> &mut Engine {
        self.engine.as_mut().unwrap()
    }

    fn record(&mut self, id: &str) {
        self.engine()
            .record(
                serde_json::from_value(serde_json::json!({
                    "id":id,"task":"Build and verify the requested result.",
                    "tools_used":[],"skills_used":[],"steps":[],"errors":[],
                    "retries":0,"user_corrections":[],"success":true,
                    "project":"project","session":id,"at":1
                }))
                .unwrap(),
            )
            .unwrap();
    }

    fn attempt(&mut self, reply: Failure) -> Result<bool> {
        // Make each attempt eligible without sleeping through exponential cooldown.
        self.engine().ledger.last_analysis = 0;
        self.engine().save().unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let provider: Arc<dyn ModelProvider> = Arc::new(reply);
        process(
            &self.root,
            &self.root.join("memory.db"),
            "project",
            &provider,
            &[],
            runtime.handle(),
            Some(Command::End),
            &mut self.engine,
        )
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        drop(self.engine.take());
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[derive(Clone, Copy)]
enum Failure {
    Timeout,
    Provider,
}

#[async_trait::async_trait]
impl ModelProvider for Failure {
    fn name(&self) -> &'static str {
        "round4-regression"
    }
    fn model_id(&self) -> &'static str {
        "round4-regression"
    }
    fn context_window(&self) -> usize {
        32_000
    }
    async fn complete(&self, _: ModelRequest) -> Result<model::ModelResponse, model::ModelError> {
        match self {
            Self::Timeout => std::future::pending().await,
            Self::Provider => Err(model::ModelError::HttpStatus {
                status: 503,
                message: "simulated provider failure".into(),
            }),
        }
    }
}

#[test]
fn bracketed_notice_before_json_array_still_parses_actions() {
    let mut fixture = Fixture::new();
    let actions = parse_actions(
        fixture.engine(),
        "[注意] 以下为分析结果：\n[{\"action\":\"IGNORE\"}]",
    )
    .unwrap();
    assert!(matches!(actions.as_slice(), [Action::Ignore]));
}

#[test]
fn invalid_batch_retries_accumulate_across_new_experiences_and_audit_skip() {
    let mut fixture = Fixture::new();
    fixture.record("already-consumed");
    let (_, initial) = fixture.engine().prepare_analysis(16_000).unwrap();
    fixture.engine().complete_analysis(initial).unwrap();
    let start = fixture.engine().ledger.processed_cursor;
    let mut previous_end = start;
    for attempt in 1..=3 {
        fixture.record(&format!("pending-{attempt}"));
        let (_, end) = fixture.engine().prepare_analysis(16_000).unwrap();
        assert!(
            end > previous_end,
            "each append must change the selected end"
        );
        previous_end = end;
        fixture
            .engine()
            .analysis_failed(end, "invalid JSON array")
            .unwrap();
        assert_eq!(fixture.engine().ledger.failed_batch_attempts, attempt);
        assert_eq!(fixture.engine().ledger.failed_batch_cursor, start);
        assert_eq!(
            fixture.engine().ledger.processed_cursor,
            if attempt < 3 { start } else { end }
        );
        let audit: Vec<Value> = fs::read_to_string(fixture.root.join("decisions.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(
            audit
                .iter()
                .filter(|entry| entry["event"] == "ANALYSIS_SKIP")
                .count(),
            usize::from(attempt == 3)
        );
    }
    assert_eq!(fixture.engine().ledger.pending, 0);
    let restored = Engine::open(
        fixture.root.clone(),
        fixture.root.join("memory.db"),
        "project".into(),
    )
    .unwrap();
    assert_eq!(restored.ledger.processed_cursor, previous_end);
}

fn assert_three_transport_failures(reply: Failure) {
    let mut fixture = Fixture::new();
    fixture.record("pending");
    let start = fixture.engine().ledger.processed_cursor;
    let attempts = fixture.engine().ledger.failed_batch_attempts;
    for count in 1..=3 {
        let error = fixture.attempt(reply).unwrap_err();
        match reply {
            Failure::Timeout => assert!(error.is::<tokio::time::error::Elapsed>(), "{error:#}"),
            Failure::Provider => assert!(error.is::<model::ModelError>(), "{error:#}"),
        }
        assert_eq!(fixture.engine().ledger.processed_cursor, start);
        assert_eq!(fixture.engine().ledger.failed_batch_attempts, attempts);
        assert_eq!(fixture.engine().ledger.consecutive_failures, count);
        let restored = Engine::open(
            fixture.root.clone(),
            fixture.root.join("memory.db"),
            "project".into(),
        )
        .unwrap();
        assert_eq!(restored.ledger.processed_cursor, start);
        assert_eq!(restored.ledger.failed_batch_attempts, attempts);
        assert_eq!(restored.ledger.consecutive_failures, count);
    }
    assert_eq!(fixture.engine().ledger.pending, 1);
    let audit = fixture.root.join("decisions.jsonl");
    assert!(!audit.exists() || !fs::read_to_string(audit).unwrap().contains("ANALYSIS_SKIP"));
}

#[test]
fn three_provider_timeouts_only_increment_consecutive_failures() {
    assert_three_transport_failures(Failure::Timeout);
}

#[test]
fn three_provider_errors_only_increment_consecutive_failures() {
    assert_three_transport_failures(Failure::Provider);
}
