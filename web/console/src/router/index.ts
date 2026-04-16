import type { RouteRecordRaw } from "vue-router";

import AuditView from "../views/AuditView.vue";
import DashboardView from "../views/DashboardView.vue";
import DeploymentsView from "../views/DeploymentsView.vue";
import NodeDetailView from "../views/NodeDetailView.vue";
import NodeTlsView from "../views/NodeTlsView.vue";
import RevisionsView from "../views/RevisionsView.vue";

export const routes: RouteRecordRaw[] = [
  {
    path: "/",
    name: "dashboard",
    component: DashboardView,
  },
  {
    path: "/audit",
    name: "audit",
    component: AuditView,
  },
  {
    path: "/nodes/:nodeId",
    name: "node-detail",
    component: NodeDetailView,
  },
  {
    path: "/nodes/:nodeId/tls",
    name: "node-tls",
    component: NodeTlsView,
  },
  {
    path: "/revisions",
    name: "revisions",
    component: RevisionsView,
  },
  {
    path: "/deployments",
    name: "deployments",
    component: DeploymentsView,
  },
];
