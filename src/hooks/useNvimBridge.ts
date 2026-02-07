import { useState, useCallback, useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import type {
  NvimContext,
  Diagnostic,
  BufferEdit,
  ConnectionStatus,
  NvimHealth,
  KeymapStatus,
} from "../types/nvim";

const POLL_INTERVAL_MS = 2000;
const HEALTH_FAILURE_THRESHOLD = 2;

export interface NvimBridgeApi {
  status: ConnectionStatus;
  context: NvimContext | null;
  diagnostics: Diagnostic[];
  health: NvimHealth | null;
  keymapStatus: KeymapStatus;
  lastError: string | null;
  connect: (socketPath: string) => Promise<void>;
  disconnect: () => Promise<void>;
  refreshContext: () => Promise<void>;
  probeHealth: () => Promise<NvimHealth | null>;
  reinjectKeymaps: () => Promise<void>;
  applyEdit: (edit: BufferEdit) => Promise<void>;
  applyEdits: (edits: BufferEdit[]) => Promise<void>;
  execCommand: (command: string) => Promise<string>;
}

export function useNvimBridge(terminalId: string | null): NvimBridgeApi {
  const [status, setStatus] = useState<ConnectionStatus>("Disconnected");
  const [context, setContext] = useState<NvimContext | null>(null);
  const [diagnostics, setDiagnostics] = useState<Diagnostic[]>([]);
  const [health, setHealth] = useState<NvimHealth | null>(null);
  const [keymapStatus, setKeymapStatus] = useState<KeymapStatus>("unknown");
  const [lastError, setLastError] = useState<string | null>(null);

  const pollRef = useRef<number | null>(null);
  const failedRefreshesRef = useRef(0);

  const applyHealthSnapshot = useCallback((snapshot: NvimHealth) => {
    setHealth(snapshot);

    if (!snapshot.connected) {
      setStatus("Disconnected");
      setKeymapStatus("unknown");
      setContext(null);
      setDiagnostics([]);
      setLastError(snapshot.lastError ?? null);
      return;
    }

    setStatus("Connected");
    setKeymapStatus(snapshot.keymapsInjected ? "present" : "missing");
    setLastError(null);
  }, []);

  const probeHealth = useCallback(async (): Promise<NvimHealth | null> => {
    if (!terminalId) return null;

    try {
      const snapshot = await invoke<NvimHealth>("nvim_probe_health", {
        terminalId,
      });
      applyHealthSnapshot(snapshot);
      return snapshot;
    } catch (e) {
      console.error("nvim_probe_health error:", e);
      setStatus("Error");
      setHealth(null);
      setKeymapStatus("error");
      setLastError(String(e));
      return null;
    }
  }, [terminalId, applyHealthSnapshot]);

  const fetchContextAndDiagnostics = useCallback(async () => {
    if (!terminalId) return;

    const ctx = await invoke<NvimContext>("nvim_get_context", {
      terminalId,
    });
    const diags = await invoke<Diagnostic[]>("nvim_get_diagnostics", {
      terminalId,
    });

    setContext(ctx);
    setDiagnostics(diags);
  }, [terminalId]);

  const connect = useCallback(
    async (socketPath: string) => {
      if (!terminalId) return;
      try {
        await invoke("nvim_connect", {
          terminalId,
          socketPath,
        });

        failedRefreshesRef.current = 0;
        const snapshot = await probeHealth();
        if (!snapshot?.connected) {
          const err = snapshot?.lastError ?? "Neovim is not ready yet";
          throw new Error(err);
        }
        await fetchContextAndDiagnostics();
      } catch (e) {
        console.error("nvim_connect error:", e);
        setStatus("Error");
        setHealth(null);
        setKeymapStatus("error");
        setLastError(String(e));
        throw e;
      }
    },
    [terminalId, probeHealth, fetchContextAndDiagnostics]
  );

  const disconnect = useCallback(async () => {
    if (!terminalId) return;
    try {
      await invoke("nvim_disconnect", { terminalId });
      failedRefreshesRef.current = 0;
      setStatus("Disconnected");
      setHealth(null);
      setKeymapStatus("unknown");
      setLastError(null);
      setContext(null);
      setDiagnostics([]);
    } catch (e) {
      console.error("nvim_disconnect error:", e);
    }
  }, [terminalId]);

  const refreshContext = useCallback(async () => {
    if (!terminalId || status !== "Connected") return;

    const snapshot = await probeHealth();
    if (!snapshot?.connected) {
      failedRefreshesRef.current = 0;
      return;
    }

    try {
      await fetchContextAndDiagnostics();
      failedRefreshesRef.current = 0;
    } catch (e) {
      console.error("nvim context refresh error:", e);
      setLastError(String(e));

      failedRefreshesRef.current += 1;
      if (failedRefreshesRef.current >= HEALTH_FAILURE_THRESHOLD) {
        failedRefreshesRef.current = 0;
        await probeHealth();
      }
    }
  }, [terminalId, status, probeHealth, fetchContextAndDiagnostics]);

  const reinjectKeymaps = useCallback(async () => {
    if (!terminalId) return;

    try {
      await invoke("nvim_reinject_keymaps", { terminalId });
      const snapshot = await probeHealth();
      if (snapshot?.connected) {
        setKeymapStatus(snapshot.keymapsInjected ? "present" : "missing");
      }
    } catch (e) {
      console.error("nvim_reinject_keymaps error:", e);
      setKeymapStatus("error");
      setLastError(String(e));
      throw e;
    }
  }, [terminalId, probeHealth]);

  const applyEdit = useCallback(
    async (edit: BufferEdit) => {
      if (!terminalId) return;
      await invoke("nvim_apply_edit", { terminalId, edit });
    },
    [terminalId]
  );

  const applyEdits = useCallback(
    async (edits: BufferEdit[]) => {
      if (!terminalId) return;
      await invoke("nvim_apply_edits", { terminalId, edits });
    },
    [terminalId]
  );

  const execCommand = useCallback(
    async (command: string): Promise<string> => {
      if (!terminalId) return "";
      return invoke<string>("nvim_exec_command", { terminalId, command });
    },
    [terminalId]
  );

  useEffect(() => {
    failedRefreshesRef.current = 0;

    if (!terminalId) {
      setStatus("Disconnected");
      setHealth(null);
      setKeymapStatus("unknown");
      setLastError(null);
      setContext(null);
      setDiagnostics([]);
      return;
    }

    void (async () => {
      const snapshot = await probeHealth();
      if (snapshot?.connected) {
        try {
          await fetchContextAndDiagnostics();
          failedRefreshesRef.current = 0;
        } catch (e) {
          console.error("nvim initial refresh error:", e);
          setLastError(String(e));
        }
      }
    })();
  }, [terminalId, probeHealth, fetchContextAndDiagnostics]);

  // Poll health + context every 2s when connected
  useEffect(() => {
    if (status !== "Connected") {
      if (pollRef.current) {
        clearInterval(pollRef.current);
        pollRef.current = null;
      }
      return;
    }

    void refreshContext();
    pollRef.current = window.setInterval(() => {
      void refreshContext();
    }, POLL_INTERVAL_MS);

    return () => {
      if (pollRef.current) {
        clearInterval(pollRef.current);
        pollRef.current = null;
      }
    };
  }, [status, refreshContext]);

  return {
    status,
    context,
    diagnostics,
    health,
    keymapStatus,
    lastError,
    connect,
    disconnect,
    refreshContext,
    probeHealth,
    reinjectKeymaps,
    applyEdit,
    applyEdits,
    execCommand,
  };
}
