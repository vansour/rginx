<script setup lang="ts">
import { computed } from "vue";
import { useRoute } from "vue-router";

import type {
  ListenerStatsSnapshot,
  PeerHealthSnapshot,
  ReloadResultSnapshot,
  RouteStatsSnapshot,
  RuntimeListenerBindingSnapshot,
  RuntimeListenerSnapshot,
  UpstreamHealthSnapshot,
  UpstreamPeerStatsSnapshot,
  UpstreamStatsSnapshot,
  VhostStatsSnapshot,
} from "../api/controlPlane";
import MetricCard from "../components/MetricCard.vue";
import { useNodeDetailStream } from "../composables/useNodeDetailStream";
import {
  formatBoolean,
  formatList,
  formatNullable,
  formatUnixMs,
  streamStateLabel,
} from "../lib/display";

const route = useRoute();
const nodeId = computed(() => String(route.params.nodeId ?? ""));
const { actor, detail, error, loading, reload, streamState } = useNodeDetailStream(nodeId);

const snapshot = computed(() => detail.value?.latest_snapshot ?? null);
const runtime = computed(() => snapshot.value?.status ?? null);
const counters = computed(() => snapshot.value?.counters ?? null);
const traffic = computed(() => snapshot.value?.traffic ?? null);
const peerHealth = computed(() => snapshot.value?.peer_health ?? []);
const upstreams = computed(() => snapshot.value?.upstreams ?? []);

const metrics = computed(() => {
  if (!detail.value) {
    return [];
  }

  const currentRuntime = runtime.value;
  const currentSnapshot = snapshot.value;
  return [
    {
      title: "State",
      value: detail.value.node.state,
      description: detail.value.node.status_reason ?? "节点生命周期状态",
    },
    {
      title: "Snapshot",
      value: currentSnapshot?.snapshot_version ?? "-",
      description: currentSnapshot
        ? `captured ${formatUnixMs(currentSnapshot.captured_at_unix_ms)}`
        : "尚未收到完整 snapshot",
    },
    {
      title: "Listeners",
      value: currentRuntime?.listeners.length ?? 0,
      description: "当前运行态中的 listener 数量",
    },
    {
      title: "Connections",
      value: currentRuntime?.active_connections ?? 0,
      description: "当前活跃连接数",
    },
    {
      title: "Routes",
      value: currentRuntime?.total_routes ?? 0,
      description: "当前路由总数",
    },
    {
      title: "Upstreams",
      value: currentRuntime?.total_upstreams ?? 0,
      description: "当前 upstream 总数",
    },
  ];
});

function bindingSummary(binding: RuntimeListenerBindingSnapshot): string {
  return `${binding.transport} ${binding.listen_addr} (${binding.worker_count} workers)`;
}

function listenerFeatures(listener: RuntimeListenerSnapshot): string {
  return [
    `tls ${formatBoolean(listener.tls_enabled, "on", "off")}`,
    `http3 ${formatBoolean(listener.http3_enabled, "on", "off")}`,
    `proxy-proto ${formatBoolean(listener.proxy_protocol_enabled, "on", "off")}`,
    `keep-alive ${formatBoolean(listener.keep_alive, "on", "off")}`,
  ].join(" · ");
}

function listenerTrafficSummary(listener: ListenerStatsSnapshot): string {
  return [
    `accepted ${listener.downstream_connections_accepted}`,
    `unmatched ${listener.unmatched_requests_total}`,
    `grpc ${listener.grpc.requests_total}`,
    listener.http3_runtime ? `h3 streams ${listener.http3_runtime.active_request_streams}` : null,
  ]
    .filter((value): value is string => Boolean(value))
    .join(" · ");
}

function vhostTrafficSummary(vhost: VhostStatsSnapshot): string {
  return [
    `unmatched ${vhost.unmatched_requests_total}`,
    `grpc ${vhost.grpc.requests_total}`,
    `recent ${vhost.recent_60s.downstream_requests_total}/${vhost.recent_60s.window_secs}s`,
  ].join(" · ");
}

function routeTrafficSummary(routeEntry: RouteStatsSnapshot): string {
  return [
    `rate-limited ${routeEntry.rate_limited_total}`,
    `denied ${routeEntry.access_denied_total}`,
    `grpc ${routeEntry.grpc.requests_total}`,
  ].join(" · ");
}

function peerSummary(peer: PeerHealthSnapshot): string {
  return [
    peer.peer_url,
    peer.available ? "available" : "cooldown",
    peer.active_unhealthy ? "active-unhealthy" : "active-healthy",
    `req ${peer.active_requests}`,
  ].join(" · ");
}

function upstreamHealthSummary(upstream: UpstreamHealthSnapshot): string {
  return upstream.peers.map(peerSummary).join(" | ");
}

function upstreamTlsSummary(upstream: UpstreamStatsSnapshot): string {
  return [
    upstream.tls.protocol,
    upstream.tls.verify_mode,
    `sni ${formatBoolean(upstream.tls.server_name_enabled, "on", "off")}`,
  ].join(" · ");
}

function upstreamPeersSummary(peers: UpstreamPeerStatsSnapshot[]): string {
  return peers
    .map(
      (peer) =>
        `${peer.peer_url} (${peer.successes_total}/${peer.attempts_total}, timeout ${peer.timeouts_total})`,
    )
    .join(" | ");
}

function formatReloadResult(result: ReloadResultSnapshot | null): string {
  if (!result) {
    return "-";
  }

  const success = (result.outcome as { Success?: { revision: number } }).Success;
  if (success) {
    return `success · rev ${success.revision}`;
  }

  const failure = (result.outcome as { Failure?: { error: string } }).Failure;
  if (failure) {
    return `failure · ${failure.error}`;
  }

  return JSON.stringify(result.outcome);
}
</script>

<template>
  <section class="page-shell">
    <header class="hero">
      <div>
        <p class="eyebrow">node detail</p>
        <div class="breadcrumb-row">
          <RouterLink class="breadcrumb-link" :to="{ name: 'dashboard' }">Dashboard</RouterLink>
          <span>/</span>
          <span>Nodes</span>
        </div>
        <h1>{{ detail?.node.node_id ?? nodeId }}</h1>
        <p class="hero-copy">
          节点详情页聚合控制面落库的完整 snapshot、运行态 listeners / vhosts / routes /
          upstreams，以及最近的节点审计事件。
        </p>
      </div>
      <div class="hero-meta">
        <p><strong>cluster</strong> {{ detail?.node.cluster_id ?? "-" }}</p>
        <p>
          <strong>state</strong>
          <span v-if="detail" :class="['state-pill', `state-pill--${detail.node.state}`]">
            {{ detail.node.state }}
          </span>
          <span v-else>-</span>
        </p>
        <p>
          <strong>stream</strong>
          <span :class="['realtime-pill', `realtime-pill--${streamState}`]">
            {{ streamStateLabel(streamState) }}
          </span>
        </p>
        <p><strong>user</strong> {{ actor?.user.username ?? "-" }}</p>
        <p><strong>last seen</strong> {{ formatUnixMs(detail?.node.last_seen_unix_ms) }}</p>
        <p><strong>addr</strong> {{ detail?.node.advertise_addr ?? "-" }}</p>
      </div>
    </header>

    <p v-if="loading" class="state-banner">正在加载节点详情…</p>
    <p v-else-if="error && !detail" class="state-banner state-banner--error">{{ error }}</p>

    <template v-if="detail">
      <section class="toolbar">
        <div class="toolbar-links">
          <RouterLink class="secondary-button secondary-button--link" :to="{ name: 'dashboard' }">
            Dashboard
          </RouterLink>
          <RouterLink
            class="secondary-button secondary-button--link"
            :to="{ name: 'node-tls', params: { nodeId } }"
          >
            TLS / OCSP
          </RouterLink>
          <button class="secondary-button" type="button" @click="reload">Refresh</button>
        </div>
        <div class="identity-card">
          <p class="identity-card__name">{{ actor?.user.display_name ?? "viewer" }}</p>
          <p class="identity-card__meta">
            {{ actor?.user.roles.join(", ") ?? "-" }} · {{ detail.node.cluster_id }}
          </p>
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

      <section class="panel-grid">
        <article class="panel">
          <header class="panel__header">
            <h2>Node Summary</h2>
            <span>{{ detail.node.running_version }}</span>
          </header>
          <dl class="kv-grid">
            <div>
              <dt>Advertise</dt>
              <dd>{{ detail.node.advertise_addr }}</dd>
            </div>
            <div>
              <dt>Role</dt>
              <dd>{{ detail.node.role }}</dd>
            </div>
            <div>
              <dt>Admin Socket</dt>
              <dd>{{ detail.node.admin_socket_path }}</dd>
            </div>
            <div>
              <dt>Status Reason</dt>
              <dd>{{ detail.node.status_reason ?? "healthy" }}</dd>
            </div>
            <div>
              <dt>Last Snapshot</dt>
              <dd>{{ detail.node.last_snapshot_version ?? "-" }}</dd>
            </div>
            <div>
              <dt>Runtime Revision</dt>
              <dd>{{ detail.node.runtime_revision ?? "-" }}</dd>
            </div>
            <div>
              <dt>Runtime PID</dt>
              <dd>{{ detail.node.runtime_pid ?? "-" }}</dd>
            </div>
            <div>
              <dt>Last Seen</dt>
              <dd>{{ formatUnixMs(detail.node.last_seen_unix_ms) }}</dd>
            </div>
            <div>
              <dt>Included Modules</dt>
              <dd>{{ formatList(snapshot?.included_modules ?? []) }}</dd>
            </div>
          </dl>
        </article>

        <article class="panel">
          <header class="panel__header">
            <h2>Runtime Summary</h2>
            <span>revision {{ runtime?.revision ?? "-" }}</span>
          </header>
          <dl class="kv-grid">
            <div>
              <dt>Config Path</dt>
              <dd>{{ runtime?.config_path ?? "-" }}</dd>
            </div>
            <div>
              <dt>Workers</dt>
              <dd>{{ runtime?.worker_threads ?? "-" }}</dd>
            </div>
            <div>
              <dt>Accept Workers</dt>
              <dd>{{ runtime?.accept_workers ?? "-" }}</dd>
            </div>
            <div>
              <dt>Listeners</dt>
              <dd>{{ runtime?.listeners.length ?? 0 }}</dd>
            </div>
            <div>
              <dt>VHosts</dt>
              <dd>{{ runtime?.total_vhosts ?? 0 }}</dd>
            </div>
            <div>
              <dt>Routes</dt>
              <dd>{{ runtime?.total_routes ?? 0 }}</dd>
            </div>
            <div>
              <dt>Upstreams</dt>
              <dd>{{ runtime?.total_upstreams ?? 0 }}</dd>
            </div>
            <div>
              <dt>Active Connections</dt>
              <dd>{{ runtime?.active_connections ?? 0 }}</dd>
            </div>
            <div>
              <dt>HTTP/3 Active</dt>
              <dd>{{ runtime?.http3_active_connections ?? 0 }}</dd>
            </div>
            <div>
              <dt>Reload</dt>
              <dd>{{ formatReloadResult(runtime?.reload.last_result ?? null) }}</dd>
            </div>
          </dl>
        </article>

        <article class="panel">
          <header class="panel__header">
            <h2>HTTP Counters</h2>
            <span>{{ snapshot?.snapshot_version ?? "-" }}</span>
          </header>
          <dl class="kv-grid">
            <div>
              <dt>Accepted</dt>
              <dd>{{ counters?.downstream_connections_accepted ?? 0 }}</dd>
            </div>
            <div>
              <dt>Rejected</dt>
              <dd>{{ counters?.downstream_connections_rejected ?? 0 }}</dd>
            </div>
            <div>
              <dt>Requests</dt>
              <dd>{{ counters?.downstream_requests ?? 0 }}</dd>
            </div>
            <div>
              <dt>Responses</dt>
              <dd>{{ counters?.downstream_responses ?? 0 }}</dd>
            </div>
            <div>
              <dt>2xx / 4xx / 5xx</dt>
              <dd>
                {{ counters?.downstream_responses_2xx ?? 0 }} /
                {{ counters?.downstream_responses_4xx ?? 0 }} /
                {{ counters?.downstream_responses_5xx ?? 0 }}
              </dd>
            </div>
            <div>
              <dt>mTLS Auth</dt>
              <dd>
                {{ counters?.downstream_mtls_authenticated_connections ?? 0 }} conn /
                {{ counters?.downstream_mtls_authenticated_requests ?? 0 }} req
              </dd>
            </div>
            <div>
              <dt>TLS Failures</dt>
              <dd>{{ counters?.downstream_tls_handshake_failures ?? 0 }}</dd>
            </div>
            <div>
              <dt>0-RTT</dt>
              <dd>
                {{ counters?.downstream_http3_early_data_accepted_requests ?? 0 }} accepted /
                {{ counters?.downstream_http3_early_data_rejected_requests ?? 0 }} rejected
              </dd>
            </div>
          </dl>
        </article>
      </section>

      <article class="panel panel--stack">
        <header class="panel__header">
          <h2>Runtime Listeners</h2>
          <span>{{ runtime?.listeners.length ?? 0 }} listeners</span>
        </header>
        <p v-if="!runtime?.listeners.length" class="empty-state">当前 snapshot 中没有 listener 视图。</p>
        <div v-else class="table-scroll">
          <table class="data-table">
            <thead>
              <tr>
                <th>Listener</th>
                <th>Bindings</th>
                <th>Features</th>
                <th>Certificate</th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="listener in runtime.listeners" :key="listener.listener_id">
                <td>
                  <strong>{{ listener.listener_name }}</strong>
                  <div class="cell-meta">{{ listener.listen_addr }} · {{ listener.listener_id }}</div>
                </td>
                <td>{{ formatList(listener.bindings.map(bindingSummary)) }}</td>
                <td>{{ listenerFeatures(listener) }}</td>
                <td>{{ listener.default_certificate ?? "-" }}</td>
              </tr>
            </tbody>
          </table>
        </div>
      </article>

      <section class="panel-grid">
        <article class="panel">
          <header class="panel__header">
            <h2>Listener Traffic</h2>
            <span>{{ traffic?.listeners.length ?? 0 }} listeners</span>
          </header>
          <p v-if="!traffic?.listeners.length" class="empty-state">没有 listener traffic 数据。</p>
          <div v-else class="table-scroll">
            <table class="data-table">
              <thead>
                <tr>
                  <th>Listener</th>
                  <th>Requests</th>
                  <th>Responses</th>
                  <th>4xx / 5xx</th>
                  <th>Notes</th>
                </tr>
              </thead>
              <tbody>
                <tr v-for="listener in traffic.listeners" :key="listener.listener_id">
                  <td>
                    <strong>{{ listener.listener_name }}</strong>
                    <div class="cell-meta">{{ listener.listen_addr }}</div>
                  </td>
                  <td>{{ listener.downstream_requests }}</td>
                  <td>{{ listener.downstream_responses }}</td>
                  <td>{{ listener.downstream_responses_4xx }} / {{ listener.downstream_responses_5xx }}</td>
                  <td>{{ listenerTrafficSummary(listener) }}</td>
                </tr>
              </tbody>
            </table>
          </div>
        </article>

        <article class="panel">
          <header class="panel__header">
            <h2>VHost Traffic</h2>
            <span>{{ traffic?.vhosts.length ?? 0 }} vhosts</span>
          </header>
          <p v-if="!traffic?.vhosts.length" class="empty-state">没有 vhost traffic 数据。</p>
          <div v-else class="table-scroll">
            <table class="data-table">
              <thead>
                <tr>
                  <th>VHost</th>
                  <th>Requests</th>
                  <th>Responses</th>
                  <th>4xx / 5xx</th>
                  <th>Notes</th>
                </tr>
              </thead>
              <tbody>
                <tr v-for="vhost in traffic.vhosts" :key="vhost.vhost_id">
                  <td>
                    <strong>{{ vhost.vhost_id }}</strong>
                    <div class="cell-meta">{{ formatList(vhost.server_names) }}</div>
                  </td>
                  <td>{{ vhost.downstream_requests }}</td>
                  <td>{{ vhost.downstream_responses }}</td>
                  <td>{{ vhost.downstream_responses_4xx }} / {{ vhost.downstream_responses_5xx }}</td>
                  <td>{{ vhostTrafficSummary(vhost) }}</td>
                </tr>
              </tbody>
            </table>
          </div>
        </article>
      </section>

      <article class="panel panel--stack">
        <header class="panel__header">
          <h2>Route Traffic</h2>
          <span>{{ traffic?.routes.length ?? 0 }} routes</span>
        </header>
        <p v-if="!traffic?.routes.length" class="empty-state">没有 route traffic 数据。</p>
        <div v-else class="table-scroll">
          <table class="data-table">
            <thead>
              <tr>
                <th>Route</th>
                <th>Requests</th>
                <th>Responses</th>
                <th>2xx / 4xx / 5xx</th>
                <th>Policy</th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="routeEntry in traffic.routes" :key="`${routeEntry.vhost_id}-${routeEntry.route_id}`">
                <td>
                  <strong>{{ routeEntry.route_id }}</strong>
                  <div class="cell-meta">{{ routeEntry.vhost_id }}</div>
                </td>
                <td>{{ routeEntry.downstream_requests }}</td>
                <td>{{ routeEntry.downstream_responses }}</td>
                <td>
                  {{ routeEntry.downstream_responses_2xx }} /
                  {{ routeEntry.downstream_responses_4xx }} /
                  {{ routeEntry.downstream_responses_5xx }}
                </td>
                <td>{{ routeTrafficSummary(routeEntry) }}</td>
              </tr>
            </tbody>
          </table>
        </div>
      </article>

      <section class="panel-grid">
        <article class="panel">
          <header class="panel__header">
            <h2>Upstream Health</h2>
            <span>{{ peerHealth.length }} upstreams</span>
          </header>
          <p v-if="!peerHealth.length" class="empty-state">没有 peer health 数据。</p>
          <div v-else class="table-scroll">
            <table class="data-table">
              <thead>
                <tr>
                  <th>Upstream</th>
                  <th>Policy</th>
                  <th>Peers</th>
                </tr>
              </thead>
              <tbody>
                <tr v-for="upstream in peerHealth" :key="upstream.upstream_name">
                  <td>{{ upstream.upstream_name }}</td>
                  <td>
                    failures {{ upstream.unhealthy_after_failures }} · cooldown
                    {{ upstream.cooldown_ms }}ms · active
                    {{ formatBoolean(upstream.active_health_enabled, "on", "off") }}
                  </td>
                  <td>{{ upstreamHealthSummary(upstream) }}</td>
                </tr>
              </tbody>
            </table>
          </div>
        </article>

        <article class="panel">
          <header class="panel__header">
            <h2>Upstream Stats</h2>
            <span>{{ upstreams.length }} upstreams</span>
          </header>
          <p v-if="!upstreams.length" class="empty-state">没有 upstream 统计数据。</p>
          <div v-else class="table-scroll">
            <table class="data-table">
              <thead>
                <tr>
                  <th>Upstream</th>
                  <th>Requests / Attempts</th>
                  <th>Failures</th>
                  <th>TLS</th>
                  <th>Peers</th>
                </tr>
              </thead>
              <tbody>
                <tr v-for="upstream in upstreams" :key="upstream.upstream_name">
                  <td>{{ upstream.upstream_name }}</td>
                  <td>{{ upstream.downstream_requests_total }} / {{ upstream.peer_attempts_total }}</td>
                  <td>
                    fail {{ upstream.peer_failures_total }} · 502 {{ upstream.bad_gateway_responses_total }}
                    · 504 {{ upstream.gateway_timeout_responses_total }} · no-healthy
                    {{ upstream.no_healthy_peers_total }}
                  </td>
                  <td>{{ upstreamTlsSummary(upstream) }}</td>
                  <td>{{ upstreamPeersSummary(upstream.peers) }}</td>
                </tr>
              </tbody>
            </table>
          </div>
        </article>
      </section>

      <section class="panel-grid">
        <article class="panel">
          <header class="panel__header">
            <h2>Recent Snapshots</h2>
            <span>{{ detail.recent_snapshots.length }} entries</span>
          </header>
          <p v-if="!detail.recent_snapshots.length" class="empty-state">暂无 snapshot 历史。</p>
          <table v-else class="data-table">
            <thead>
              <tr>
                <th>Version</th>
                <th>Captured</th>
                <th>PID</th>
                <th>Binary</th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="entry in detail.recent_snapshots" :key="entry.snapshot_version">
                <td>
                  <strong>{{ entry.snapshot_version }}</strong>
                  <div class="cell-meta">schema {{ entry.schema_version }}</div>
                </td>
                <td>{{ formatUnixMs(entry.captured_at_unix_ms) }}</td>
                <td>{{ entry.pid }}</td>
                <td>
                  {{ entry.binary_version }}
                  <div class="cell-meta">{{ formatList(entry.included_modules) }}</div>
                </td>
              </tr>
            </tbody>
          </table>
        </article>

        <article class="panel">
          <header class="panel__header">
            <h2>Recent Events</h2>
            <span>{{ detail.recent_events.length }} entries</span>
          </header>
          <p v-if="!detail.recent_events.length" class="empty-state">暂无节点审计事件。</p>
          <table v-else class="data-table">
            <thead>
              <tr>
                <th>Time</th>
                <th>Action</th>
                <th>Actor</th>
                <th>Result</th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="entry in detail.recent_events" :key="entry.audit_id">
                <td>{{ formatUnixMs(entry.created_at_unix_ms) }}</td>
                <td>
                  <strong>{{ entry.action }}</strong>
                  <div class="cell-meta">{{ entry.request_id }}</div>
                </td>
                <td>{{ formatNullable(entry.actor_id) }}</td>
                <td>
                  {{ entry.result }}
                  <div class="cell-meta">{{ entry.resource_type }}/{{ entry.resource_id }}</div>
                </td>
              </tr>
            </tbody>
          </table>
        </article>
      </section>
    </template>
  </section>
</template>
