<script setup lang="ts">
import { computed, onMounted, onUnmounted, reactive, ref } from "vue";

import MetricCard from "../components/MetricCard.vue";
import {
  buildEventsUrl,
  clearStoredAuthToken,
  ensureEventsSession,
  extractApiErrorMessage,
  getAuditLogs,
  getDashboard,
  getHealth,
  getMe,
  getMeta,
  getNodes,
  getStoredAuthToken,
  login,
  logout,
  setStoredAuthToken,
  type AuditLogEntry,
  type AuthenticatedActor,
  type ControlPlaneMeta,
  type ControlPlaneOverviewEvent,
  type DashboardSummary,
  type NodeSummary,
  type ServiceHealth,
} from "../api/controlPlane";
import { formatUnixMs, streamStateLabel, type StreamState } from "../lib/display";

const health = ref<ServiceHealth | null>(null);
const meta = ref<ControlPlaneMeta | null>(null);
const actor = ref<AuthenticatedActor | null>(null);
const dashboard = ref<DashboardSummary | null>(null);
const nodes = ref<NodeSummary[]>([]);
const auditLogs = ref<AuditLogEntry[]>([]);
const loading = ref(true);
const loginPending = ref(false);
const error = ref<string | null>(null);
const streamState = ref<StreamState>("idle");
const credentials = reactive({
  username: "admin",
  password: "change-me-now",
});

let dashboardEventSource: EventSource | null = null;

const isAuthenticated = computed(() => actor.value !== null);
const canSeeAudit = computed(() =>
  actor.value?.user.roles.some((role) => role === "operator" || role === "super_admin") ?? false,
);

const metrics = computed(() => {
  if (!dashboard.value) {
    return [];
  }

  return [
    {
      title: "Clusters",
      value: dashboard.value.total_clusters,
      description: "当前纳管的边缘集群数量",
    },
    {
      title: "Nodes",
      value: dashboard.value.total_nodes,
      description: "纳管节点总数",
    },
    {
      title: "Online",
      value: dashboard.value.online_nodes,
      description: "SSE 驱动的在线节点视图",
    },
    {
      title: "Offline",
      value: dashboard.value.offline_nodes,
      description: "超时未上报、已被 worker 标记离线的节点",
    },
    {
      title: "Drifted",
      value: dashboard.value.drifted_nodes,
      description: "agent 在线但本地 admin.sock 读取失败的节点",
    },
    {
      title: "Deployments",
      value: dashboard.value.active_deployments,
      description: "正在执行的发布任务",
    },
    {
      title: "Alerts",
      value: dashboard.value.open_alert_count,
      description: `${dashboard.value.critical_alert_count} critical / ${dashboard.value.warning_alert_count} warning`,
    },
  ];
});

function canActorSeeAudit(currentActor: AuthenticatedActor): boolean {
  return currentActor.user.roles.some((role) => role === "operator" || role === "super_admin");
}

function closeDashboardStream(): void {
  if (dashboardEventSource) {
    dashboardEventSource.close();
    dashboardEventSource = null;
  }
  streamState.value = "idle";
}

async function openDashboardStream(): Promise<void> {
  closeDashboardStream();
  await ensureEventsSession();

  try {
    dashboardEventSource = new EventSource(buildEventsUrl());
  } catch (caught) {
    error.value = extractApiErrorMessage(caught);
    streamState.value = "error";
    return;
  }

  streamState.value = "connecting";
  dashboardEventSource.addEventListener("open", () => {
    streamState.value = "live";
  });
  dashboardEventSource.addEventListener("overview.tick", (event) => {
    try {
      const payload = JSON.parse((event as MessageEvent<string>).data) as ControlPlaneOverviewEvent;
      dashboard.value = payload.dashboard;
      nodes.value = payload.nodes;
      error.value = null;
      streamState.value = "live";
    } catch (caught) {
      error.value = caught instanceof Error ? caught.message : "failed to decode overview event";
      streamState.value = "error";
    }
  });
  dashboardEventSource.addEventListener("stream.error", (event) => {
    try {
      const payload = JSON.parse((event as MessageEvent<string>).data) as { message?: string };
      error.value = payload.message ?? "dashboard realtime stream reported an error";
    } catch {
      error.value = "dashboard realtime stream reported an error";
    }
    streamState.value = "error";
  });
  dashboardEventSource.addEventListener("error", () => {
    if (dashboardEventSource?.readyState === EventSource.CLOSED) {
      streamState.value = "error";
      return;
    }

    streamState.value = "reconnecting";
  });
}

async function loadPublicState(): Promise<void> {
  health.value = await getHealth();
}

async function loadProtectedState(): Promise<void> {
  const currentActor = await getMe();
  actor.value = currentActor;

  const [metaValue, dashboardValue, nodesValue, auditValue] = await Promise.all([
    getMeta(),
    getDashboard(),
    getNodes(),
    canActorSeeAudit(currentActor) ? getAuditLogs() : Promise.resolve([]),
  ]);

  meta.value = metaValue;
  dashboard.value = dashboardValue;
  nodes.value = nodesValue;
  auditLogs.value = auditValue;
  await openDashboardStream();
}

function resetProtectedState(): void {
  closeDashboardStream();
  actor.value = null;
  meta.value = null;
  dashboard.value = null;
  nodes.value = [];
  auditLogs.value = [];
}

async function handleLogin(): Promise<void> {
  loginPending.value = true;
  error.value = null;

  try {
    const response = await login({
      username: credentials.username,
      password: credentials.password,
    });
    setStoredAuthToken(response.token);
    actor.value = response.actor;
    await loadProtectedState();
  } catch (caught) {
    clearStoredAuthToken();
    resetProtectedState();
    error.value = extractApiErrorMessage(caught);
  } finally {
    loginPending.value = false;
  }
}

async function handleLogout(): Promise<void> {
  try {
    await logout();
  } catch (caught) {
    error.value = extractApiErrorMessage(caught);
  } finally {
    clearStoredAuthToken();
    resetProtectedState();
  }
}

onMounted(async () => {
  try {
    await loadPublicState();

    if (getStoredAuthToken()) {
      await loadProtectedState();
    }
  } catch (caught) {
    clearStoredAuthToken();
    resetProtectedState();
    error.value = extractApiErrorMessage(caught);
  } finally {
    loading.value = false;
  }
});

onUnmounted(() => {
  closeDashboardStream();
});
</script>

<template>
  <section class="page-shell">
    <header class="hero">
      <div>
        <p class="eyebrow">rginx control plane</p>
        <h1>边缘节点实时总览</h1>
        <p class="hero-copy">
          当前控制面已完成 Phase 6 的只读诊断闭环。登录后可以直接看到节点上线、离线、漂移状态，
          跳转到节点详情 / TLS 页面，并通过 SSE 持续刷新 Dashboard 与节点运行态。
        </p>
      </div>
      <div class="hero-meta">
        <p><strong>health</strong> {{ health?.status ?? "loading" }}</p>
        <p><strong>user</strong> {{ actor?.user.username ?? "anonymous" }}</p>
        <p><strong>stream</strong> <span :class="['realtime-pill', `realtime-pill--${streamState}`]">{{ streamStateLabel(streamState) }}</span></p>
        <p>
          <strong>roles</strong>
          {{ actor?.user.roles.join(", ") ?? "-" }}
        </p>
        <p><strong>nodes</strong> {{ dashboard?.total_nodes ?? 0 }}</p>
        <p><strong>api</strong> {{ meta?.api_version ?? "-" }}</p>
        <p><strong>bind</strong> {{ meta?.api_listen_addr ?? "-" }}</p>
      </div>
    </header>

    <p v-if="loading" class="state-banner">正在加载控制面状态…</p>
    <p v-else-if="error && !dashboard" class="state-banner state-banner--error">{{ error }}</p>

    <section v-if="!isAuthenticated && !loading" class="panel auth-panel">
      <header class="panel__header">
        <h2>Sign In</h2>
        <span>local accounts</span>
      </header>
      <form class="auth-form" @submit.prevent="handleLogin">
        <label class="field">
          <span>Username</span>
          <input v-model="credentials.username" autocomplete="username" />
        </label>
        <label class="field">
          <span>Password</span>
          <input v-model="credentials.password" type="password" autocomplete="current-password" />
        </label>
        <div class="auth-actions">
          <button class="primary-button" type="submit" :disabled="loginPending">
            {{ loginPending ? "Signing In..." : "Sign In" }}
          </button>
          <p class="auth-hint">
            开发环境默认账号：admin / operator / viewer，默认密码：change-me-now
          </p>
        </div>
      </form>
    </section>

    <template v-else-if="dashboard && actor">
      <section class="toolbar">
        <div class="identity-card">
          <p class="identity-card__name">{{ actor.user.display_name }}</p>
          <p class="identity-card__meta">
            {{ actor.user.username }} · session {{ actor.session.session_id }}
          </p>
        </div>
        <div class="toolbar-links">
          <RouterLink class="secondary-button secondary-button--link" :to="{ name: 'deployments' }">
            Deployments
          </RouterLink>
          <RouterLink class="secondary-button secondary-button--link" :to="{ name: 'audit' }">
            Audit
          </RouterLink>
          <RouterLink class="secondary-button secondary-button--link" :to="{ name: 'revisions' }">
            Revisions
          </RouterLink>
          <button class="secondary-button" type="button" @click="handleLogout">Sign Out</button>
        </div>
      </section>

      <p v-if="error" class="state-banner state-banner--warn">{{ error }}</p>

      <section class="metric-grid">
        <MetricCard
          v-for="metric in metrics"
          :key="metric.title"
          :title="metric.title"
          :value="metric.value"
          :description="metric.description"
        />
      </section>

      <article class="panel panel--stack">
        <header class="panel__header">
          <h2>Managed Nodes</h2>
          <span>{{ nodes.length }} entries</span>
        </header>
        <div class="table-scroll">
          <table class="data-table">
            <thead>
              <tr>
                <th>Node</th>
                <th>State</th>
                <th>Runtime</th>
                <th>Last Seen</th>
                <th>Reason / Admin</th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="node in nodes" :key="node.node_id">
                <td>
                  <strong>
                    <RouterLink
                      class="table-link"
                      :to="{ name: 'node-detail', params: { nodeId: node.node_id } }"
                    >
                      {{ node.node_id }}
                    </RouterLink>
                  </strong>
                  <div class="cell-meta">{{ node.cluster_id }} · {{ node.advertise_addr }}</div>
                  <div class="cell-actions">
                    <RouterLink
                      class="table-link table-link--subtle"
                      :to="{ name: 'node-detail', params: { nodeId: node.node_id } }"
                    >
                      detail
                    </RouterLink>
                    <RouterLink
                      class="table-link table-link--subtle"
                      :to="{ name: 'node-tls', params: { nodeId: node.node_id } }"
                    >
                      tls / ocsp
                    </RouterLink>
                  </div>
                </td>
                <td>
                  <span :class="['state-pill', `state-pill--${node.state}`]">{{ node.state }}</span>
                  <div class="cell-meta">{{ node.role }}</div>
                </td>
                <td>
                  <strong>{{ node.running_version }}</strong>
                  <div class="cell-meta">
                    rev {{ node.runtime_revision ?? "-" }} · snap {{ node.last_snapshot_version ?? "-" }}
                  </div>
                  <div class="cell-meta">
                    pid {{ node.runtime_pid ?? "-" }} · conn {{ node.active_connections ?? "-" }}
                  </div>
                </td>
                <td>{{ formatUnixMs(node.last_seen_unix_ms) }}</td>
                <td>
                  <strong>{{ node.status_reason ?? "healthy" }}</strong>
                  <div class="cell-meta">{{ node.admin_socket_path }}</div>
                </td>
              </tr>
            </tbody>
          </table>
        </div>
      </article>

      <section class="panel-grid">
        <article class="panel">
          <header class="panel__header">
            <h2>Open Alerts</h2>
            <span>{{ dashboard.open_alerts.length }} entries</span>
          </header>
          <p v-if="!dashboard.open_alerts.length" class="empty-state">当前没有打开中的异常告警。</p>
          <div v-else class="list-stack">
            <div v-for="alert in dashboard.open_alerts" :key="alert.alert_id" class="list-card">
              <strong>{{ alert.title }}</strong>
              <div class="cell-meta">{{ alert.severity }} · {{ alert.kind }} · {{ alert.resource_type }}/{{ alert.resource_id }}</div>
              <div class="cell-meta">{{ alert.message }}</div>
              <div class="cell-meta">{{ formatUnixMs(alert.observed_at_unix_ms) }}</div>
            </div>
          </div>
        </article>

        <article class="panel">
          <header class="panel__header">
            <h2>Recent Node Activity</h2>
            <span>{{ dashboard.recent_nodes.length }} entries</span>
          </header>
          <table class="data-table">
            <thead>
              <tr>
                <th>Node</th>
                <th>State</th>
                <th>Revision</th>
                <th>Seen</th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="node in dashboard.recent_nodes" :key="node.node_id">
                <td>
                  <RouterLink
                    class="table-link"
                    :to="{ name: 'node-detail', params: { nodeId: node.node_id } }"
                  >
                    {{ node.node_id }}
                  </RouterLink>
                </td>
                <td>{{ node.state }}</td>
                <td>{{ node.runtime_revision ?? "-" }}</td>
                <td>{{ formatUnixMs(node.last_seen_unix_ms) }}</td>
              </tr>
            </tbody>
          </table>
        </article>

        <article class="panel">
          <header class="panel__header">
            <h2>Recent Deployments</h2>
            <span>{{ dashboard.recent_deployments.length }} entries</span>
          </header>
          <table class="data-table">
            <thead>
              <tr>
                <th>Deployment</th>
                <th>Revision</th>
                <th>Status</th>
                <th>Healthy</th>
              </tr>
            </thead>
            <tbody>
              <tr
                v-for="deployment in dashboard.recent_deployments"
                :key="deployment.deployment_id"
              >
                <td>
                  <RouterLink
                    class="table-link"
                    :to="{ name: 'deployments', query: { deployment_id: deployment.deployment_id } }"
                  >
                    {{ deployment.deployment_id }}
                  </RouterLink>
                </td>
                <td>{{ deployment.revision_version_label }}</td>
                <td>{{ deployment.status }}</td>
                <td>{{ deployment.healthy_nodes }}/{{ deployment.target_nodes }}</td>
              </tr>
            </tbody>
          </table>
        </article>
      </section>

      <article v-if="dashboard.latest_revision" class="panel">
        <header class="panel__header">
          <h2>Latest Revision</h2>
          <span>{{ dashboard.latest_revision.version_label }}</span>
        </header>
        <p class="revision-summary">{{ dashboard.latest_revision.summary }}</p>
      </article>

      <article v-if="canSeeAudit" class="panel">
        <header class="panel__header">
          <h2>Recent Audit Logs</h2>
          <span>{{ auditLogs.length }} entries</span>
        </header>
        <div class="toolbar-links">
          <RouterLink class="secondary-button secondary-button--link" :to="{ name: 'audit' }">
            Open Full Audit
          </RouterLink>
        </div>
        <table class="data-table">
          <thead>
            <tr>
              <th>Request</th>
              <th>Actor</th>
              <th>Action</th>
              <th>Resource</th>
              <th>Result</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="audit in auditLogs" :key="audit.audit_id">
              <td>{{ audit.request_id }}</td>
              <td>{{ audit.actor_id }}</td>
              <td>{{ audit.action }}</td>
              <td>{{ audit.resource_type }}/{{ audit.resource_id }}</td>
              <td>{{ audit.result }}</td>
            </tr>
          </tbody>
        </table>
      </article>
    </template>
  </section>
</template>
