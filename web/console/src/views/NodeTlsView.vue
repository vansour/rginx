<script setup lang="ts">
import { computed } from "vue";
import { useRoute } from "vue-router";

import MetricCard from "../components/MetricCard.vue";
import { useNodeDetailStream } from "../composables/useNodeDetailStream";
import {
  formatBoolean,
  formatList,
  formatUnixMs,
  streamStateLabel,
} from "../lib/display";

const route = useRoute();
const nodeId = computed(() => String(route.params.nodeId ?? ""));
const { actor, detail, error, loading, reload, streamState } = useNodeDetailStream(nodeId);

const snapshot = computed(() => detail.value?.latest_snapshot ?? null);
const runtime = computed(() => snapshot.value?.status ?? null);
const tls = computed(() => runtime.value?.tls ?? null);
const mtls = computed(() => runtime.value?.mtls ?? null);
const upstreamTls = computed(() => runtime.value?.upstream_tls ?? []);

const metrics = computed(() => {
  return [
    {
      title: "TLS",
      value: formatBoolean(runtime.value?.tls_enabled, "enabled", "disabled"),
      description: "节点当前 TLS 总状态",
    },
    {
      title: "TLS Listeners",
      value: tls.value?.listeners.length ?? 0,
      description: "带证书或 TLS 配置的 listener 数量",
    },
    {
      title: "Certificates",
      value: tls.value?.certificates.length ?? 0,
      description: "控制面快照中的证书条目数量",
    },
    {
      title: "Expiring",
      value: tls.value?.expiring_certificate_count ?? 0,
      description: "即将过期的证书数量",
    },
    {
      title: "OCSP",
      value: tls.value?.ocsp.length ?? 0,
      description: "OCSP 状态条目数量",
    },
    {
      title: "Upstream TLS",
      value: upstreamTls.value.length,
      description: "上游 TLS 配置与校验概览",
    },
  ];
});

function tlsListenerFeatures(): string {
  return [
    `tls ${formatBoolean(runtime.value?.tls_enabled, "on", "off")}`,
    `mtls required ${mtls.value?.required_listeners ?? 0}`,
    `0-rtt listeners ${runtime.value?.http3_early_data_enabled_listeners ?? 0}`,
  ].join(" · ");
}
</script>

<template>
  <section class="page-shell">
    <header class="hero">
      <div>
        <p class="eyebrow">node tls</p>
        <div class="breadcrumb-row">
          <RouterLink class="breadcrumb-link" :to="{ name: 'dashboard' }">Dashboard</RouterLink>
          <span>/</span>
          <RouterLink class="breadcrumb-link" :to="{ name: 'node-detail', params: { nodeId } }">
            {{ detail?.node.node_id ?? nodeId }}
          </RouterLink>
          <span>/</span>
          <span>TLS</span>
        </div>
        <h1>{{ detail?.node.node_id ?? nodeId }} TLS / OCSP</h1>
        <p class="hero-copy">
          该页集中展示 listener 证书、OCSP、mTLS、SNI 绑定和 upstream TLS 诊断，避免再通过 SSH
          或本地 CLI 手工拼接只读排障信息。
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
        <p><strong>snapshot</strong> {{ snapshot?.snapshot_version ?? "-" }}</p>
        <p><strong>captured</strong> {{ formatUnixMs(snapshot?.captured_at_unix_ms) }}</p>
      </div>
    </header>

    <p v-if="loading" class="state-banner">正在加载 TLS / OCSP 详情…</p>
    <p v-else-if="error && !detail" class="state-banner state-banner--error">{{ error }}</p>

    <template v-if="detail">
      <section class="toolbar">
        <div class="toolbar-links">
          <RouterLink class="secondary-button secondary-button--link" :to="{ name: 'dashboard' }">
            Dashboard
          </RouterLink>
          <RouterLink
            class="secondary-button secondary-button--link"
            :to="{ name: 'node-detail', params: { nodeId } }"
          >
            Node Detail
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
            <h2>TLS Runtime</h2>
            <span>{{ tls?.listeners.length ?? 0 }} listeners</span>
          </header>
          <dl class="kv-grid">
            <div>
              <dt>Feature Summary</dt>
              <dd>{{ tlsListenerFeatures() }}</dd>
            </div>
            <div>
              <dt>Expiring Certificates</dt>
              <dd>{{ tls?.expiring_certificate_count ?? 0 }}</dd>
            </div>
            <div>
              <dt>SNI Bindings</dt>
              <dd>{{ tls?.sni_bindings.length ?? 0 }}</dd>
            </div>
            <div>
              <dt>SNI Conflicts</dt>
              <dd>{{ tls?.sni_conflicts.length ?? 0 }}</dd>
            </div>
            <div>
              <dt>Default Bindings</dt>
              <dd>{{ tls?.default_certificate_bindings.length ?? 0 }}</dd>
            </div>
            <div>
              <dt>Reloadable Fields</dt>
              <dd>{{ formatList(tls?.reload_boundary.reloadable_fields ?? []) }}</dd>
            </div>
            <div>
              <dt>Restart Required</dt>
              <dd>{{ formatList(tls?.reload_boundary.restart_required_fields ?? []) }}</dd>
            </div>
          </dl>
        </article>

        <article class="panel">
          <header class="panel__header">
            <h2>mTLS Summary</h2>
            <span>{{ mtls?.required_listeners ?? 0 }} required</span>
          </header>
          <dl class="kv-grid">
            <div>
              <dt>Configured</dt>
              <dd>{{ mtls?.configured_listeners ?? 0 }}</dd>
            </div>
            <div>
              <dt>Optional</dt>
              <dd>{{ mtls?.optional_listeners ?? 0 }}</dd>
            </div>
            <div>
              <dt>Required</dt>
              <dd>{{ mtls?.required_listeners ?? 0 }}</dd>
            </div>
            <div>
              <dt>Authenticated Connections</dt>
              <dd>{{ mtls?.authenticated_connections ?? 0 }}</dd>
            </div>
            <div>
              <dt>Authenticated Requests</dt>
              <dd>{{ mtls?.authenticated_requests ?? 0 }}</dd>
            </div>
            <div>
              <dt>Anonymous Requests</dt>
              <dd>{{ mtls?.anonymous_requests ?? 0 }}</dd>
            </div>
            <div>
              <dt>Handshake Failures</dt>
              <dd>{{ mtls?.handshake_failures_total ?? 0 }}</dd>
            </div>
            <div>
              <dt>Verify Depth Exceeded</dt>
              <dd>{{ mtls?.handshake_failures_verify_depth_exceeded ?? 0 }}</dd>
            </div>
          </dl>
        </article>
      </section>

      <article class="panel panel--stack">
        <header class="panel__header">
          <h2>TLS Listeners</h2>
          <span>{{ tls?.listeners.length ?? 0 }} listeners</span>
        </header>
        <p v-if="!tls?.listeners.length" class="empty-state">当前 snapshot 中没有 TLS listener 信息。</p>
        <div v-else class="table-scroll">
          <table class="data-table">
            <thead>
              <tr>
                <th>Listener</th>
                <th>Protocols</th>
                <th>HTTP/3</th>
                <th>Client Auth</th>
                <th>Certificate</th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="listener in tls.listeners" :key="listener.listener_id">
                <td>
                  <strong>{{ listener.listener_name }}</strong>
                  <div class="cell-meta">{{ listener.listen_addr }}</div>
                </td>
                <td>{{ formatList(listener.alpn_protocols) }}</td>
                <td>
                  {{ formatBoolean(listener.http3_enabled, "enabled", "disabled") }}
                  <div class="cell-meta">{{ listener.http3_listen_addr ?? "-" }}</div>
                </td>
                <td>
                  {{ listener.client_auth_mode ?? "none" }}
                  <div class="cell-meta">
                    depth {{ listener.client_auth_verify_depth ?? "-" }} · crl
                    {{ formatBoolean(listener.client_auth_crl_configured, "on", "off") }}
                  </div>
                </td>
                <td>
                  {{ listener.default_certificate ?? "-" }}
                  <div class="cell-meta">SNI {{ formatList(listener.sni_names) }}</div>
                </td>
              </tr>
            </tbody>
          </table>
        </div>
      </article>

      <section class="panel-grid">
        <article class="panel">
          <header class="panel__header">
            <h2>Certificates</h2>
            <span>{{ tls?.certificates.length ?? 0 }} certs</span>
          </header>
          <p v-if="!tls?.certificates.length" class="empty-state">没有证书快照数据。</p>
          <div v-else class="table-scroll">
            <table class="data-table">
              <thead>
                <tr>
                  <th>Scope</th>
                  <th>Subject / Issuer</th>
                  <th>Expires</th>
                  <th>Default Listeners</th>
                  <th>OCSP</th>
                </tr>
              </thead>
              <tbody>
                <tr v-for="certificate in tls.certificates" :key="certificate.scope">
                  <td>
                    <strong>{{ certificate.scope }}</strong>
                    <div class="cell-meta">{{ certificate.cert_path }}</div>
                  </td>
                  <td>
                    {{ certificate.subject ?? "-" }}
                    <div class="cell-meta">{{ certificate.issuer ?? "-" }}</div>
                  </td>
                  <td>
                    {{ formatUnixMs(certificate.not_after_unix_ms) }}
                    <div class="cell-meta">{{ certificate.expires_in_days ?? "-" }} days</div>
                  </td>
                  <td>{{ formatList(certificate.selected_as_default_for_listeners) }}</td>
                  <td>
                    {{ formatBoolean(certificate.ocsp_staple_configured, "configured", "off") }}
                    <div class="cell-meta">{{ formatList(certificate.server_names) }}</div>
                  </td>
                </tr>
              </tbody>
            </table>
          </div>
        </article>

        <article class="panel">
          <header class="panel__header">
            <h2>OCSP</h2>
            <span>{{ tls?.ocsp.length ?? 0 }} entries</span>
          </header>
          <p v-if="!tls?.ocsp.length" class="empty-state">没有 OCSP 快照数据。</p>
          <div v-else class="table-scroll">
            <table class="data-table">
              <thead>
                <tr>
                  <th>Scope</th>
                  <th>Responder</th>
                  <th>Cache</th>
                  <th>Last Refresh</th>
                  <th>Failures</th>
                </tr>
              </thead>
              <tbody>
                <tr v-for="ocsp in tls.ocsp" :key="ocsp.scope">
                  <td>
                    <strong>{{ ocsp.scope }}</strong>
                    <div class="cell-meta">{{ ocsp.cert_path }}</div>
                  </td>
                  <td>{{ formatList(ocsp.responder_urls) }}</td>
                  <td>
                    {{ formatBoolean(ocsp.cache_loaded, "loaded", "empty") }}
                    <div class="cell-meta">
                      auto {{ formatBoolean(ocsp.auto_refresh_enabled, "on", "off") }}
                    </div>
                  </td>
                  <td>{{ formatUnixMs(ocsp.last_refresh_unix_ms) }}</td>
                  <td>
                    {{ ocsp.failures_total }}
                    <div class="cell-meta">{{ ocsp.last_error ?? "ok" }}</div>
                  </td>
                </tr>
              </tbody>
            </table>
          </div>
        </article>
      </section>

      <section class="panel-grid">
        <article class="panel">
          <header class="panel__header">
            <h2>SNI Bindings</h2>
            <span>{{ tls?.sni_bindings.length ?? 0 }} bindings</span>
          </header>
          <p v-if="!tls?.sni_bindings.length" class="empty-state">没有 SNI binding 数据。</p>
          <table v-else class="data-table">
            <thead>
              <tr>
                <th>Listener</th>
                <th>Server Name</th>
                <th>Certificate Scopes</th>
                <th>Default</th>
              </tr>
            </thead>
            <tbody>
              <tr
                v-for="binding in tls.sni_bindings"
                :key="`${binding.listener_name}-${binding.server_name}`"
              >
                <td>{{ binding.listener_name }}</td>
                <td>{{ binding.server_name }}</td>
                <td>{{ formatList(binding.certificate_scopes) }}</td>
                <td>{{ formatBoolean(binding.default_selected, "yes", "no") }}</td>
              </tr>
            </tbody>
          </table>
        </article>

        <article class="panel">
          <header class="panel__header">
            <h2>Default Certificate Bindings</h2>
            <span>{{ tls?.default_certificate_bindings.length ?? 0 }} bindings</span>
          </header>
          <p
            v-if="!tls?.default_certificate_bindings.length"
            class="empty-state"
          >
            没有 default certificate binding 数据。
          </p>
          <table v-else class="data-table">
            <thead>
              <tr>
                <th>Listener</th>
                <th>Server Name</th>
                <th>Certificate Scopes</th>
              </tr>
            </thead>
            <tbody>
              <tr
                v-for="binding in tls.default_certificate_bindings"
                :key="`${binding.listener_name}-${binding.server_name}`"
              >
                <td>{{ binding.listener_name }}</td>
                <td>{{ binding.server_name }}</td>
                <td>{{ formatList(binding.certificate_scopes) }}</td>
              </tr>
            </tbody>
          </table>
        </article>
      </section>

      <article class="panel panel--stack">
        <header class="panel__header">
          <h2>Upstream TLS</h2>
          <span>{{ upstreamTls.length }} upstreams</span>
        </header>
        <p v-if="!upstreamTls.length" class="empty-state">没有 upstream TLS 诊断数据。</p>
        <div v-else class="table-scroll">
          <table class="data-table">
            <thead>
              <tr>
                <th>Upstream</th>
                <th>Protocol</th>
                <th>Verify</th>
                <th>TLS Versions</th>
                <th>SNI / Client Identity</th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="entry in upstreamTls" :key="entry.upstream_name">
                <td>{{ entry.upstream_name }}</td>
                <td>{{ entry.protocol }}</td>
                <td>
                  {{ entry.verify_mode }}
                  <div class="cell-meta">depth {{ entry.verify_depth ?? "-" }}</div>
                </td>
                <td>{{ formatList(entry.tls_versions ?? []) }}</td>
                <td>
                  sni {{ formatBoolean(entry.server_name_enabled, "on", "off") }}
                  <div class="cell-meta">
                    override {{ entry.server_name_override ?? "-" }} · client cert
                    {{ formatBoolean(entry.client_identity_configured, "on", "off") }}
                  </div>
                </td>
              </tr>
            </tbody>
          </table>
        </div>
      </article>
    </template>
  </section>
</template>
