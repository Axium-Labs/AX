//! ACP session/activity bridge. Sessions and permission decisions remain owned by AX.
use crate::distributed_client::Client;
use anyhow::{Result, anyhow, ensure};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::Path,
    time::{Duration, Instant},
};
use tokio::{
    io::{BufReader, Lines},
    process::{ChildStdin, ChildStdout},
};

pub(crate) struct Bridge<'a> {
    pub client: &'a Client,
    pub source: Value,
    pub session: Value,
    pub session_options: Value,
    pub root: &'a Path,
}
impl Bridge<'_> {
    async fn report(&self, kind: &str, value: &Value) -> Result<()> {
        let mut body = self.source.clone();
        body["message_id"] = json!(uuid::Uuid::new_v4().to_string());
        body["kind"] = json!(kind);
        body["text"] = json!(value.to_string());
        self.client
            .request("POST", "/api/distributed/worker/report", Some(body))
            .await?;
        Ok(())
    }
    async fn ack(&self, command: &Value, status: &str, result: &str) -> Result<()> {
        self.client.request("POST", &format!("/api/distributed/worker/commands/{}", command["id"].as_str().unwrap_or_default()), Some(json!({"source":self.source,"status":status,"result":result.chars().take(16000).collect::<String>()}))).await?;
        Ok(())
    }
    /// A single ACP connection executes one turn. Guidance uses AX's existing steer seam.
    #[allow(clippy::too_many_lines)]
    pub(crate) async fn drive(
        &self,
        input: &mut ChildStdin,
        lines: &mut Lines<BufReader<ChildStdout>>,
    ) -> Result<(String, Option<String>)> {
        self.report("session", &json!({"session_id":self.session["sessionId"],"model":self.session["_crewModel"],"model_source":"execution ACP configuration","sandbox_source":"host policy"})).await?;
        let mut session_id = self.session["sessionId"].clone();
        let mut bound_session = session_id.clone();
        let mut binding = None;
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        let mut output = String::new();
        let mut batch = vec![];
        let mut permission: Option<(Value, Value, Instant)> = None;
        let mut requests: BTreeMap<u64, Value> = BTreeMap::new();
        let mut next_id = 100_u64;
        let mut current_turn = 3_u64;
        let mut finished = None;
        let mut waiting_for_user = false;
        let mut pending_question = false;
        let mut answering_question = false;
        let mut goal_state = String::new();
        let mut question = Value::Null;
        let mut turn_output = String::new();
        loop {
            tokio::select! {
                line = lines.next_line() => {
                    let line = line?.ok_or_else(|| anyhow!("ACP closed before result"))?;
                    let value: Value = serde_json::from_str(&line)?;
                    if value["method"] == "session/request_permission" {
                        self.report("activity", &json!({"session_id":session_id,"permission":value["params"],"request_id":value["id"]})).await?;
                        permission = Some((value["id"].clone(), value["params"]["options"].clone(), Instant::now() + Duration::from_secs(90)));
                    } else if value["method"] == "session/update" {
                        let update = &value["params"]["update"];
                        if let Some(state) = update["_meta"]["axGoal"]["goal"]["state"].as_str() { state.clone_into(&mut goal_state); }
                        if update["sessionUpdate"] == "agent_question" { question = update["question"].clone(); }
                        if let Some(text) = update["content"]["text"].as_str().filter(|_| update["sessionUpdate"] == "agent_message_chunk") {
                            output.push_str(text); turn_output.push_str(text);
                        }
                        if let Some(session) = update["_meta"]["axSessionRotation"]["session"].as_str() {
                            session_id = json!(session);
                            self.report("session", &json!({"session_id":session,"rotation":update["_meta"]["axSessionRotation"]})).await?;
                        }
                        let update = if update.to_string().len() > 16000 {
                            json!({"sessionUpdate":update["sessionUpdate"],"toolCallId":update["toolCallId"],"title":update["title"],"status":update["status"],"content":{"type":"text","text":update["content"]["text"].as_str().unwrap_or_default().chars().take(8000).collect::<String>()},"detail_truncated":true})
                        } else { update.clone() };
                        batch.push(json!({"session_id":session_id,"update":update}));
                        if batch.len() >= 32 { self.report("activity", &json!(batch)).await?; batch.clear(); }
                    } else if let Some(id) = value["id"].as_u64() {
                        if binding == Some(id) {
                            ensure!(value["error"].is_null(), "rotated ACP Session binding failed: {}",value["error"]);
                            binding = None; bound_session = session_id.clone();
                        }
                        let native_response = requests.get(&id).is_some_and(|c| !["prompt", "answer"].contains(&c["kind"].as_str().unwrap_or_default()));
                        if let Some(command) = requests.remove(&id) {
                            let ok = value["error"].is_null();
                            self.ack(&command, if ok {"completed"} else {"failed"}, &if ok {turn_output.clone()} else {value["error"].to_string()}).await?;
                        }
                        if id == current_turn {
                            answering_question = false;
                            if !value["error"].is_null() && pending_question { waiting_for_user = true; continue; }
                            let failure = if native_response { None } else if !value["error"].is_null() { Some(format!("ACP execution error: {}", value["error"])) }
                                else if value["result"]["stopReason"] != "end_turn" { Some("AX execution cancelled".into()) } else { None };
                            // Keep the same Session open for an explicit answer to AX's pending question.
                            pending_question = value["result"]["_meta"]["axGoal"]["waiting_for_user"] == true;
                            waiting_for_user = pending_question || matches!(goal_state.as_str(), "paused" | "blocked");
                            if !waiting_for_user { finished = Some((Instant::now(), failure)); }
                            if session_id != bound_session {
                                next_id += 1; binding = Some(next_id);
                                crate::distributed_worker::send(input,json!({"jsonrpc":"2.0","id":next_id,"method":"session/resume","params":{"sessionId":session_id,"cwd":self.root,"mcpServers":[],"_ax":self.session_options}})).await?;
                            }
                        }
                    }
                    ensure!(output.len() <= 8 * 1024 * 1024, "output exceeds artifact limit");
                }
                _ = tick.tick() => {
                    if !batch.is_empty() { self.report("activity", &json!(batch)).await?; batch.clear(); }
                    if binding.is_some() { continue; }
                    let commands = self.client.request("POST", "/api/distributed/worker/inbox", Some(self.source.clone())).await?;
                    for command in commands.as_array().into_iter().flatten() {
                        if command["session_id"] != session_id { self.ack(command, "failed", "Session rotated; send to the current active Session").await?; continue; }
                        let kind = command["kind"].as_str().unwrap_or_default();
                        let text = command["text"].as_str().unwrap_or_default();
                        if kind == "permission" {
                            if let Some((id, options, deadline)) = permission.take() {
                                if command["request_id"].as_str() == Some(id.to_string().trim_matches('"')) && options.as_array().is_some_and(|a| a.iter().any(|o| o["optionId"] == text)) {
                                    crate::distributed_worker::send(input, json!({"jsonrpc":"2.0","id":id,"result":{"outcome":{"outcome":"selected","optionId":text}}})).await?;
                                    self.ack(command, "completed", "AX accepted one-request permission decision").await?;
                                } else { permission = Some((id, options, deadline)); self.ack(command, "failed", "permission option not offered by AX").await?; }
                            } else { self.ack(command, "failed", "permission request expired or absent").await?; }
                            continue;
                        }
                        let idle = finished.is_some() || waiting_for_user;
                        if !["prompt", "answer"].contains(&kind) && !idle { continue; } // Native management waits for a safe turn boundary.
                        if kind == "prompt" && pending_question && answering_question { self.ack(command,"failed","question answer is already in progress").await?; continue; }
                        let prompt = match kind {
                            "prompt" => text.to_owned(),
                            "answer" => {
                                let answer: Value = serde_json::from_str(text)?;
                                if answer["question_id"] != question["id"] || !pending_question || answering_question || answer["option_id"].as_str().is_some_and(|selected| !question["options"].as_array().is_some_and(|options| options.iter().any(|option| option["id"] == selected))) { self.ack(command,"failed","AX question changed or already answered").await?; continue; }
                                answer["option_id"].as_str().or_else(|| answer["text"].as_str()).unwrap_or_default().to_owned()
                            },
                            "project_read" => "/goal project show".into(),
                            "project_history" => format!("/goal project history {text}"),
                            "project_config" => {
                                let config: Value = serde_json::from_str(text)?;
                                format!("/goal project {config}")
                            },
                            "rotate" => "/goal rotate".into(),
                            "project_import" => {
                                // Fixed execution-local destination, never a user-supplied machine path.
                                let path = self.root.join(".distributed-inputs").join(format!("{}.md", command["id"].as_str().unwrap_or_default()));
                                tokio::fs::write(&path, text).await?;
                                format!("/goal project import {}", path.display())
                            },
                            _ => { self.ack(command, "failed", "unsupported command").await?; continue; }
                        };
                        self.ack(command, "executing", "delivered to existing ACP Session").await?;
                        next_id += 1;
                        let id = next_id;
                        let method = if idle { current_turn = id; finished = None; waiting_for_user = false; turn_output.clear(); "session/prompt" } else { "_ax/steer" };
                        let mut params = json!({"sessionId":session_id,"prompt":[{"type":"text","text":prompt}]});
                        if method == "session/prompt" && ["prompt", "answer"].contains(&kind) && (pending_question || (!goal_state.is_empty() && !["completed", "cancelled", "failed"].contains(&goal_state.as_str()))) {
                            params["_meta"] = if pending_question {
                                let answer = if kind == "answer" { serde_json::from_str::<Value>(text)? } else { json!({"question_id":question["id"],"text":text}) };
                                json!({"axGoal":{"action":"answer","goal_id":self.source["task_id"],"answer":answer}})
                            } else {json!({"axGoal":{"action":"resume","goal_id":self.source["task_id"]}})};
                        }
                        if params["_meta"]["axGoal"]["action"] == "answer" { answering_question = true; }
                        crate::distributed_worker::send(input, json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})).await?;
                        requests.insert(id, command.clone());
                    }
                    if let Some((id, _, deadline)) = &permission && Instant::now() >= *deadline {
                        crate::distributed_worker::send(input, json!({"jsonrpc":"2.0","id":id,"result":{"outcome":{"outcome":"selected","optionId":"reject_once"}}})).await?;
                        self.report("activity", &json!({"session_id":session_id,"request_id":id,"permission_expired":true})).await?;
                        permission = None;
                    }
                    if let Some((deadline, _)) = &finished && deadline.elapsed() >= Duration::from_secs(3) && requests.is_empty() { break; }
                }
            }
        }
        if !batch.is_empty() {
            self.report("activity", &json!(batch)).await?;
        }
        Ok((output, finished.and_then(|(_, failure)| failure)))
    }
}
