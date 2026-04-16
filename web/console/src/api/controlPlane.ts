import axios from "axios";

const AUTH_TOKEN_STORAGE_KEY = "rginx-control-plane-token";

export interface ServiceHealth {
  service: string;
  status: string;
}

export interface ControlPlaneMeta {
  service_name: string;
  api_version: string;
  api_listen_addr: string;
  ui_path: string;
  node_agent_path: string;
}

export type AuthRole = "viewer" | "operator" | "super_admin";

export interface AuthUserSummary {
  user_id: string;
  username: string;
  display_name: string;
  active: boolean;
  roles: AuthRole[];
  created_at_unix_ms: number;
}

export interface AuthSessionSummary {
  session_id: string;
  issued_at_unix_ms: number;
  expires_at_unix_ms: number;
}

export interface AuthenticatedActor {
  user: AuthUserSummary;
  session: AuthSessionSummary;
}

export interface AuthLoginRequest {
  username: string;
  password: string;
}

export interface AuthLoginResponse {
  token: string;
  actor: AuthenticatedActor;
}

export interface AuditLogSummary {
  audit_id: string;
  request_id: string;
  cluster_id: string | null;
  actor_id: string;
  action: string;
  resource_type: string;
  resource_id: string;
  result: string;
  created_at_unix_ms: number;
}

export interface AuditLogEntry extends AuditLogSummary {
  details: Record<string, unknown>;
}

export type AlertSeverity = "warning" | "critical";

export interface ControlPlaneAlertSummary {
  alert_id: string;
  cluster_id: string | null;
  severity: AlertSeverity;
  kind: string;
  title: string;
  message: string;
  resource_type: string;
  resource_id: string;
  observed_at_unix_ms: number;
}

export interface ConfigRevisionSummary {
  revision_id: string;
  cluster_id: string;
  version_label: string;
  created_at_unix_ms: number;
  summary: string;
}

export interface NodeSummary {
  node_id: string;
  cluster_id: string;
  advertise_addr: string;
  role: string;
  state: string;
  running_version: string;
  admin_socket_path: string;
  last_seen_unix_ms: number;
  last_snapshot_version: number | null;
  runtime_revision: number | null;
  runtime_pid: number | null;
  listener_count: number | null;
  active_connections: number | null;
  status_reason: string | null;
}

export interface TlsListenerStatusSnapshot {
  listener_id: string;
  listener_name: string;
  listen_addr: string;
  tls_enabled: boolean;
  http3_enabled: boolean;
  http3_listen_addr: string | null;
  default_certificate: string | null;
  versions: string[] | null;
  alpn_protocols: string[];
  http3_versions: string[];
  http3_alpn_protocols: string[];
  http3_max_concurrent_streams: number | null;
  http3_stream_buffer_size: number | null;
  http3_active_connection_id_limit: number | null;
  http3_retry: boolean | null;
  http3_host_key_path: string | null;
  http3_gso: boolean | null;
  http3_early_data_enabled: boolean | null;
  session_resumption_enabled: boolean | null;
  session_tickets_enabled: boolean | null;
  session_cache_size: number | null;
  session_ticket_count: number | null;
  client_auth_mode: string | null;
  client_auth_verify_depth: number | null;
  client_auth_crl_configured: boolean;
  sni_names: string[];
}

export interface TlsCertificateStatusSnapshot {
  scope: string;
  cert_path: string;
  server_names: string[];
  subject: string | null;
  issuer: string | null;
  serial_number: string | null;
  san_dns_names: string[];
  fingerprint_sha256: string | null;
  subject_key_identifier: string | null;
  authority_key_identifier: string | null;
  is_ca: boolean | null;
  path_len_constraint: number | null;
  key_usage: string | null;
  extended_key_usage: string[];
  not_before_unix_ms: number | null;
  expires_in_days: number | null;
  not_after_unix_ms: number | null;
  chain_length: number;
  chain_subjects: string[];
  chain_diagnostics: string[];
  selected_as_default_for_listeners: string[];
  ocsp_staple_configured: boolean;
  additional_certificate_count: number;
}

export interface TlsOcspStatusSnapshot {
  scope: string;
  cert_path: string;
  ocsp_staple_path: string | null;
  responder_urls: string[];
  nonce_mode: string;
  responder_policy: string;
  auto_refresh_enabled: boolean;
  cache_loaded: boolean;
  cache_size_bytes: number | null;
  cache_modified_unix_ms: number | null;
  refreshes_total: number;
  failures_total: number;
  last_error: string | null;
  last_refresh_unix_ms: number | null;
}

export interface TlsVhostBindingSnapshot {
  listener_name: string;
  vhost_id: string;
  server_names: string[];
  certificate_scopes: string[];
  fingerprints: string[];
  default_selected: boolean;
}

export interface TlsSniBindingSnapshot {
  listener_name: string;
  server_name: string;
  certificate_scopes: string[];
  fingerprints: string[];
  default_selected: boolean;
}

export interface TlsDefaultCertificateBindingSnapshot {
  listener_name: string;
  server_name: string;
  certificate_scopes: string[];
  fingerprints: string[];
}

export interface TlsReloadBoundarySnapshot {
  reloadable_fields: string[];
  restart_required_fields: string[];
}

export interface MtlsStatusSnapshot {
  configured_listeners: number;
  optional_listeners: number;
  required_listeners: number;
  authenticated_connections: number;
  authenticated_requests: number;
  anonymous_requests: number;
  handshake_failures_total: number;
  handshake_failures_missing_client_cert: number;
  handshake_failures_unknown_ca: number;
  handshake_failures_bad_certificate: number;
  handshake_failures_certificate_revoked: number;
  handshake_failures_verify_depth_exceeded: number;
  handshake_failures_other: number;
}

export interface Http3ListenerRuntimeSnapshot {
  active_connections: number;
  active_request_streams: number;
  retry_issued_total: number;
  retry_failed_total: number;
  request_accept_errors_total: number;
  request_resolve_errors_total: number;
  request_body_stream_errors_total: number;
  response_stream_errors_total: number;
  connection_close_version_mismatch_total: number;
  connection_close_transport_error_total: number;
  connection_close_connection_closed_total: number;
  connection_close_application_closed_total: number;
  connection_close_reset_total: number;
  connection_close_timed_out_total: number;
  connection_close_locally_closed_total: number;
  connection_close_cids_exhausted_total: number;
}

export interface RuntimeListenerBindingSnapshot {
  binding_name: string;
  transport: string;
  listen_addr: string;
  protocols: string[];
  worker_count: number;
  reuse_port_enabled?: boolean | null;
  advertise_alt_svc?: boolean | null;
  alt_svc_max_age_secs?: number | null;
  http3_max_concurrent_streams?: number | null;
  http3_stream_buffer_size?: number | null;
  http3_active_connection_id_limit?: number | null;
  http3_retry?: boolean | null;
  http3_host_key_path?: string | null;
  http3_gso?: boolean | null;
  http3_early_data_enabled?: boolean | null;
}

export interface RuntimeListenerSnapshot {
  listener_id: string;
  listener_name: string;
  listen_addr: string;
  binding_count: number;
  http3_enabled: boolean;
  tls_enabled: boolean;
  proxy_protocol_enabled: boolean;
  default_certificate: string | null;
  keep_alive: boolean;
  max_connections: number | null;
  access_log_format_configured: boolean;
  http3_runtime?: Http3ListenerRuntimeSnapshot | null;
  bindings: RuntimeListenerBindingSnapshot[];
}

export type ReloadOutcomeSnapshot =
  | { Success: { revision: number } }
  | { Failure: { error: string } }
  | Record<string, unknown>;

export interface ReloadResultSnapshot {
  finished_at_unix_ms: number;
  outcome: ReloadOutcomeSnapshot;
  tls_certificate_changes: string[];
  active_revision: number;
  rollback_preserved_revision: number | null;
}

export interface ReloadStatusSnapshot {
  attempts_total: number;
  successes_total: number;
  failures_total: number;
  last_result: ReloadResultSnapshot | null;
}

export interface TlsRuntimeSnapshot {
  listeners: TlsListenerStatusSnapshot[];
  certificates: TlsCertificateStatusSnapshot[];
  ocsp: TlsOcspStatusSnapshot[];
  vhost_bindings: TlsVhostBindingSnapshot[];
  sni_bindings: TlsSniBindingSnapshot[];
  sni_conflicts: TlsSniBindingSnapshot[];
  default_certificate_bindings: TlsDefaultCertificateBindingSnapshot[];
  reload_boundary: TlsReloadBoundarySnapshot;
  expiring_certificate_count: number;
}

export interface UpstreamTlsStatusSnapshot {
  upstream_name: string;
  protocol: string;
  verify_mode: string;
  tls_versions: string[] | null;
  server_name_enabled: boolean;
  server_name_override: string | null;
  verify_depth: number | null;
  crl_configured: boolean;
  client_identity_configured: boolean;
}

export interface RuntimeStatusSnapshot {
  revision: number;
  config_path: string | null;
  listeners: RuntimeListenerSnapshot[];
  worker_threads: number | null;
  accept_workers: number;
  total_vhosts: number;
  total_routes: number;
  total_upstreams: number;
  tls_enabled: boolean;
  http3_active_connections: number;
  http3_active_request_streams: number;
  http3_retry_issued_total: number;
  http3_retry_failed_total: number;
  http3_request_accept_errors_total: number;
  http3_request_resolve_errors_total: number;
  http3_request_body_stream_errors_total: number;
  http3_response_stream_errors_total: number;
  http3_early_data_enabled_listeners: number;
  http3_early_data_accepted_requests: number;
  http3_early_data_rejected_requests: number;
  tls: TlsRuntimeSnapshot;
  mtls: MtlsStatusSnapshot;
  upstream_tls: UpstreamTlsStatusSnapshot[];
  active_connections: number;
  reload: ReloadStatusSnapshot;
}

export interface HttpCountersSnapshot {
  downstream_connections_accepted: number;
  downstream_connections_rejected: number;
  downstream_requests: number;
  downstream_responses: number;
  downstream_responses_1xx: number;
  downstream_responses_2xx: number;
  downstream_responses_3xx: number;
  downstream_responses_4xx: number;
  downstream_responses_5xx: number;
  downstream_mtls_authenticated_connections: number;
  downstream_mtls_authenticated_requests: number;
  downstream_mtls_anonymous_requests: number;
  downstream_tls_handshake_failures: number;
  downstream_tls_handshake_failures_missing_client_cert: number;
  downstream_tls_handshake_failures_unknown_ca: number;
  downstream_tls_handshake_failures_bad_certificate: number;
  downstream_tls_handshake_failures_certificate_revoked: number;
  downstream_tls_handshake_failures_verify_depth_exceeded: number;
  downstream_tls_handshake_failures_other: number;
  downstream_http3_early_data_accepted_requests: number;
  downstream_http3_early_data_rejected_requests: number;
}

export interface RecentTrafficStatsSnapshot {
  window_secs: number;
  downstream_requests_total: number;
  downstream_responses_total: number;
  downstream_responses_2xx_total: number;
  downstream_responses_4xx_total: number;
  downstream_responses_5xx_total: number;
  grpc_requests_total: number;
}

export interface GrpcTrafficSnapshot {
  requests_total: number;
  protocol_grpc_total: number;
  protocol_grpc_web_total: number;
  protocol_grpc_web_text_total: number;
  status_0_total: number;
  status_1_total: number;
  status_3_total: number;
  status_4_total: number;
  status_7_total: number;
  status_8_total: number;
  status_12_total: number;
  status_14_total: number;
  status_other_total: number;
}

export interface ListenerStatsSnapshot {
  listener_id: string;
  listener_name: string;
  listen_addr: string;
  active_connections: number;
  http3_runtime?: Http3ListenerRuntimeSnapshot | null;
  downstream_connections_accepted: number;
  downstream_connections_rejected: number;
  downstream_requests: number;
  unmatched_requests_total: number;
  downstream_responses: number;
  downstream_responses_1xx: number;
  downstream_responses_2xx: number;
  downstream_responses_3xx: number;
  downstream_responses_4xx: number;
  downstream_responses_5xx: number;
  recent_60s: RecentTrafficStatsSnapshot;
  recent_window?: RecentTrafficStatsSnapshot | null;
  grpc: GrpcTrafficSnapshot;
}

export interface VhostStatsSnapshot {
  vhost_id: string;
  server_names: string[];
  downstream_requests: number;
  unmatched_requests_total: number;
  downstream_responses: number;
  downstream_responses_1xx: number;
  downstream_responses_2xx: number;
  downstream_responses_3xx: number;
  downstream_responses_4xx: number;
  downstream_responses_5xx: number;
  recent_60s: RecentTrafficStatsSnapshot;
  recent_window?: RecentTrafficStatsSnapshot | null;
  grpc: GrpcTrafficSnapshot;
}

export interface RouteStatsSnapshot {
  route_id: string;
  vhost_id: string;
  downstream_requests: number;
  downstream_responses: number;
  downstream_responses_1xx: number;
  downstream_responses_2xx: number;
  downstream_responses_3xx: number;
  downstream_responses_4xx: number;
  downstream_responses_5xx: number;
  access_denied_total: number;
  rate_limited_total: number;
  recent_60s: RecentTrafficStatsSnapshot;
  recent_window?: RecentTrafficStatsSnapshot | null;
  grpc: GrpcTrafficSnapshot;
}

export interface TrafficStatsSnapshot {
  listeners: ListenerStatsSnapshot[];
  vhosts: VhostStatsSnapshot[];
  routes: RouteStatsSnapshot[];
}

export interface PeerHealthSnapshot {
  peer_url: string;
  backup: boolean;
  weight: number;
  available: boolean;
  passive_consecutive_failures: number;
  passive_cooldown_remaining_ms: number | null;
  passive_pending_recovery: boolean;
  active_unhealthy: boolean;
  active_consecutive_successes: number;
  active_requests: number;
}

export interface UpstreamHealthSnapshot {
  upstream_name: string;
  unhealthy_after_failures: number;
  cooldown_ms: number;
  active_health_enabled: boolean;
  peers: PeerHealthSnapshot[];
}

export interface UpstreamPeerStatsSnapshot {
  peer_url: string;
  attempts_total: number;
  successes_total: number;
  failures_total: number;
  timeouts_total: number;
}

export interface RecentUpstreamStatsSnapshot {
  window_secs: number;
  downstream_requests_total: number;
  peer_attempts_total: number;
  completed_responses_total: number;
  bad_gateway_responses_total: number;
  gateway_timeout_responses_total: number;
  failovers_total: number;
}

export interface UpstreamStatsSnapshot {
  upstream_name: string;
  downstream_requests_total: number;
  peer_attempts_total: number;
  peer_successes_total: number;
  peer_failures_total: number;
  peer_timeouts_total: number;
  failovers_total: number;
  completed_responses_total: number;
  bad_gateway_responses_total: number;
  gateway_timeout_responses_total: number;
  bad_request_responses_total: number;
  payload_too_large_responses_total: number;
  unsupported_media_type_responses_total: number;
  no_healthy_peers_total: number;
  tls_failures_unknown_ca_total: number;
  tls_failures_bad_certificate_total: number;
  tls_failures_certificate_revoked_total: number;
  tls_failures_verify_depth_exceeded_total: number;
  recent_60s: RecentUpstreamStatsSnapshot;
  recent_window?: RecentUpstreamStatsSnapshot | null;
  tls: UpstreamTlsStatusSnapshot;
  peers: UpstreamPeerStatsSnapshot[];
}

export interface NodeSnapshotMeta {
  node_id: string;
  snapshot_version: number;
  schema_version: number;
  captured_at_unix_ms: number;
  pid: number;
  binary_version: string;
  included_modules: string[];
}

export interface NodeSnapshotDetail extends NodeSnapshotMeta {
  status: RuntimeStatusSnapshot | null;
  counters: HttpCountersSnapshot | null;
  traffic: TrafficStatsSnapshot | null;
  peer_health: UpstreamHealthSnapshot[] | null;
  upstreams: UpstreamStatsSnapshot[] | null;
}

export interface NodeDetailResponse {
  node: NodeSummary;
  latest_snapshot: NodeSnapshotDetail | null;
  recent_snapshots: NodeSnapshotMeta[];
  recent_events: AuditLogSummary[];
}

export interface DeploymentSummary {
  deployment_id: string;
  cluster_id: string;
  revision_id: string;
  revision_version_label: string;
  status: string;
  target_nodes: number;
  healthy_nodes: number;
  failed_nodes: number;
  in_flight_nodes: number;
  parallelism: number;
  failure_threshold: number;
  auto_rollback: boolean;
  created_by: string;
  rollback_of_deployment_id: string | null;
  rollback_revision_id: string | null;
  status_reason: string | null;
  created_at_unix_ms: number;
  started_at_unix_ms: number | null;
  finished_at_unix_ms: number | null;
}

export interface DeploymentTargetSummary {
  target_id: string;
  deployment_id: string;
  node_id: string;
  advertise_addr: string;
  node_state: string;
  desired_revision_id: string;
  state: string;
  task_id: string | null;
  task_kind: string | null;
  task_state: string | null;
  attempt: number;
  batch_index: number;
  last_error: string | null;
  dispatched_at_unix_ms: number | null;
  acked_at_unix_ms: number | null;
  completed_at_unix_ms: number | null;
}

export interface DeploymentDetail {
  deployment: DeploymentSummary;
  revision: ConfigRevisionSummary;
  rollback_revision: ConfigRevisionSummary | null;
  targets: DeploymentTargetSummary[];
  recent_events: AuditLogSummary[];
}

export interface CreateDeploymentRequest {
  cluster_id: string;
  revision_id: string;
  target_node_ids: string[] | null;
  parallelism: number | null;
  failure_threshold: number | null;
  auto_rollback: boolean | null;
}

export interface CreateDeploymentResponse {
  deployment: DeploymentDetail;
  reused: boolean;
}

export type ConfigDraftValidationState = "pending" | "valid" | "invalid" | "published";

export interface CompiledListenerBindingSummary {
  binding_name: string;
  transport: string;
  listen_addr: string;
  protocols: string[];
}

export interface CompiledListenerSummary {
  listener_id: string;
  listener_name: string;
  listen_addr: string;
  binding_count: number;
  tls_enabled: boolean;
  http3_enabled: boolean;
  default_certificate: string | null;
  bindings: CompiledListenerBindingSummary[];
}

export interface CompiledTlsSummary {
  listener_tls_profiles: number;
  vhost_tls_overrides: number;
  default_certificate_bindings: string[];
}

export interface ConfigCompileSummary {
  listener_model: string;
  listener_count: number;
  listener_binding_count: number;
  total_vhost_count: number;
  total_route_count: number;
  upstream_count: number;
  worker_threads: number | null;
  accept_workers: number;
  tls_enabled: boolean;
  http3_enabled: boolean;
  http3_early_data_enabled_listeners: number;
  default_server_names: string[];
  upstream_names: string[];
  listeners: CompiledListenerSummary[];
  tls: CompiledTlsSummary;
}

export interface ConfigValidationReport {
  valid: boolean;
  validated_at_unix_ms: number;
  normalized_source_path: string;
  issues: string[];
  summary: ConfigCompileSummary | null;
}

export interface ConfigRevisionListItem {
  revision_id: string;
  cluster_id: string;
  version_label: string;
  summary: string;
  created_by: string;
  created_at_unix_ms: number;
}

export interface ConfigRevisionDetail {
  revision_id: string;
  cluster_id: string;
  version_label: string;
  summary: string;
  created_by: string;
  created_at_unix_ms: number;
  source_path: string;
  config_text: string;
  compile_summary: ConfigCompileSummary | null;
}

export interface ConfigDraftSummary {
  draft_id: string;
  cluster_id: string;
  title: string;
  summary: string;
  base_revision_id: string | null;
  validation_state: ConfigDraftValidationState;
  published_revision_id: string | null;
  created_by: string;
  updated_by: string;
  created_at_unix_ms: number;
  updated_at_unix_ms: number;
}

export interface ConfigDraftDetail {
  draft_id: string;
  cluster_id: string;
  title: string;
  summary: string;
  source_path: string;
  config_text: string;
  base_revision_id: string | null;
  validation_state: ConfigDraftValidationState;
  published_revision_id: string | null;
  created_by: string;
  updated_by: string;
  created_at_unix_ms: number;
  updated_at_unix_ms: number;
  last_validation: ConfigValidationReport | null;
}

export interface CreateConfigDraftRequest {
  cluster_id: string;
  title: string;
  summary: string;
  source_path: string;
  config_text: string;
  base_revision_id: string | null;
}

export interface UpdateConfigDraftRequest {
  title: string;
  summary: string;
  source_path: string;
  config_text: string;
  base_revision_id: string | null;
}

export interface PublishConfigDraftRequest {
  version_label: string;
  summary: string | null;
}

export interface PublishConfigDraftResponse {
  draft: ConfigDraftDetail;
  revision: ConfigRevisionDetail;
}

export type ConfigDiffLineKind = "context" | "added" | "removed";

export interface ConfigDiffLine {
  kind: ConfigDiffLineKind;
  left_line_number: number | null;
  right_line_number: number | null;
  text: string;
}

export interface ConfigDiffResponse {
  left_label: string;
  right_label: string;
  changed: boolean;
  lines: ConfigDiffLine[];
}

export interface DashboardSummary {
  total_clusters: number;
  total_nodes: number;
  online_nodes: number;
  draining_nodes: number;
  offline_nodes: number;
  drifted_nodes: number;
  total_revisions: number;
  active_deployments: number;
  open_alert_count: number;
  critical_alert_count: number;
  warning_alert_count: number;
  latest_revision: ConfigRevisionSummary | null;
  recent_nodes: NodeSummary[];
  recent_deployments: DeploymentSummary[];
  open_alerts: ControlPlaneAlertSummary[];
}

export interface ControlPlaneOverviewEvent {
  event_id: string;
  emitted_at_unix_ms: number;
  dashboard: DashboardSummary;
  nodes: NodeSummary[];
}

export interface ControlPlaneNodeDetailEvent {
  event_id: string;
  emitted_at_unix_ms: number;
  detail: NodeDetailResponse;
}

export interface ControlPlaneDeploymentEvent {
  event_id: string;
  emitted_at_unix_ms: number;
  detail: DeploymentDetail;
}

const client = axios.create({
  baseURL: "/",
  timeout: 3000,
});

client.interceptors.request.use((config) => {
  const token = getStoredAuthToken();
  if (token) {
    config.headers = config.headers ?? {};
    config.headers.Authorization = `Bearer ${token}`;
  }

  return config;
});

export function getStoredAuthToken(): string | null {
  return window.localStorage.getItem(AUTH_TOKEN_STORAGE_KEY);
}

export function setStoredAuthToken(token: string): void {
  window.localStorage.setItem(AUTH_TOKEN_STORAGE_KEY, token);
}

export function clearStoredAuthToken(): void {
  window.localStorage.removeItem(AUTH_TOKEN_STORAGE_KEY);
}

export function extractApiErrorMessage(caught: unknown): string {
  if (axios.isAxiosError(caught)) {
    const message =
      (caught.response?.data as { error?: { message?: string } } | undefined)?.error?.message;
    return message ?? caught.message;
  }

  return caught instanceof Error ? caught.message : "unexpected control-plane request failure";
}

export async function getHealth(): Promise<ServiceHealth> {
  const { data } = await client.get<ServiceHealth>("/healthz");
  return data;
}

export async function login(request: AuthLoginRequest): Promise<AuthLoginResponse> {
  const { data } = await client.post<AuthLoginResponse>("/api/v1/auth/login", request);
  return data;
}

export async function logout(): Promise<void> {
  await client.post("/api/v1/auth/logout");
}

export async function getMe(): Promise<AuthenticatedActor> {
  const { data } = await client.get<AuthenticatedActor>("/api/v1/auth/me");
  return data;
}

export async function getMeta(): Promise<ControlPlaneMeta> {
  const { data } = await client.get<ControlPlaneMeta>("/api/v1/meta");
  return data;
}

export async function getDashboard(): Promise<DashboardSummary> {
  const { data } = await client.get<DashboardSummary>("/api/v1/dashboard");
  return data;
}

export async function getAlerts(): Promise<ControlPlaneAlertSummary[]> {
  const { data } = await client.get<ControlPlaneAlertSummary[]>("/api/v1/alerts");
  return data;
}

export async function getNodes(): Promise<NodeSummary[]> {
  const { data } = await client.get<NodeSummary[]>("/api/v1/nodes");
  return data;
}

export async function getNodeDetail(nodeId: string): Promise<NodeDetailResponse> {
  const { data } = await client.get<NodeDetailResponse>(`/api/v1/nodes/${nodeId}`);
  return data;
}

export async function getRevisions(): Promise<ConfigRevisionListItem[]> {
  const { data } = await client.get<ConfigRevisionListItem[]>("/api/v1/revisions");
  return data;
}

export async function getDeployments(): Promise<DeploymentSummary[]> {
  const { data } = await client.get<DeploymentSummary[]>("/api/v1/deployments");
  return data;
}

export async function getDeployment(deploymentId: string): Promise<DeploymentDetail> {
  const { data } = await client.get<DeploymentDetail>(`/api/v1/deployments/${deploymentId}`);
  return data;
}

export async function createDeployment(
  request: CreateDeploymentRequest,
  idempotencyKey?: string,
): Promise<CreateDeploymentResponse> {
  const { data } = await client.post<CreateDeploymentResponse>("/api/v1/deployments", request, {
    headers: idempotencyKey ? { "Idempotency-Key": idempotencyKey } : undefined,
  });
  return data;
}

export async function pauseDeployment(deploymentId: string): Promise<DeploymentDetail> {
  const { data } = await client.post<DeploymentDetail>(`/api/v1/deployments/${deploymentId}/pause`);
  return data;
}

export async function resumeDeployment(deploymentId: string): Promise<DeploymentDetail> {
  const { data } = await client.post<DeploymentDetail>(`/api/v1/deployments/${deploymentId}/resume`);
  return data;
}

export async function getRevision(revisionId: string): Promise<ConfigRevisionDetail> {
  const { data } = await client.get<ConfigRevisionDetail>(`/api/v1/revisions/${revisionId}`);
  return data;
}

export async function getDrafts(): Promise<ConfigDraftSummary[]> {
  const { data } = await client.get<ConfigDraftSummary[]>("/api/v1/revisions/drafts");
  return data;
}

export async function getDraft(draftId: string): Promise<ConfigDraftDetail> {
  const { data } = await client.get<ConfigDraftDetail>(`/api/v1/revisions/drafts/${draftId}`);
  return data;
}

export async function createDraft(request: CreateConfigDraftRequest): Promise<ConfigDraftDetail> {
  const { data } = await client.post<ConfigDraftDetail>("/api/v1/revisions/drafts", request);
  return data;
}

export async function updateDraft(
  draftId: string,
  request: UpdateConfigDraftRequest,
): Promise<ConfigDraftDetail> {
  const { data } = await client.put<ConfigDraftDetail>(`/api/v1/revisions/drafts/${draftId}`, request);
  return data;
}

export async function validateDraft(draftId: string): Promise<ConfigDraftDetail> {
  const { data } = await client.post<ConfigDraftDetail>(`/api/v1/revisions/drafts/${draftId}/validate`);
  return data;
}

export async function diffDraft(
  draftId: string,
  targetRevisionId?: string | null,
): Promise<ConfigDiffResponse> {
  const query = new URLSearchParams();
  if (targetRevisionId) {
    query.set("target_revision_id", targetRevisionId);
  }
  const suffix = query.toString();
  const { data } = await client.get<ConfigDiffResponse>(
    `/api/v1/revisions/drafts/${draftId}/diff${suffix ? `?${suffix}` : ""}`,
  );
  return data;
}

export async function publishDraft(
  draftId: string,
  request: PublishConfigDraftRequest,
): Promise<PublishConfigDraftResponse> {
  const { data } = await client.post<PublishConfigDraftResponse>(
    `/api/v1/revisions/drafts/${draftId}/publish`,
    request,
  );
  return data;
}

export async function getAuditLogs(query?: {
  cluster_id?: string | null;
  actor_id?: string | null;
  action?: string | null;
  resource_type?: string | null;
  resource_id?: string | null;
  result?: string | null;
  limit?: number | null;
}): Promise<AuditLogEntry[]> {
  const params = new URLSearchParams();
  if (query?.cluster_id) params.set("cluster_id", query.cluster_id);
  if (query?.actor_id) params.set("actor_id", query.actor_id);
  if (query?.action) params.set("action", query.action);
  if (query?.resource_type) params.set("resource_type", query.resource_type);
  if (query?.resource_id) params.set("resource_id", query.resource_id);
  if (query?.result) params.set("result", query.result);
  if (query?.limit !== null && query?.limit !== undefined) params.set("limit", String(query.limit));
  const suffix = params.toString();
  const { data } = await client.get<AuditLogEntry[]>(`/api/v1/audit-logs${suffix ? `?${suffix}` : ""}`);
  return data;
}

export async function getAuditLog(auditId: string): Promise<AuditLogEntry> {
  const { data } = await client.get<AuditLogEntry>(`/api/v1/audit-logs/${auditId}`);
  return data;
}

export function buildEventsUrl(params?: { nodeId?: string; deploymentId?: string }): string {
  const token = getStoredAuthToken();
  if (!token) {
    throw new Error("missing control-plane auth token");
  }

  const query = new URLSearchParams({ access_token: token });
  if (params?.nodeId) {
    query.set("node_id", params.nodeId);
  }
  if (params?.deploymentId) {
    query.set("deployment_id", params.deploymentId);
  }

  return `/api/v1/events?${query.toString()}`;
}
