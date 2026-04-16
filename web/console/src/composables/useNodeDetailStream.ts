import axios from "axios";
import { onMounted, onUnmounted, ref, watch, type Ref } from "vue";
import { useRouter } from "vue-router";

import {
  buildEventsUrl,
  clearStoredAuthToken,
  extractApiErrorMessage,
  getMe,
  getNodeDetail,
  getStoredAuthToken,
  type AuthenticatedActor,
  type ControlPlaneNodeDetailEvent,
  type NodeDetailResponse,
} from "../api/controlPlane";
import type { StreamState } from "../lib/display";

export function useNodeDetailStream(nodeId: Ref<string>) {
  const router = useRouter();
  const actor = ref<AuthenticatedActor | null>(null);
  const detail = ref<NodeDetailResponse | null>(null);
  const loading = ref(true);
  const error = ref<string | null>(null);
  const streamState = ref<StreamState>("idle");

  let eventSource: EventSource | null = null;

  function redirectToDashboard(): void {
    void router.replace({ name: "dashboard" });
  }

  function closeStream(): void {
    if (eventSource) {
      eventSource.close();
      eventSource = null;
    }
    streamState.value = "idle";
  }

  function setStreamError(message: string): void {
    error.value = message;
    streamState.value = "error";
  }

  function handleAuthFailure(caught: unknown): boolean {
    if (!axios.isAxiosError(caught)) {
      return false;
    }

    const status = caught.response?.status;
    if (status !== 401 && status !== 403) {
      return false;
    }

    clearStoredAuthToken();
    actor.value = null;
    detail.value = null;
    closeStream();
    redirectToDashboard();
    return true;
  }

  function openStream(): void {
    closeStream();

    try {
      eventSource = new EventSource(buildEventsUrl({ nodeId: nodeId.value }));
    } catch (caught) {
      setStreamError(extractApiErrorMessage(caught));
      return;
    }

    streamState.value = "connecting";
    eventSource.addEventListener("open", () => {
      streamState.value = "live";
    });
    eventSource.addEventListener("node.tick", (event) => {
      try {
        const payload = JSON.parse((event as MessageEvent<string>).data) as ControlPlaneNodeDetailEvent;
        detail.value = payload.detail;
        error.value = null;
        streamState.value = "live";
      } catch (caught) {
        setStreamError(caught instanceof Error ? caught.message : "failed to decode node event");
      }
    });
    eventSource.addEventListener("stream.error", (event) => {
      try {
        const payload = JSON.parse((event as MessageEvent<string>).data) as { message?: string };
        setStreamError(payload.message ?? `node stream failed for ${nodeId.value}`);
      } catch {
        setStreamError(`node stream failed for ${nodeId.value}`);
      }
    });
    eventSource.addEventListener("error", () => {
      if (eventSource?.readyState === EventSource.CLOSED) {
        streamState.value = "error";
        return;
      }

      streamState.value = "reconnecting";
    });
  }

  async function load(): Promise<void> {
    loading.value = true;
    error.value = null;
    closeStream();

    if (!getStoredAuthToken()) {
      loading.value = false;
      redirectToDashboard();
      return;
    }

    try {
      const [currentActor, currentDetail] = await Promise.all([getMe(), getNodeDetail(nodeId.value)]);
      actor.value = currentActor;
      detail.value = currentDetail;
      openStream();
    } catch (caught) {
      if (handleAuthFailure(caught)) {
        loading.value = false;
        return;
      }

      error.value = extractApiErrorMessage(caught);
    } finally {
      loading.value = false;
    }
  }

  onMounted(() => {
    void load();
  });

  watch(nodeId, () => {
    void load();
  });

  onUnmounted(() => {
    closeStream();
  });

  return {
    actor,
    detail,
    error,
    loading,
    reload: load,
    streamState,
  };
}
