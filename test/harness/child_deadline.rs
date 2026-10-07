use super::*;

struct DeadlineHost(Arc<MockHost>);

#[async_trait]
impl ChildHost for DeadlineHost {
    async fn prepare(
        &self,
        controller: &AgentKernel,
        input: &str,
        previous: Option<&ChildRun>,
    ) -> Result<PreparedChild, AgentError> {
        let mut child = self.0.prepare(controller, input, previous).await?;
        child.kernel.budget.turn_timeout_secs = 1;
        child
            .run
            .execution_budget
            .as_mut()
            .unwrap()
            .turn_timeout_secs = 1;
        Ok(child)
    }
}

#[tokio::test]
async fn active_child_cannot_refresh_its_wall_deadline_and_next_item_continues() {
    let fixture = harness_with(
        vec![
            call(
                "plan",
                "task_queue",
                json!({"action":"start","execution":"children","overall_goal":"two independent tasks","tasks":[{"title":"loop","input":"loop"},{"title":"next","input":"next"}]}),
            ),
            plain("all settled"),
        ],
        Duration::from_millis(100),
        1,
    );
    fixture.host.push_script(
        (0..50)
            .map(|index| call(&format!("activity-{index}"), "always_fail", json!({})))
            .collect(),
    );
    fixture.host.push_script(vec![plain("next completed")]);
    let receipts = Arc::clone(&fixture.host.receipts);
    let mut kernel = fixture
        .kernel
        .with_child_host(Arc::new(DeadlineHost(fixture.host)));
    tokio::time::timeout(
        // The one-time environment probe on the first turn of a test process
        // shares this wall clock with the child deadline.
        Duration::from_secs(20),
        kernel.run_turn("execute both", |_| {}),
    )
    .await
    .expect("active child is bounded")
    .unwrap();
    let receipts = receipts.lock().unwrap();
    assert_eq!(receipts.len(), 2);
    assert_eq!(receipts[0].status, ChildStatus::TimedOut);
    assert!(
        receipts[0].metrics.tool_calls >= 2,
        "ongoing activity cannot renew the child deadline"
    );
    assert_eq!(receipts[1].status, ChildStatus::Completed);
    assert_eq!(
        kernel.task_queue().unwrap().tasks[1].status,
        TaskStatus::Completed
    );
}
