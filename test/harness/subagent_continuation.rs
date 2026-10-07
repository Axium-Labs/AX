use super::*;

#[tokio::test]
async fn unfinished_subagent_waits_then_result_is_consumed_before_direct_final() {
    let (mut kernel, provider, _, _) = fixture(1);
    let mut receiver = kernel.prepare_subagents();
    let manager = kernel.subagent_manager.as_ref().unwrap().clone();
    manager
        .spawn_agent("child-work", SpawnOptions::default())
        .unwrap();
    assert_eq!(
        kernel.turn_state().continuation(),
        crate::TurnContinuation::Wait(crate::WaitReason::Child)
    );
    let events = Mutex::new(Vec::new());
    let emit = Mutex::new(|event| events.lock().unwrap().push(event));
    let mut calls = 0;
    let mut steps = 1;
    let result = kernel
        .tool_step(
            &emit,
            &mut |_| Ok(()),
            "early".into(),
            vec![],
            false,
            &mut calls,
            &mut steps,
            &mut receiver,
        )
        .await
        .unwrap();
    assert!(matches!(result, crate::loop_runtime::ToolStep::Continue));
    assert_eq!(
        kernel.turn_state().continuation(),
        crate::TurnContinuation::Continue(crate::ContinuationReason::ChildResult)
    );
    assert!(
        !events
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, AgentEvent::TurnFinished))
    );
    let step = kernel
        .model_step(&emit, &[], steps, &mut |_| Ok(()))
        .await
        .unwrap();
    assert!(
        provider
            .requests
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .messages
            .iter()
            .any(|m| m.content.starts_with("[ax-child-result]"))
    );
    let result = kernel
        .tool_step(
            &emit,
            &mut |_| Ok(()),
            step.content,
            step.tool_calls,
            false,
            &mut calls,
            &mut steps,
            &mut receiver,
        )
        .await
        .unwrap();
    assert!(matches!(result, crate::loop_runtime::ToolStep::Final(_)));
    assert_eq!(
        provider.requests.lock().unwrap().len(),
        2,
        "one worker request and one parent request; no reviewer"
    );
}

#[tokio::test]
async fn completed_unconsumed_subagent_still_requires_follow_up() {
    let (mut kernel, _, _, _) = fixture(1);
    let _receiver = kernel.prepare_subagents();
    let manager = kernel.subagent_manager.as_ref().unwrap().clone();
    manager
        .spawn_agent("child-work", SpawnOptions::default())
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while manager.continuation_counts().0 > 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        kernel.turn_state().continuation(),
        crate::TurnContinuation::Continue(crate::ContinuationReason::ChildResult)
    );
}
