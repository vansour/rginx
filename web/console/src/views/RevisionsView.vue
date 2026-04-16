<script setup lang="ts">
import { computed, onMounted, reactive, ref } from "vue";
import { useRouter } from "vue-router";

import MetricCard from "../components/MetricCard.vue";
import {
  clearStoredAuthToken,
  createDraft,
  diffDraft,
  extractApiErrorMessage,
  getDraft,
  getDrafts,
  getMe,
  getRevision,
  getRevisions,
  getStoredAuthToken,
  publishDraft,
  type AuthenticatedActor,
  type ConfigDiffLine,
  type ConfigDiffResponse,
  type ConfigDraftDetail,
  type ConfigDraftSummary,
  type ConfigRevisionDetail,
  type ConfigRevisionListItem,
  updateDraft,
  validateDraft,
} from "../api/controlPlane";
import { formatList, formatUnixMs } from "../lib/display";

const DEFAULT_DRAFT_CONFIG = `Config(
    runtime: RuntimeConfig(
        shutdown_timeout_secs: 2,
        worker_threads: Some(2),
        accept_workers: Some(1),
    ),
    server: ServerConfig(
        listen: "0.0.0.0:8080",
        server_names: ["edge.example.local"],
    ),
    upstreams: [],
    locations: [
        LocationConfig(
            matcher: Exact("/"),
            handler: Return(
                status: 200,
                location: "",
                body: Some("ok\\n"),
            ),
        ),
    ],
)`;

const router = useRouter();
const actor = ref<AuthenticatedActor | null>(null);
const revisions = ref<ConfigRevisionListItem[]>([]);
const drafts = ref<ConfigDraftSummary[]>([]);
const selectedDraft = ref<ConfigDraftDetail | null>(null);
const selectedRevision = ref<ConfigRevisionDetail | null>(null);
const diff = ref<ConfigDiffResponse | null>(null);
const loading = ref(true);
const saving = ref(false);
const error = ref<string | null>(null);

const createForm = reactive({
  cluster_id: "cluster-mainland",
  title: "phase7-draft",
  summary: "new control-plane draft",
  source_path: "configs/rginx.phase7.ron",
  config_text: DEFAULT_DRAFT_CONFIG,
  base_revision_id: "",
});

const draftForm = reactive({
  title: "",
  summary: "",
  source_path: "",
  config_text: "",
  base_revision_id: "",
});

const publishForm = reactive({
  version_label: "",
  summary: "",
});

const canManage = computed(() =>
  actor.value?.user.roles.some((role) => role === "operator" || role === "super_admin") ?? false,
);

const compileMetrics = computed(() => {
  const summary = selectedDraft.value?.last_validation?.summary ?? selectedRevision.value?.compile_summary;
  if (!summary) {
    return [];
  }

  return [
    {
      title: "Listeners",
      value: summary.listener_count,
      description: `${summary.listener_model} listener model`,
    },
    {
      title: "Bindings",
      value: summary.listener_binding_count,
      description: "compiled transport bindings",
    },
    {
      title: "VHosts",
      value: summary.total_vhost_count,
      description: "compiled virtual hosts",
    },
    {
      title: "Routes",
      value: summary.total_route_count,
      description: "compiled routes",
    },
    {
      title: "Upstreams",
      value: summary.upstream_count,
      description: "compiled upstream definitions",
    },
    {
      title: "HTTP/3",
      value: summary.http3_enabled ? "enabled" : "disabled",
      description: `${summary.http3_early_data_enabled_listeners} early-data listeners`,
    },
  ];
});

function populateDraftForm(draft: ConfigDraftDetail): void {
  draftForm.title = draft.title;
  draftForm.summary = draft.summary;
  draftForm.source_path = draft.source_path;
  draftForm.config_text = draft.config_text;
  draftForm.base_revision_id = draft.base_revision_id ?? "";
  publishForm.version_label = draft.published_revision_id ? "" : draft.title.replace(/\s+/g, "-");
  publishForm.summary = draft.summary;
}

function resetUnauthorized(): void {
  clearStoredAuthToken();
  actor.value = null;
  revisions.value = [];
  drafts.value = [];
  selectedDraft.value = null;
  selectedRevision.value = null;
  diff.value = null;
  void router.replace({ name: "dashboard" });
}

async function loadLists(): Promise<void> {
  const [currentActor, revisionItems, draftItems] = await Promise.all([
    getMe(),
    getRevisions(),
    getDrafts(),
  ]);

  actor.value = currentActor;
  revisions.value = revisionItems;
  drafts.value = draftItems;

  if (revisionItems.length > 0) {
    createForm.cluster_id = revisionItems[0].cluster_id;
    if (!createForm.base_revision_id) {
      createForm.base_revision_id = revisionItems[0].revision_id;
    }
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
    await loadLists();

    if (drafts.value.length > 0) {
      await selectDraft(drafts.value[0].draft_id);
    } else if (revisions.value.length > 0) {
      await selectRevision(revisions.value[0].revision_id);
    }
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

async function selectDraft(draftId: string): Promise<void> {
  error.value = null;
  diff.value = null;

  try {
    const draft = await getDraft(draftId);
    selectedDraft.value = draft;
    populateDraftForm(draft);
  } catch (caught) {
    error.value = extractApiErrorMessage(caught);
  }
}

async function selectRevision(revisionId: string): Promise<void> {
  error.value = null;

  try {
    selectedRevision.value = await getRevision(revisionId);
  } catch (caught) {
    error.value = extractApiErrorMessage(caught);
  }
}

async function refreshLists(): Promise<void> {
  try {
    await loadLists();
  } catch (caught) {
    error.value = extractApiErrorMessage(caught);
  }
}

async function handleCreateDraft(): Promise<void> {
  if (!canManage.value) {
    return;
  }

  saving.value = true;
  error.value = null;
  try {
    const draft = await createDraft({
      cluster_id: createForm.cluster_id,
      title: createForm.title,
      summary: createForm.summary,
      source_path: createForm.source_path,
      config_text: createForm.config_text,
      base_revision_id: createForm.base_revision_id || null,
    });
    await refreshLists();
    await selectDraft(draft.draft_id);
  } catch (caught) {
    error.value = extractApiErrorMessage(caught);
  } finally {
    saving.value = false;
  }
}

async function handleSaveDraft(): Promise<void> {
  if (!canManage.value || !selectedDraft.value) {
    return;
  }

  saving.value = true;
  error.value = null;
  try {
    const draft = await updateDraft(selectedDraft.value.draft_id, {
      title: draftForm.title,
      summary: draftForm.summary,
      source_path: draftForm.source_path,
      config_text: draftForm.config_text,
      base_revision_id: draftForm.base_revision_id || null,
    });
    selectedDraft.value = draft;
    populateDraftForm(draft);
    await refreshLists();
  } catch (caught) {
    error.value = extractApiErrorMessage(caught);
  } finally {
    saving.value = false;
  }
}

async function handleValidateDraft(): Promise<void> {
  if (!canManage.value || !selectedDraft.value) {
    return;
  }

  saving.value = true;
  error.value = null;
  try {
    const draft = await validateDraft(selectedDraft.value.draft_id);
    selectedDraft.value = draft;
    populateDraftForm(draft);
    await refreshLists();
  } catch (caught) {
    error.value = extractApiErrorMessage(caught);
  } finally {
    saving.value = false;
  }
}

async function handleLoadDiff(): Promise<void> {
  if (!selectedDraft.value) {
    return;
  }

  saving.value = true;
  error.value = null;
  try {
    diff.value = await diffDraft(
      selectedDraft.value.draft_id,
      draftForm.base_revision_id || selectedRevision.value?.revision_id || null,
    );
  } catch (caught) {
    error.value = extractApiErrorMessage(caught);
  } finally {
    saving.value = false;
  }
}

async function handlePublishDraft(): Promise<void> {
  if (!canManage.value || !selectedDraft.value) {
    return;
  }

  saving.value = true;
  error.value = null;
  try {
    const response = await publishDraft(selectedDraft.value.draft_id, {
      version_label: publishForm.version_label,
      summary: publishForm.summary || null,
    });
    selectedDraft.value = response.draft;
    selectedRevision.value = response.revision;
    populateDraftForm(response.draft);
    await refreshLists();
  } catch (caught) {
    error.value = extractApiErrorMessage(caught);
  } finally {
    saving.value = false;
  }
}

function diffClass(line: ConfigDiffLine): string {
  if (line.kind === "added") {
    return "diff-line diff-line--added";
  }
  if (line.kind === "removed") {
    return "diff-line diff-line--removed";
  }
  return "diff-line";
}

onMounted(() => {
  void loadInitialState();
});
</script>

<template>
  <section class="page-shell">
    <header class="hero">
      <div>
        <p class="eyebrow">phase 7</p>
        <div class="breadcrumb-row">
          <RouterLink class="breadcrumb-link" :to="{ name: 'dashboard' }">Dashboard</RouterLink>
          <span>/</span>
          <span>Revisions</span>
        </div>
        <h1>配置版本管理</h1>
        <p class="hero-copy">
          这里把配置从“文件”变成控制面实体。支持草稿、validate / compile dry-run、版本 diff，
          以及将通过校验的 draft 发布成可部署 revision。
        </p>
      </div>
      <div class="hero-meta">
        <p><strong>user</strong> {{ actor?.user.username ?? "-" }}</p>
        <p><strong>roles</strong> {{ actor?.user.roles.join(", ") ?? "-" }}</p>
        <p><strong>revisions</strong> {{ revisions.length }}</p>
        <p><strong>drafts</strong> {{ drafts.length }}</p>
      </div>
    </header>

    <p v-if="loading" class="state-banner">正在加载 revision / draft 状态…</p>
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
          <button class="secondary-button" type="button" :disabled="saving" @click="refreshLists">
            Refresh Lists
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
            <h2>Create Draft</h2>
            <span>new working copy</span>
          </header>
          <div class="field-grid">
            <label class="field">
              <span>Cluster</span>
              <input v-model="createForm.cluster_id" />
            </label>
            <label class="field">
              <span>Title</span>
              <input v-model="createForm.title" />
            </label>
            <label class="field field--full">
              <span>Summary</span>
              <input v-model="createForm.summary" />
            </label>
            <label class="field field--full">
              <span>Source Path</span>
              <input v-model="createForm.source_path" />
            </label>
            <label class="field field--full">
              <span>Base Revision</span>
              <select v-model="createForm.base_revision_id">
                <option value="">latest revision / empty</option>
                <option
                  v-for="revision in revisions"
                  :key="revision.revision_id"
                  :value="revision.revision_id"
                >
                  {{ revision.version_label }} · {{ revision.revision_id }}
                </option>
              </select>
            </label>
            <label class="field field--full">
              <span>Config Text</span>
              <textarea v-model="createForm.config_text" class="code-textarea" rows="14" />
            </label>
          </div>
          <button class="primary-button" type="button" :disabled="saving" @click="handleCreateDraft">
            Create Draft
          </button>
        </article>
      </section>

      <section class="panel-grid">
        <article class="panel">
          <header class="panel__header">
            <h2>Drafts</h2>
            <span>{{ drafts.length }} items</span>
          </header>
          <p v-if="!drafts.length" class="empty-state">还没有配置草稿。</p>
          <div v-else class="list-stack">
            <button
              v-for="draft in drafts"
              :key="draft.draft_id"
              class="list-card"
              :class="{ 'list-card--active': draft.draft_id === selectedDraft?.draft_id }"
              type="button"
              @click="selectDraft(draft.draft_id)"
            >
              <strong>{{ draft.title }}</strong>
              <div class="cell-meta">{{ draft.cluster_id }} · {{ draft.validation_state }}</div>
              <div class="cell-meta">
                base {{ draft.base_revision_id ?? "-" }} · published {{ draft.published_revision_id ?? "-" }}
              </div>
              <div class="cell-meta">updated {{ formatUnixMs(draft.updated_at_unix_ms) }}</div>
            </button>
          </div>
        </article>

        <article class="panel">
          <header class="panel__header">
            <h2>Revisions</h2>
            <span>{{ revisions.length }} items</span>
          </header>
          <div class="toolbar-links">
            <RouterLink
              v-if="selectedRevision"
              class="secondary-button secondary-button--link"
              :to="{ name: 'deployments', query: { revision_id: selectedRevision.revision_id } }"
            >
              Deploy Selected Revision
            </RouterLink>
          </div>
          <p v-if="!revisions.length" class="empty-state">还没有可发布 revision。</p>
          <div v-else class="list-stack">
            <button
              v-for="revision in revisions"
              :key="revision.revision_id"
              class="list-card"
              :class="{ 'list-card--active': revision.revision_id === selectedRevision?.revision_id }"
              type="button"
              @click="selectRevision(revision.revision_id)"
            >
              <strong>{{ revision.version_label }}</strong>
              <div class="cell-meta">{{ revision.cluster_id }} · {{ revision.created_by }}</div>
              <div class="cell-meta">{{ revision.summary }}</div>
              <div class="cell-meta">{{ formatUnixMs(revision.created_at_unix_ms) }}</div>
            </button>
          </div>
        </article>
      </section>

      <section v-if="selectedDraft" class="panel-grid">
        <article class="panel">
          <header class="panel__header">
            <h2>Draft Editor</h2>
            <span>{{ selectedDraft.validation_state }}</span>
          </header>
          <div class="field-grid">
            <label class="field">
              <span>Title</span>
              <input v-model="draftForm.title" :readonly="!canManage" />
            </label>
            <label class="field">
              <span>Cluster</span>
              <input :value="selectedDraft.cluster_id" readonly />
            </label>
            <label class="field field--full">
              <span>Summary</span>
              <input v-model="draftForm.summary" :readonly="!canManage" />
            </label>
            <label class="field field--full">
              <span>Source Path</span>
              <input v-model="draftForm.source_path" :readonly="!canManage" />
            </label>
            <label class="field field--full">
              <span>Base Revision</span>
              <select v-model="draftForm.base_revision_id" :disabled="!canManage">
                <option value="">latest revision / empty</option>
                <option
                  v-for="revision in revisions"
                  :key="revision.revision_id"
                  :value="revision.revision_id"
                >
                  {{ revision.version_label }} · {{ revision.revision_id }}
                </option>
              </select>
            </label>
            <label class="field field--full">
              <span>Config Text</span>
              <textarea
                v-model="draftForm.config_text"
                class="code-textarea"
                rows="20"
                :readonly="!canManage"
              />
            </label>
          </div>
          <div class="toolbar-links">
            <button class="secondary-button" type="button" :disabled="saving" @click="handleLoadDiff">
              Load Diff
            </button>
            <button
              v-if="canManage"
              class="secondary-button"
              type="button"
              :disabled="saving"
              @click="handleSaveDraft"
            >
              Save Draft
            </button>
            <button
              v-if="canManage"
              class="secondary-button"
              type="button"
              :disabled="saving"
              @click="handleValidateDraft"
            >
              Validate / Compile
            </button>
          </div>
        </article>

        <article class="panel">
          <header class="panel__header">
            <h2>Publish Draft</h2>
            <span>{{ selectedDraft.published_revision_id ?? "unpublished" }}</span>
          </header>
          <dl class="kv-grid">
            <div>
              <dt>Created By</dt>
              <dd>{{ selectedDraft.created_by }}</dd>
            </div>
            <div>
              <dt>Updated By</dt>
              <dd>{{ selectedDraft.updated_by }}</dd>
            </div>
            <div>
              <dt>Created At</dt>
              <dd>{{ formatUnixMs(selectedDraft.created_at_unix_ms) }}</dd>
            </div>
            <div>
              <dt>Updated At</dt>
              <dd>{{ formatUnixMs(selectedDraft.updated_at_unix_ms) }}</dd>
            </div>
          </dl>
          <div v-if="canManage" class="field-grid">
            <label class="field">
              <span>Version Label</span>
              <input v-model="publishForm.version_label" />
            </label>
            <label class="field field--full">
              <span>Revision Summary</span>
              <input v-model="publishForm.summary" />
            </label>
          </div>
          <button
            v-if="canManage"
            class="primary-button"
            type="button"
            :disabled="saving"
            @click="handlePublishDraft"
          >
            Publish Revision
          </button>
        </article>
      </section>

      <section v-if="compileMetrics.length" class="metric-grid">
        <MetricCard
          v-for="metric in compileMetrics"
          :key="metric.title"
          :title="metric.title"
          :value="metric.value"
          :description="metric.description"
        />
      </section>

      <section class="panel-grid">
        <article v-if="selectedDraft?.last_validation" class="panel">
          <header class="panel__header">
            <h2>Validation Report</h2>
            <span>{{ selectedDraft.last_validation.valid ? "valid" : "invalid" }}</span>
          </header>
          <dl class="kv-grid">
            <div>
              <dt>Validated At</dt>
              <dd>{{ formatUnixMs(selectedDraft.last_validation.validated_at_unix_ms) }}</dd>
            </div>
            <div>
              <dt>Source Path</dt>
              <dd>{{ selectedDraft.last_validation.normalized_source_path }}</dd>
            </div>
            <div>
              <dt>Issues</dt>
              <dd>{{ selectedDraft.last_validation.issues.length }}</dd>
            </div>
            <div>
              <dt>Upstreams</dt>
              <dd>{{ selectedDraft.last_validation.summary?.upstream_names.length ?? 0 }}</dd>
            </div>
          </dl>
          <ul v-if="selectedDraft.last_validation.issues.length" class="issues-list">
            <li v-for="issue in selectedDraft.last_validation.issues" :key="issue">{{ issue }}</li>
          </ul>
        </article>

        <article v-if="selectedRevision" class="panel">
          <header class="panel__header">
            <h2>Revision Detail</h2>
            <span>{{ selectedRevision.version_label }}</span>
          </header>
          <dl class="kv-grid">
            <div>
              <dt>Revision ID</dt>
              <dd>{{ selectedRevision.revision_id }}</dd>
            </div>
            <div>
              <dt>Cluster</dt>
              <dd>{{ selectedRevision.cluster_id }}</dd>
            </div>
            <div>
              <dt>Created By</dt>
              <dd>{{ selectedRevision.created_by }}</dd>
            </div>
            <div>
              <dt>Created At</dt>
              <dd>{{ formatUnixMs(selectedRevision.created_at_unix_ms) }}</dd>
            </div>
            <div>
              <dt>Source Path</dt>
              <dd>{{ selectedRevision.source_path }}</dd>
            </div>
            <div>
              <dt>Upstreams</dt>
              <dd>{{ formatList(selectedRevision.compile_summary?.upstream_names ?? []) }}</dd>
            </div>
          </dl>
          <label class="field field--full">
            <span>Config Text</span>
            <textarea :value="selectedRevision.config_text" class="code-textarea" rows="16" readonly />
          </label>
        </article>
      </section>

      <article v-if="diff" class="panel panel--stack">
        <header class="panel__header">
          <h2>Diff</h2>
          <span>{{ diff.left_label }} -> {{ diff.right_label }}</span>
        </header>
        <p class="cell-meta">changed: {{ diff.changed ? "yes" : "no" }}</p>
        <div class="diff-view">
          <div
            v-for="(line, index) in diff.lines"
            :key="`${index}-${line.left_line_number}-${line.right_line_number}`"
            :class="diffClass(line)"
          >
            <span class="diff-gutter">{{ line.left_line_number ?? "" }}</span>
            <span class="diff-gutter">{{ line.right_line_number ?? "" }}</span>
            <code>{{ line.text || " " }}</code>
          </div>
        </div>
      </article>
    </template>
  </section>
</template>
