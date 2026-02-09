import { useState, useCallback, useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  AcpEvent,
  AcpInstallStatus,
  AcpPromptBlock,
  AcpPermissionRequest,
  AgentStatus,
} from "../types/acp";

const DEFAULT_AGENT_PATH = "codex-acp";

export function useAcpAgent() {
  const [status, setStatus] = useState<AgentStatus>("Stopped");
  const [installState, setInstallState] = useState<AcpInstallStatus | null>(null);
  const [sessionId, setSessionId] = useState<string | null>(null);
  const [permissionQueue, setPermissionQueue] = useState<AcpPermissionRequest[]>([]);
  const listenersRef = useRef<UnlistenFn[]>([]);
  const eventCallbackRef = useRef<((event: AcpEvent) => void) | null>(null);

  // Set up event listener for streaming ACP events
  useEffect(() => {
    let cancelled = false;

    const setup = async () => {
      const unlisten = await listen<AcpEvent>("acp-event", (event) => {
        if (!cancelled && eventCallbackRef.current) {
          eventCallbackRef.current(event.payload);
        }
      });
      const unlistenInstall = await listen<AcpInstallStatus>("acp-install-status", (event) => {
        if (cancelled) return;
        const next = event.payload;
        if (next.phase === "done") {
          setInstallState(null);
          return;
        }
        setInstallState(next);
      });
      const unlistenPermission = await listen<AcpPermissionRequest>(
        "acp-permission-request",
        (event) => {
          if (cancelled) return;
          setPermissionQueue((prev) => [...prev, event.payload]);
        }
      );
      if (!cancelled) {
        listenersRef.current.push(unlisten);
        listenersRef.current.push(unlistenInstall);
        listenersRef.current.push(unlistenPermission);
      } else {
        unlisten();
        unlistenInstall();
        unlistenPermission();
      }
    };

    setup();

    return () => {
      cancelled = true;
      for (const unlisten of listenersRef.current) {
        unlisten();
      }
      listenersRef.current = [];
    };
  }, []);

  const onEvent = useCallback((callback: (event: AcpEvent) => void) => {
    eventCallbackRef.current = callback;
  }, []);

  const startAgent = useCallback(
    async (agentPath: string = DEFAULT_AGENT_PATH) => {
      try {
        setStatus("Starting");
        setPermissionQueue([]);
        setInstallState({
          phase: "starting",
          message: "Starting AI agent...",
        });
        await invoke("acp_start_agent", { agentPath });
        setStatus("Running");
        setInstallState(null);
      } catch (e) {
        setStatus({ Error: String(e) });
        setInstallState((prev) => (prev?.phase === "error" ? prev : null));
        throw e;
      }
    },
    []
  );

  const stopAgent = useCallback(async () => {
    try {
      await invoke("acp_stop_agent");
      setStatus("Stopped");
      setInstallState(null);
      setSessionId(null);
      setPermissionQueue([]);
    } catch (e) {
      console.error("acp_stop_agent error:", e);
    }
  }, []);

  const createSession = useCallback(async (workingDir: string, terminalId: string) => {
    const sid = await invoke<string>("acp_create_session", {
      workingDir,
      terminalId,
    });
    setSessionId(sid);
    return sid;
  }, []);

  const sendPrompt = useCallback(
    async (blocks: AcpPromptBlock[]) => {
      if (!sessionId) throw new Error("No active session");
      return invoke<string>("acp_send_prompt", {
        sessionId,
        blocks,
      });
    },
    [sessionId]
  );

  const refreshStatus = useCallback(async () => {
    const s = await invoke<AgentStatus>("acp_agent_status");
    setStatus(s);
    if (s === "Running") {
      setInstallState(null);
    }
    return s;
  }, []);

  const respondPermission = useCallback(
    async (requestId: string, optionId?: string) => {
      await invoke("acp_respond_permission_request", {
        requestId,
        optionId: optionId ?? null,
      });
      setPermissionQueue((prev) => prev.filter((req) => req.requestId !== requestId));
    },
    []
  );

  return {
    status,
    installState,
    sessionId,
    permissionQueue,
    currentPermission: permissionQueue[0] ?? null,
    startAgent,
    stopAgent,
    createSession,
    sendPrompt,
    respondPermission,
    onEvent,
    refreshStatus,
  };
}
