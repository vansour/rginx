use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use reqwest::{Client, RequestBuilder};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use rginx_control_types::{
    DeploymentTaskKind, NodeAgentHeartbeatRequest, NodeAgentRegistrationRequest, NodeAgentTask,
    NodeAgentTaskAckRequest, NodeAgentTaskCompleteRequest, NodeAgentTaskCompleteResponse,
    NodeAgentTaskPollRequest, NodeAgentTaskPollResponse, NodeAgentWriteResponse,
    NodeLifecycleState, NodeRuntimeReport, NodeSnapshotIngestRequest, NodeSnapshotIngestResponse,
};

use crate::config::NodeAgentConfig;

pub async fn run(config: NodeAgentConfig) -> anyhow::Result<()> {
    tracing::info!(
        node_id = %config.node_id,
        cluster_id = %config.cluster_id,
        advertise_addr = %config.advertise_addr,
        role = %config.role,
        lifecycle_state = %config.lifecycle_state.as_str(),
        control_plane_origin = %config.control_plane_origin,
        admin_socket_path = %config.admin_socket_path.display(),
        rginx_binary_path = %config.rginx_binary_path.display(),
        config_path = %config.config_path.display(),
        "node agent started"
    );

    let client = ControlPlaneClient::new(&config)?;
    let mut last_sent_snapshot_version = None;

    let initial_payload = build_payload(&config).await;
    register_once(&client, &initial_payload).await;
    send_snapshot_if_needed(&client, &initial_payload.snapshot, &mut last_sent_snapshot_version)
        .await;

    let mut heartbeat_ticker = tokio::time::interval(config.heartbeat_interval);
    heartbeat_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    heartbeat_ticker.tick().await;

    let mut task_ticker = tokio::time::interval(config.task_poll_interval);
    task_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    task_ticker.tick().await;

    loop {
        tokio::select! {
            _ = heartbeat_ticker.tick() => {
                let payload = build_payload(&config).await;
                match client.heartbeat(&payload.report).await {
                    Ok(response) => {
                        tracing::info!(
                            node_id = %response.node.node_id,
                            state = %response.node.state.as_str(),
                            snapshot_version = ?response.node.last_snapshot_version,
                            runtime_revision = ?response.node.runtime_revision,
                            status_reason = ?response.node.status_reason,
                            "node agent heartbeat accepted"
                        );
                        send_snapshot_if_needed(&client, &payload.snapshot, &mut last_sent_snapshot_version).await;
                    }
                    Err(error) => {
                        tracing::warn!(node_id = %config.node_id, error = %error, "node agent heartbeat failed");
                    }
                }
            }
            _ = task_ticker.tick() => {
                if let Err(error) = handle_task_tick(&client, &config).await {
                    tracing::warn!(node_id = %config.node_id, error = %error, "node agent task loop failed");
                }
            }
            result = tokio::signal::ctrl_c() => {
                result?;
                tracing::info!("node agent received shutdown signal");
                return Ok(());
            }
        }
    }
}

async fn register_once(client: &ControlPlaneClient, payload: &BuiltAgentPayload) {
    match client.register(&payload.report).await {
        Ok(response) => {
            tracing::info!(
                node_id = %response.node.node_id,
                state = %response.node.state.as_str(),
                snapshot_version = ?response.node.last_snapshot_version,
                runtime_revision = ?response.node.runtime_revision,
                status_reason = ?response.node.status_reason,
                "node agent registration accepted"
            );
        }
        Err(error) => {
            tracing::warn!(
                node_id = %payload.report.node_id,
                error = %error,
                "node agent registration failed"
            );
        }
    }
}

async fn send_snapshot_if_needed(
    client: &ControlPlaneClient,
    snapshot: &Option<NodeSnapshotIngestRequest>,
    last_sent_snapshot_version: &mut Option<u64>,
) {
    let Some(snapshot) = snapshot else {
        return;
    };
    if Some(snapshot.snapshot_version) == *last_sent_snapshot_version {
        return;
    }

    match client.ingest_snapshot(snapshot).await {
        Ok(response) => {
            tracing::info!(
                node_id = %response.snapshot.node_id,
                snapshot_version = response.snapshot.snapshot_version,
                captured_at_unix_ms = response.snapshot.captured_at_unix_ms,
                "node agent snapshot accepted"
            );
            *last_sent_snapshot_version = Some(response.snapshot.snapshot_version);
        }
        Err(error) => {
            tracing::warn!(
                node_id = %snapshot.node_id,
                snapshot_version = snapshot.snapshot_version,
                error = %error,
                "node agent snapshot ingest failed"
            );
        }
    }
}

async fn handle_task_tick(client: &ControlPlaneClient, config: &NodeAgentConfig) -> Result<()> {
    let response = client
        .poll_task(&NodeAgentTaskPollRequest {
            node_id: config.node_id.clone(),
            cluster_id: config.cluster_id.clone(),
        })
        .await?;
    let Some(task) = response.task else {
        return Ok(());
    };

    tracing::info!(
        task_id = %task.task_id,
        deployment_id = %task.deployment_id,
        node_id = %task.node_id,
        kind = %task.kind.as_str(),
        revision_id = %task.revision_id,
        "node agent received deployment task"
    );

    client
        .ack_task(&task.task_id, &NodeAgentTaskAckRequest { node_id: config.node_id.clone() })
        .await?;

    let execution = execute_task(config, &task).await;
    let completion = match execution {
        Ok(result) => NodeAgentTaskCompleteRequest {
            node_id: config.node_id.clone(),
            succeeded: true,
            message: Some(result.message),
            runtime_revision: result.runtime_revision,
        },
        Err(error) => {
            tracing::warn!(
                task_id = %task.task_id,
                deployment_id = %task.deployment_id,
                error = %error,
                "node agent task execution failed"
            );
            NodeAgentTaskCompleteRequest {
                node_id: config.node_id.clone(),
                succeeded: false,
                message: Some(error.to_string()),
                runtime_revision: None,
            }
        }
    };
    let completion_response = client
        .complete_task(
            &task.task_id,
            &completion,
            Some(format!(
                "task-complete:{}:{}",
                task.task_id,
                if completion.succeeded { "success" } else { "failed" }
            )),
        )
        .await?;
    log_task_completion(&task, &completion_response, completion.succeeded);

    Ok(())
}

fn log_task_completion(
    task: &NodeAgentTask,
    response: &NodeAgentTaskCompleteResponse,
    succeeded: bool,
) {
    if succeeded {
        tracing::info!(
            task_id = %task.task_id,
            deployment_id = %task.deployment_id,
            node_id = %task.node_id,
            state = %response.state.as_str(),
            "node agent completed deployment task successfully"
        );
    } else {
        tracing::warn!(
            task_id = %task.task_id,
            deployment_id = %task.deployment_id,
            node_id = %task.node_id,
            state = %response.state.as_str(),
            "node agent completed deployment task with failure"
        );
    }
}

async fn execute_task(
    config: &NodeAgentConfig,
    task: &NodeAgentTask,
) -> Result<TaskExecutionResult> {
    fs::create_dir_all(&config.config_backup_dir).with_context(|| {
        format!("failed to create backup directory {}", config.config_backup_dir.display())
    })?;
    fs::create_dir_all(&config.config_staging_dir).with_context(|| {
        format!("failed to create staging directory {}", config.config_staging_dir.display())
    })?;

    let backup_path =
        config.config_backup_dir.join(format!("{}-{}.ron", task.deployment_id, task.task_id));
    let staging_path =
        config.config_staging_dir.join(format!("{}-{}.ron", task.deployment_id, task.task_id));
    let previous_config = read_existing_config(&config.config_path)?;
    if let Some(ref contents) = previous_config {
        fs::write(&backup_path, contents)
            .with_context(|| format!("failed to write backup config {}", backup_path.display()))?;
    }

    fs::write(&staging_path, task.config_text.as_bytes())
        .with_context(|| format!("failed to write staging config {}", staging_path.display()))?;
    run_rginx_check(config, &staging_path)?;
    promote_staging_to_live(&staging_path, &config.config_path)?;

    if let Err(error) = run_rginx_reload(config, &config.config_path) {
        restore_previous_config(config, previous_config.as_deref(), &backup_path)?;
        return Err(error);
    }

    let runtime_revision = collect_runtime(&config.admin_socket_path).await.report.revision;
    Ok(TaskExecutionResult {
        message: format!(
            "{} revision `{}` applied successfully",
            match task.kind {
                DeploymentTaskKind::ApplyRevision => "deployment",
                DeploymentTaskKind::RollbackRevision => "rollback",
            },
            task.revision_id
        ),
        runtime_revision,
    })
}

fn read_existing_config(path: &Path) -> Result<Option<Vec<u8>>> {
    if path.exists() {
        Ok(Some(fs::read(path).with_context(|| format!("failed to read {}", path.display()))?))
    } else {
        Ok(None)
    }
}

fn promote_staging_to_live(staging_path: &Path, live_path: &Path) -> Result<()> {
    if let Some(parent) = live_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    fs::copy(staging_path, live_path).with_context(|| {
        format!(
            "failed to promote staging config {} to live path {}",
            staging_path.display(),
            live_path.display()
        )
    })?;
    Ok(())
}

fn restore_previous_config(
    config: &NodeAgentConfig,
    previous_config: Option<&[u8]>,
    backup_path: &Path,
) -> Result<()> {
    let Some(previous_config) = previous_config else {
        return Err(anyhow!(
            "reload failed and no previous config was available for recovery at {}",
            config.config_path.display()
        ));
    };

    fs::write(&config.config_path, previous_config).with_context(|| {
        format!(
            "failed to restore previous config from backup {} to {}",
            backup_path.display(),
            config.config_path.display()
        )
    })?;
    run_rginx_reload(config, &config.config_path).context("reload recovery attempt failed")?;
    Ok(())
}

fn run_rginx_check(config: &NodeAgentConfig, config_path: &Path) -> Result<()> {
    run_rginx_command(
        config,
        &["--config", &config_path.display().to_string(), "check"],
        "rginx check",
    )
}

fn run_rginx_reload(config: &NodeAgentConfig, config_path: &Path) -> Result<()> {
    run_rginx_command(
        config,
        &["--config", &config_path.display().to_string(), "-s", "reload"],
        "rginx reload",
    )
}

fn run_rginx_command(config: &NodeAgentConfig, args: &[&str], label: &str) -> Result<()> {
    let output =
        Command::new(&config.rginx_binary_path).args(args).output().with_context(|| {
            format!("failed to spawn {} via {}", label, config.rginx_binary_path.display())
        })?;
    if output.status.success() {
        return Ok(());
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    bail!("{} failed with status {} stdout=`{}` stderr=`{}`", label, output.status, stdout, stderr);
}

async fn build_payload(config: &NodeAgentConfig) -> BuiltAgentPayload {
    let observed_at_unix_ms = unix_time_ms(SystemTime::now());
    let runtime = collect_runtime(&config.admin_socket_path).await;
    let state = if runtime.report.error.is_some() {
        NodeLifecycleState::Drifted
    } else {
        normalize_agent_state(config.lifecycle_state)
    };

    BuiltAgentPayload {
        report: NodeAgentRegistrationRequest {
            node_id: config.node_id.clone(),
            cluster_id: config.cluster_id.clone(),
            advertise_addr: config.advertise_addr.clone(),
            role: config.role.clone(),
            running_version: runtime
                .binary_version
                .clone()
                .unwrap_or_else(|| config.running_version.clone()),
            admin_socket_path: config.admin_socket_path.display().to_string(),
            state,
            observed_at_unix_ms,
            runtime: runtime.report,
        },
        snapshot: runtime.snapshot.map(|snapshot| NodeSnapshotIngestRequest {
            node_id: config.node_id.clone(),
            cluster_id: config.cluster_id.clone(),
            observed_at_unix_ms,
            snapshot_version: snapshot.snapshot_version,
            schema_version: snapshot.schema_version,
            captured_at_unix_ms: snapshot.captured_at_unix_ms,
            pid: snapshot.pid,
            binary_version: snapshot.binary_version,
            included_modules: snapshot.included_modules,
            status: snapshot.status,
            counters: snapshot.counters,
            traffic: snapshot.traffic,
            peer_health: snapshot.peer_health,
            upstreams: snapshot.upstreams,
        }),
    }
}

fn normalize_agent_state(state: NodeLifecycleState) -> NodeLifecycleState {
    match state {
        NodeLifecycleState::Offline | NodeLifecycleState::Drifted => NodeLifecycleState::Online,
        other => other,
    }
}

async fn collect_runtime(socket_path: &Path) -> CollectedRuntime {
    if !socket_path.exists() {
        return CollectedRuntime::drift(format!(
            "admin socket not found at {}",
            socket_path.display()
        ));
    }

    match read_admin_snapshot(socket_path).await {
        Ok(snapshot) => CollectedRuntime {
            binary_version: Some(snapshot.binary_version.clone()),
            report: NodeRuntimeReport {
                snapshot_version: Some(snapshot.snapshot_version),
                revision: snapshot
                    .status
                    .as_ref()
                    .and_then(|status| status.get("revision"))
                    .and_then(Value::as_u64),
                pid: Some(snapshot.pid),
                listener_count: snapshot
                    .status
                    .as_ref()
                    .and_then(|status| status.get("listeners"))
                    .and_then(Value::as_array)
                    .and_then(|listeners| u32::try_from(listeners.len()).ok()),
                active_connections: snapshot
                    .status
                    .as_ref()
                    .and_then(|status| status.get("active_connections"))
                    .and_then(Value::as_u64)
                    .and_then(|value| u32::try_from(value).ok()),
                error: None,
            },
            snapshot: Some(snapshot),
        },
        Err(error) => CollectedRuntime::drift(error.to_string()),
    }
}

async fn read_admin_snapshot(socket_path: &Path) -> Result<AdminSnapshot> {
    let mut stream = UnixStream::connect(socket_path)
        .await
        .with_context(|| format!("failed to connect admin socket {}", socket_path.display()))?;
    let request = serde_json::to_string(&AdminSnapshotRequest {
        get_snapshot: AdminSnapshotRequestBody { include: None, window_secs: Some(60) },
    })
    .context("failed to encode admin snapshot request")?;

    stream.write_all(request.as_bytes()).await.context("failed to write admin snapshot request")?;
    stream.write_all(b"\n").await.context("failed to terminate admin snapshot request")?;
    stream.flush().await.context("failed to flush admin snapshot request")?;

    let mut response = String::new();
    let mut reader = BufReader::new(stream);
    let bytes = reader.read_line(&mut response).await.context("failed to read admin response")?;
    if bytes == 0 {
        bail!("admin socket closed without a response");
    }

    let payload: Value = serde_json::from_str(response.trim_end())
        .context("failed to decode admin response JSON")?;
    match payload
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("admin response type is missing"))?
    {
        "Snapshot" => serde_json::from_value(
            payload
                .get("data")
                .cloned()
                .ok_or_else(|| anyhow!("admin snapshot data is missing"))?,
        )
        .context("failed to decode admin snapshot payload"),
        "Error" => {
            let message = payload
                .get("data")
                .and_then(|data| data.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("unknown admin error");
            bail!("admin socket returned an error: {message}");
        }
        other => bail!("unexpected admin response type `{other}`"),
    }
}

#[derive(Debug, Clone)]
struct ControlPlaneClient {
    client: Client,
    origin: String,
    bearer_token: String,
}

impl ControlPlaneClient {
    fn new(config: &NodeAgentConfig) -> Result<Self> {
        Ok(Self {
            client: Client::builder()
                .timeout(config.request_timeout)
                .build()
                .context("failed to build node-agent HTTP client")?,
            origin: config.control_plane_origin.clone(),
            bearer_token: config.control_plane_agent_token.clone(),
        })
    }

    async fn register(
        &self,
        request: &NodeAgentRegistrationRequest,
    ) -> Result<NodeAgentWriteResponse> {
        self.post_json("/api/v1/agent/register", request, None).await
    }

    async fn heartbeat(
        &self,
        request: &NodeAgentHeartbeatRequest,
    ) -> Result<NodeAgentWriteResponse> {
        self.post_json("/api/v1/agent/heartbeat", request, None).await
    }

    async fn ingest_snapshot(
        &self,
        request: &NodeSnapshotIngestRequest,
    ) -> Result<NodeSnapshotIngestResponse> {
        self.post_json("/api/v1/agent/snapshots", request, None).await
    }

    async fn poll_task(
        &self,
        request: &NodeAgentTaskPollRequest,
    ) -> Result<NodeAgentTaskPollResponse> {
        self.post_json("/api/v1/agent/tasks/poll", request, None).await
    }

    async fn ack_task(&self, task_id: &str, request: &NodeAgentTaskAckRequest) -> Result<()> {
        let _: serde_json::Value =
            self.post_json(&format!("/api/v1/agent/tasks/{task_id}/ack"), request, None).await?;
        Ok(())
    }

    async fn complete_task(
        &self,
        task_id: &str,
        request: &NodeAgentTaskCompleteRequest,
        idempotency_key: Option<String>,
    ) -> Result<NodeAgentTaskCompleteResponse> {
        self.post_json(
            &format!("/api/v1/agent/tasks/{task_id}/complete"),
            request,
            idempotency_key,
        )
            .await
    }

    async fn post_json<TRequest, TResponse>(
        &self,
        path: &str,
        request: &TRequest,
        idempotency_key: Option<String>,
    ) -> Result<TResponse>
    where
        TRequest: Serialize + ?Sized,
        TResponse: DeserializeOwned,
    {
        let url = format!("{}{}", self.origin, path);
        let response = self
            .request_builder(&url, idempotency_key)
            .json(request)
            .send()
            .await
            .with_context(|| format!("failed to POST node-agent payload to {url}"))?;
        let response = response
            .error_for_status()
            .with_context(|| format!("control plane rejected node-agent request to {url}"))?;
        response
            .json::<TResponse>()
            .await
            .with_context(|| format!("failed to decode node-agent response from {url}"))
    }

    fn request_builder(&self, url: &str, idempotency_key: Option<String>) -> RequestBuilder {
        let builder = self.client.post(url).bearer_auth(&self.bearer_token);
        if let Some(idempotency_key) = idempotency_key {
            builder.header("Idempotency-Key", idempotency_key)
        } else {
            builder
        }
    }
}

#[derive(Debug, Clone)]
struct BuiltAgentPayload {
    report: NodeAgentRegistrationRequest,
    snapshot: Option<NodeSnapshotIngestRequest>,
}

#[derive(Debug, Clone)]
struct CollectedRuntime {
    binary_version: Option<String>,
    report: NodeRuntimeReport,
    snapshot: Option<AdminSnapshot>,
}

impl CollectedRuntime {
    fn drift(message: impl Into<String>) -> Self {
        Self {
            binary_version: None,
            report: NodeRuntimeReport {
                snapshot_version: None,
                revision: None,
                pid: None,
                listener_count: None,
                active_connections: None,
                error: Some(message.into()),
            },
            snapshot: None,
        }
    }
}

#[derive(Debug, Clone)]
struct TaskExecutionResult {
    message: String,
    runtime_revision: Option<u64>,
}

#[derive(Debug, Serialize)]
struct AdminSnapshotRequest {
    #[serde(rename = "GetSnapshot")]
    get_snapshot: AdminSnapshotRequestBody,
}

#[derive(Debug, Serialize)]
struct AdminSnapshotRequestBody {
    include: Option<Vec<String>>,
    window_secs: Option<u64>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct AdminSnapshot {
    schema_version: u32,
    snapshot_version: u64,
    captured_at_unix_ms: u64,
    pid: u32,
    binary_version: String,
    included_modules: Vec<String>,
    status: Option<Value>,
    counters: Option<Value>,
    traffic: Option<Value>,
    peer_health: Option<Value>,
    upstreams: Option<Value>,
}

fn unix_time_ms(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).unwrap_or_default().as_millis().min(u128::from(u64::MAX)) as u64
}
