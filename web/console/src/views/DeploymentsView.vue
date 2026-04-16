<script setup lang="ts">
import axios from "axios";
import { computed, onMounted, onUnmounted, reactive, ref, watch } from "vue";
import { useRoute, useRouter } from "vue-router";

import MetricCard from "../components/MetricCard.vue";
import {
  buildEventsUrl,
  clearStoredAuthToken,
  type ControlPlaneDeploymentEvent,
  createDeployment,
  ensureEventsSession,
  extractApiErrorMessage,
  getDeployment,
  getDeployments,
  getMe,
  getRevisions,
  getStoredAuthToken,
  pauseDeployment,
  resumeDeployment,
  type AuthenticatedActor,
  type ConfigRevisionListItem,
  type DeploymentDetail,
  type DeploymentSummary,
} from "../api/controlPlane";
import { formatNullable, formatUnixMs } from "../lib/display";

const router = useRouter();
const route = useRoute();

const actor = ref<AuthenticatedActor | null>(null);
const revisions = ref<ConfigRevisionListItem[]>([]);
const deployments = ref<DeploymentSummary[]>([]);
const selectedDeployment = ref<DeploymentDetail | null>(null);
const loading = ref(true);
const saving = ref(false);
const error = ref<string | null>(null);
let deploymentEventSource: EventSource | null = null;

const createForm = reactive({
  cluster_id: "cluster-mainland",
  revision_id: "",
  parallelism: "1",
  failure_threshold: "1",
  auto_rollback: true,
  target_nodes_text: "",
});

const canManage = computed(() =>
  actor.value?.user.roles.some((role) => role === "operator" || role === "super_admin") ?? false,
);
const selectedRevisionMeta = computed(() =>
  revisions.value.find((revision) => revision.revision_id === createForm.revision_id) ?? null,
);
const selectedSummary = computed(() => selectedDeployment.value?.deployment ?? null);
const deploymentMetrics = computed(() => {
  const deployment = selectedSummary.value;
  if (!deployment) {
    return [];
  }

  return [
    {
      title: "Healthy",
      value: `${deployment.healthy_nodes}/${deployment.target_nodes}`,
      description: "已成功应用 revision 的节点",
    },
    {
      title: "Failed",
      value: deployment.failed_nodes,
      description: "节点任务失败数",
    },
    {
      title: "In Flight",
      value: deployment.in_flight_nodes,
      description: "已派发但尚未完成的节点",
    },
    {
      title: "Parallelism",
      value: deployment.parallelism,
      description: `failure threshold ${deployment.failure_threshold}`,
    },
  ];
});

function resetUnauthorized(): void {
  closeDeploymentStream();
  clearStoredAuthToken();
  actor.value = null;
  revisions.value = [];
  deployments.value = [];
  selectedDeployment.value = null;
  void router.replace({ name: "dashboard" });
}

function handleAuthFailure(caught: unknown): boolean {
  if (!axios.isAxiosError(caught)) {
    return false;
  }

  const status = caught.response?.status;
  if (status !== 401 && status !== 403) {
    return false;
  }

  resetUnauthorized();
  return true;
}

function closeDeploymentStream(): void {
  if (deploymentEventSource) {
    deploymentEventSource.close();
    deploymentEventSource = null;
  }
}

async function openDeploymentStream(deploymentId: string): Promise<void> {
  closeDeploymentStream();
  await ensureEventsSession();

  try {
    deploymentEventSource = new EventSource(buildEventsUrl({ deploymentId }));
  } catch (caught) {
    error.value = extractApiErrorMessage(caught);
    return;
  }

  deploymentEventSource.addEventListener("deployment.tick", (event) => {
    try {
      const payload = JSON.parse(
        (event as MessageEvent<string>).data,
      ) as ControlPlaneDeploymentEvent;
      selectedDeployment.value = payload.detail;
      const summary = payload.detail.deployment;
      const index = deployments.value.findIndex((entry) => entry.deployment_id === summary.deployment_id);
      if (index >= 0) {
        deployments.value.splice(index, 1, summary);
      }
      error.value = null;
    } catch (caught) {
      error.value = caught instanceof Error ? caught.message : "failed to decode deployment event";
    }
  });
}

function syncCreateFormFromRevision(revisionId: string): void {
  createForm.revision_id = revisionId;
  const revision = revisions.value.find((item) => item.revision_id === revisionId);
  if (revision) {
    createForm.cluster_id = revision.cluster_id;
  }
}

function parseTargetNodes(): string[] | null {
  const values = createForm.target_nodes_text
    .split(/[\s,]+/)
    .map((value) => value.trim())
    .filter(Boolean);
  return values.length > 0 ? Array.from(new Set(values)) : null;
}

async function loadLists(): Promise<void> {
  const [currentActor, revisionItems, deploymentItems] = await Promise.all([
    getMe(),
    getRevisions(),
    getDeployments(),
  ]);

  actor.value = currentActor;
  revisions.value = revisionItems;
  deployments.value = deploymentItems;

  const routeRevisionId =
    typeof route.query.revision_id === "string" ? route.query.revision_id : undefined;
  const defaultRevisionId = routeRevisionId ?? revisionItems[0]?.revision_id;
  if (defaultRevisionId) {
    syncCreateFormFromRevision(defaultRevisionId);
  }
}

async function selectDeployment(deploymentId: string): Promise<void> {
  error.value = null;

  try {
    selectedDeployment.value = await getDeployment(deploymentId);
    await router.replace({
      name: "deployments",
      query: { deployment_id: deploymentId, revision_id: createForm.revision_id || undefined },
    });
  } catch (caught) {
    error.value = extractApiErrorMessage(caught);
  }
}

async function refreshState(): Promise<void> {
  await loadLists();
  const routeDeploymentId =
    typeof route.query.deployment_id === "string" ? route.query.deployment_id : undefined;
  const nextDeploymentId = routeDeploymentId ?? deployments.value[0]?.deployment_id;
  if (nextDeploymentId) {
    await selectDeployment(nextDeploymentId);
  } else {
    selectedDeployment.value = null;
  }
}

async function loadInitialState(): Promise<void> {
  loading.value = true;
  error.value = null;

  if (!getStoredAuthToken()) {
    resetUnauthorized();
    loading.value = false;
    return;
  }

  try {
    await refreshState();
  } catch (caught) {
    if (String(caught).includes("401") || String(caught).includes("403")) {
      resetUnauthorized();
      return;
    }
    error.value = extractApiErrorMessage(caught);
  } finally {
    loading.value = false;
  }
}

async function handleCreateDeployment(): Promise<void> {
  if (!canManage.value || !createForm.revision_id) {
    return;
  }

  saving.value = true;
  error.value = null;
  try {
    const response = await createDeployment(
      {
        cluster_id: createForm.cluster_id,
        revision_id: createForm.revision_id,
        parallelism: Number.parseInt(createForm.parallelism, 10) || null,
        failure_threshold: Number.parseInt(createForm.failure_threshold, 10) || null,
        auto_rollback: createForm.auto_rollback,
        target_node_ids: parseTargetNodes(),
      },
      `deploy:${createForm.cluster_id}:${createForm.revision_id}:${Date.now()}`,
    );
    await refreshState();
    await selectDeployment(response.deployment.deployment.deployment_id);
  } catch (caught) {
    error.value = extractApiErrorMessage(caught);
  } finally {
    saving.value = false;
  }
}

async function handlePauseResume(): Promise<void> {
  if (!canManage.value || !selectedSummary.value) {
    return;
  }

  saving.value = true;
  error.value = null;
  try {
    const detail =
      selectedSummary.value.status === "paused"
        ? await resumeDeployment(selectedSummary.value.deployment_id)
        : await pauseDeployment(selectedSummary.value.deployment_id);
    selectedDeployment.value = detail;
    await loadLists();
  } catch (caught) {
    error.value = extractApiErrorMessage(caught);
  } finally {
    saving.value = false;
  }
}

onMounted(() => {
  void loadInitialState();
});

watch(
  () => selectedSummary.value?.deployment_id,
  (deploymentId) => {
    if (deploymentId) {
      void openDeploymentStream(deploymentId).catch((caught) => {
        if (handleAuthFailure(caught)) {
          return;
        }
        error.value = extractApiErrorMessage(caught);
      });
    } else {
      closeDeploymentStream();
    }
  },
);

onUnmounted(() => {
  closeDeploymentStream();
});
</script>

<template>
  <section class="page-shell">
    <header class="hero">
      <div>
        <p class="eyebrow">phase 8</p>
        <div class="breadcrumb-row">
          <RouterLink class="breadcrumb-link" :to="{ name: 'dashboard' }">Dashboard</RouterLink>
          <span>/</span>
          <span>Deployments</span>
        </div>
        <h1>发布编排与回滚</h1>
        <p class="hero-copy">
          控制面现在可以直接创建单集群滚动发布、按并发窗口派发 agent task、汇总节点结果，
          并在失败后自动创建 rollback deployment。
        </p>
      </div>
      <div class="hero-meta">
        <p><strong>user</strong> {{ actor?.user.username ?? "-" }}</p>
        <p><strong>roles</strong> {{ actor?.user.roles.join(", ") ?? "-" }}</p>
        <p><strong>deployments</strong> {{ deployments.length }}</p>
        <p><strong>revisions</strong> {{ revisions.length }}</p>
      </div>
    </header>

    <p v-if="loading" class="state-banner">正在加载 deployment 状态…</p>
    <p v-else-if="error" class="state-banner state-banner--error">{{ error }}</p>

    <template v-if="!loading">
      <section class="toolbar">
        <div class="toolbar-links">
          <RouterLink class="secondary-button secondary-button--link" :to="{ name: 'dashboard' }">
            Dashboard
          </RouterLink>
          <RouterLink class="secondary-button secondary-button--link" :to="{ name: 'revisions' }">
            Revisions
          </RouterLink>
          <button class="secondary-button" type="button" :disabled="saving" @click="refreshState">
            Refresh
          </button>
          <button
            v-if="canManage && selectedSummary && (selectedSummary.status === 'running' || selectedSummary.status === 'paused')"
            class="secondary-button"
            type="button"
            :disabled="saving"
            @click="handlePauseResume"
          >
            {{ selectedSummary.status === "paused" ? "Resume Deployment" : "Pause Deployment" }}
          </button>
        </div>
        <div class="identity-card">
          <p class="identity-card__name">{{ actor?.user.display_name ?? "anonymous" }}</p>
          <p class="identity-card__meta">
            {{ canManage ? "operator write" : "viewer read-only" }}
          </p>
        </div>
      </section>

      <section v-if="canManage" class="panel-grid">
        <article class="panel">
          <header class="panel__header">
            <h2>Create Deployment</h2>
            <span>rolling release</span>
          </header>
          <div class="field-grid">
            <label class="field">
              <span>Cluster</span>
              <input v-model="createForm.cluster_id" />
            </label>
            <label class="field">
              <span>Revision</span>
              <select v-model="createForm.revision_id" @change="syncCreateFormFromRevision(createForm.revision_id)">
                <option disabled value="">select revision</option>
                <option
                  v-for="revision in revisions"
                  :key="revision.revision_id"
                  :value="revision.revision_id"
                >
                  {{ revision.version_label }} · {{ revision.cluster_id }}
                </option>
              </select>
            </label>
            <label class="field">
              <span>Parallelism</span>
              <input v-model="createForm.parallelism" type="number" min="1" />
            </label>
            <label class="field">
              <span>Failure Threshold</span>
              <input v-model="createForm.failure_threshold" type="number" min="1" />
            </label>
            <label class="field field--full">
              <span>Target Nodes</span>
              <textarea
                v-model="createForm.target_nodes_text"
                rows="4"
                placeholder="留空表示集群内所有 online/draining 节点；也可输入 node_id，逗号或换行分隔"
              />
            </label>
            <label class="field field--full">
              <span>Auto Rollback</span>
              <select v-model="createForm.auto_rollback">
                <option :value="true">enabled</option>
                <option :value="false">disabled</option>
              </select>
            </label>
          </div>
          <p class="cell-meta">
            selected revision: {{ selectedRevisionMeta?.version_label ?? "-" }} ·
            {{ selectedRevisionMeta?.summary ?? "-" }}
          </p>
          <button class="primary-button" type="button" :disabled="saving || !createForm.revision_id" @click="handleCreateDeployment">
            Create Deployment
          </button>
        </article>
      </section>

      <section class="panel-grid">
        <article class="panel">
          <header class="panel__header">
            <h2>Deployments</h2>
            <span>{{ deployments.length }} items</span>
          </header>
          <p v-if="!deployments.length" class="empty-state">还没有 deployment。</p>
          <div v-else class="list-stack">
            <button
              v-for="deployment in deployments"
              :key="deployment.deployment_id"
              class="list-card"
              :class="{ 'list-card--active': deployment.deployment_id === selectedSummary?.deployment_id }"
              type="button"
              @click="selectDeployment(deployment.deployment_id)"
            >
              <strong>{{ deployment.revision_version_label }}</strong>
              <div class="cell-meta">
                {{ deployment.deployment_id }} · {{ deployment.cluster_id }} · {{ deployment.status }}
              </div>
              <div class="cell-meta">
                healthy {{ deployment.healthy_nodes }}/{{ deployment.target_nodes }} · failed {{ deployment.failed_nodes }} · inflight {{ deployment.in_flight_nodes }}
              </div>
              <div class="cell-meta">
                created {{ formatUnixMs(deployment.created_at_unix_ms) }} · by {{ deployment.created_by }}
              </div>
            </button>
          </div>
        </article>

        <article v-if="selectedDeployment" class="panel">
          <header class="panel__header">
            <h2>Deployment Detail</h2>
            <span>{{ selectedDeployment.deployment.status }}</span>
          </header>
          <div class="detail-grid">
            <div>
              <strong>ID</strong>
              <div class="cell-meta">{{ selectedDeployment.deployment.deployment_id }}</div>
            </div>
            <div>
              <strong>Revision</strong>
              <div class="cell-meta">
                {{ selectedDeployment.revision.version_label }} · {{ selectedDeployment.revision.revision_id }}
              </div>
            </div>
            <div>
              <strong>Rollback</strong>
              <div class="cell-meta">
                {{ selectedDeployment.rollback_revision?.version_label ?? "-" }}
              </div>
            </div>
            <div>
              <strong>Reason</strong>
              <div class="cell-meta">{{ formatNullable(selectedDeployment.deployment.status_reason) }}</div>
            </div>
          </div>
          <section class="metric-grid">
            <MetricCard
              v-for="metric in deploymentMetrics"
              :key="metric.title"
              :title="metric.title"
              :value="metric.value"
              :description="metric.description"
            />
          </section>
        </article>
      </section>

      <article v-if="selectedDeployment" class="panel panel--stack">
        <header class="panel__header">
          <h2>Targets</h2>
          <span>{{ selectedDeployment.targets.length }} nodes</span>
        </header>
        <div class="table-scroll">
          <table class="data-table">
            <thead>
              <tr>
                <th>Node</th>
                <th>State</th>
                <th>Task</th>
                <th>Batch / Attempt</th>
                <th>Timeline</th>
                <th>Error</th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="target in selectedDeployment.targets" :key="target.target_id">
                <td>
                  <strong>{{ target.node_id }}</strong>
                  <div class="cell-meta">{{ target.advertise_addr }}</div>
                </td>
                <td>
                  <div>{{ target.state }}</div>
                  <div class="cell-meta">{{ target.node_state }}</div>
                </td>
                <td>
                  <div>{{ target.task_kind ?? "-" }}</div>
                  <div class="cell-meta">{{ target.task_state ?? "-" }}</div>
                </td>
                <td>
                  <div>batch {{ target.batch_index }}</div>
                  <div class="cell-meta">attempt {{ target.attempt }}</div>
                </td>
                <td>
                  <div>dispatch {{ formatUnixMs(target.dispatched_at_unix_ms) }}</div>
                  <div class="cell-meta">ack {{ formatUnixMs(target.acked_at_unix_ms) }}</div>
                  <div class="cell-meta">done {{ formatUnixMs(target.completed_at_unix_ms) }}</div>
                </td>
                <td>{{ formatNullable(target.last_error) }}</td>
              </tr>
            </tbody>
          </table>
        </div>
      </article>

      <article v-if="selectedDeployment" class="panel">
        <header class="panel__header">
          <h2>Recent Events</h2>
          <span>{{ selectedDeployment.recent_events.length }} entries</span>
        </header>
        <table class="data-table">
          <thead>
            <tr>
              <th>Time</th>
              <th>Actor</th>
              <th>Action</th>
              <th>Result</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="event in selectedDeployment.recent_events" :key="event.audit_id">
              <td>{{ formatUnixMs(event.created_at_unix_ms) }}</td>
              <td>{{ event.actor_id }}</td>
              <td>{{ event.action }}</td>
              <td>{{ event.result }}</td>
            </tr>
          </tbody>
        </table>
      </article>
    </template>
  </section>
</template>
