<script setup lang="ts">
import { onMounted, reactive, ref } from "vue";
import { useRouter } from "vue-router";

import {
  clearStoredAuthToken,
  extractApiErrorMessage,
  getAuditLog,
  getAuditLogs,
  getMe,
  getStoredAuthToken,
  type AuditLogEntry,
  type AuthenticatedActor,
} from "../api/controlPlane";
import { formatUnixMs } from "../lib/display";

const router = useRouter();
const actor = ref<AuthenticatedActor | null>(null);
const auditLogs = ref<AuditLogEntry[]>([]);
const selectedAudit = ref<AuditLogEntry | null>(null);
const loading = ref(true);
const querying = ref(false);
const error = ref<string | null>(null);

const filters = reactive({
  cluster_id: "",
  actor_id: "",
  action: "",
  resource_type: "",
  resource_id: "",
  result: "",
  limit: "50",
});

function resetUnauthorized(): void {
  clearStoredAuthToken();
  actor.value = null;
  auditLogs.value = [];
  selectedAudit.value = null;
  void router.replace({ name: "dashboard" });
}

async function loadAuditLogs(): Promise<void> {
  querying.value = true;
  error.value = null;

  try {
    auditLogs.value = await getAuditLogs({
      cluster_id: filters.cluster_id || null,
      actor_id: filters.actor_id || null,
      action: filters.action || null,
      resource_type: filters.resource_type || null,
      resource_id: filters.resource_id || null,
      result: filters.result || null,
      limit: Number.parseInt(filters.limit, 10) || 50,
    });
    selectedAudit.value = auditLogs.value[0] ?? null;
    if (selectedAudit.value) {
      selectedAudit.value = await getAuditLog(selectedAudit.value.audit_id);
    }
  } catch (caught) {
    if (String(caught).includes("401") || String(caught).includes("403")) {
      resetUnauthorized();
      return;
    }
    error.value = extractApiErrorMessage(caught);
  } finally {
    querying.value = false;
  }
}

async function selectAudit(auditId: string): Promise<void> {
  error.value = null;
  try {
    selectedAudit.value = await getAuditLog(auditId);
  } catch (caught) {
    error.value = extractApiErrorMessage(caught);
  }
}

onMounted(async () => {
  loading.value = true;
  if (!getStoredAuthToken()) {
    resetUnauthorized();
    loading.value = false;
    return;
  }

  try {
    actor.value = await getMe();
    await loadAuditLogs();
  } catch (caught) {
    error.value = extractApiErrorMessage(caught);
  } finally {
    loading.value = false;
  }
});
</script>

<template>
  <section class="page-shell">
    <header class="hero">
      <div>
        <p class="eyebrow">phase 9</p>
        <div class="breadcrumb-row">
          <RouterLink class="breadcrumb-link" :to="{ name: 'dashboard' }">Dashboard</RouterLink>
          <span>/</span>
          <span>Audit</span>
        </div>
        <h1>审计与运维追踪</h1>
        <p class="hero-copy">
          这里聚合完整 audit log，可按 cluster、actor、action、resource 和 result 过滤，
          用来追查是谁在什么时候对哪个资源执行了什么操作，以及结果如何。
        </p>
      </div>
      <div class="hero-meta">
        <p><strong>user</strong> {{ actor?.user.username ?? "-" }}</p>
        <p><strong>roles</strong> {{ actor?.user.roles.join(", ") ?? "-" }}</p>
        <p><strong>entries</strong> {{ auditLogs.length }}</p>
      </div>
    </header>

    <p v-if="loading" class="state-banner">正在加载 audit log…</p>
    <p v-else-if="error" class="state-banner state-banner--error">{{ error }}</p>

    <template v-if="!loading">
      <section class="toolbar">
        <div class="toolbar-links">
          <RouterLink class="secondary-button secondary-button--link" :to="{ name: 'dashboard' }">
            Dashboard
          </RouterLink>
          <RouterLink class="secondary-button secondary-button--link" :to="{ name: 'deployments' }">
            Deployments
          </RouterLink>
          <button class="secondary-button" type="button" :disabled="querying" @click="loadAuditLogs">
            Refresh
          </button>
        </div>
        <div class="identity-card">
          <p class="identity-card__name">{{ actor?.user.display_name ?? "anonymous" }}</p>
          <p class="identity-card__meta">phase 9 audit trail</p>
        </div>
      </section>

      <section class="panel panel--stack">
        <header class="panel__header">
          <h2>Filters</h2>
          <span>query audit logs</span>
        </header>
        <div class="field-grid">
          <label class="field">
            <span>Cluster</span>
            <input v-model="filters.cluster_id" />
          </label>
          <label class="field">
            <span>Actor</span>
            <input v-model="filters.actor_id" />
          </label>
          <label class="field">
            <span>Action</span>
            <input v-model="filters.action" />
          </label>
          <label class="field">
            <span>Resource Type</span>
            <input v-model="filters.resource_type" />
          </label>
          <label class="field">
            <span>Resource ID</span>
            <input v-model="filters.resource_id" />
          </label>
          <label class="field">
            <span>Result</span>
            <input v-model="filters.result" />
          </label>
          <label class="field">
            <span>Limit</span>
            <input v-model="filters.limit" type="number" min="1" />
          </label>
        </div>
        <button class="primary-button" type="button" :disabled="querying" @click="loadAuditLogs">
          Apply Filters
        </button>
      </section>

      <section class="panel-grid">
        <article class="panel">
          <header class="panel__header">
            <h2>Audit Entries</h2>
            <span>{{ auditLogs.length }} items</span>
          </header>
          <p v-if="!auditLogs.length" class="empty-state">当前筛选条件下没有 audit log。</p>
          <div v-else class="list-stack">
            <button
              v-for="entry in auditLogs"
              :key="entry.audit_id"
              class="list-card"
              :class="{ 'list-card--active': entry.audit_id === selectedAudit?.audit_id }"
              type="button"
              @click="selectAudit(entry.audit_id)"
            >
              <strong>{{ entry.action }}</strong>
              <div class="cell-meta">{{ entry.actor_id }} · {{ entry.result }}</div>
              <div class="cell-meta">{{ entry.resource_type }}/{{ entry.resource_id }}</div>
              <div class="cell-meta">{{ formatUnixMs(entry.created_at_unix_ms) }}</div>
            </button>
          </div>
        </article>

        <article v-if="selectedAudit" class="panel">
          <header class="panel__header">
            <h2>Audit Detail</h2>
            <span>{{ selectedAudit.audit_id }}</span>
          </header>
          <div class="detail-grid">
            <div>
              <strong>Request</strong>
              <div class="cell-meta">{{ selectedAudit.request_id }}</div>
            </div>
            <div>
              <strong>Actor</strong>
              <div class="cell-meta">{{ selectedAudit.actor_id }}</div>
            </div>
            <div>
              <strong>Cluster</strong>
              <div class="cell-meta">{{ selectedAudit.cluster_id ?? "-" }}</div>
            </div>
            <div>
              <strong>Result</strong>
              <div class="cell-meta">{{ selectedAudit.result }}</div>
            </div>
          </div>
          <pre class="code-block">{{ JSON.stringify(selectedAudit.details, null, 2) }}</pre>
        </article>
      </section>
    </template>
  </section>
</template>
