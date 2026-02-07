import { useRef, useEffect, useState, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { AiChatController } from "../../hooks/useAiChat";
import type { NvimStartLaunchResult } from "../../types/nvim";
import { ContextBadge } from "./ContextBadge";
import { ChatMessage } from "./ChatMessage";
import { ChatInput } from "./ChatInput";
import "./AiChat.css";

const NVIM_CONNECT_POLL_MS = 250;
const NVIM_CONNECT_TIMEOUT_MS = 8000;

type Props = {
  terminalId: string | null;
  terminalWorkingDirectory?: string | null;
  ai: AiChatController;
};

export function AiChat({ terminalId, terminalWorkingDirectory, ai }: Props) {
  const {
    messages,
    isStreaming,
    autoApply,
    setAutoApply,
    appendSystemMessage,
    sendMessage,
    applyProposedEdits,
    rejectProposedEdits,
    nvim,
    acp,
    traceEvents,
    clearTraceEvents,
  } = ai;

  const messagesEndRef = useRef<HTMLDivElement>(null);
  const startAttemptRef = useRef(0);
  const autoStartInFlightRef = useRef(false);
  const autoStartedConnectKeyRef = useRef<string | null>(null);
  const [connectInput, setConnectInput] = useState("");
  const [showManualConnect, setShowManualConnect] = useState(false);
  const [isStartingNvim, setIsStartingNvim] = useState(false);
  const [isStartingWithoutTmux, setIsStartingWithoutTmux] = useState(false);
  const [tmuxFallbackPrompt, setTmuxFallbackPrompt] = useState<string | null>(null);
  const [isReinjectingKeymaps, setIsReinjectingKeymaps] = useState(false);
  const [agentError, setAgentError] = useState<string | null>(null);
  const [keymapError, setKeymapError] = useState<string | null>(null);
  const [permissionError, setPermissionError] = useState<string | null>(null);
  const [isRespondingPermission, setIsRespondingPermission] = useState(false);

  useEffect(() => {
    if (nvim.keymapStatus === "present") {
      setKeymapError(null);
    }
  }, [nvim.keymapStatus]);

  useEffect(() => {
    // Cancel any in-flight start attempt when switching terminals.
    startAttemptRef.current += 1;
    setIsStartingNvim(false);
    setIsStartingWithoutTmux(false);
    setTmuxFallbackPrompt(null);
  }, [terminalId]);

  const handleConnect = useCallback(async () => {
    if (!connectInput.trim()) return;
    try {
      await nvim.connect(connectInput.trim());
      setShowManualConnect(false);
      appendSystemMessage("Connected to existing Neovim socket.");
    } catch (e) {
      appendSystemMessage(`Failed to connect Neovim socket: ${String(e)}`, "status-note");
    }
  }, [connectInput, nvim, appendSystemMessage]);

  const waitForNvimConnection = useCallback(
    async (attemptId: number, socketPath: string, successMessage: string) => {
      const deadline = Date.now() + NVIM_CONNECT_TIMEOUT_MS;
      let lastError: unknown = null;

      while (Date.now() < deadline) {
        if (startAttemptRef.current !== attemptId) {
          return;
        }

        try {
          await nvim.connect(socketPath);
          appendSystemMessage(successMessage);
          return;
        } catch (e) {
          lastError = e;
          await new Promise((resolve) => setTimeout(resolve, NVIM_CONNECT_POLL_MS));
        }
      }

      const reason = lastError ? String(lastError) : "Timed out waiting for Neovim to be ready";
      throw new Error(reason);
    },
    [nvim, appendSystemMessage]
  );

  const startNvim = useCallback(
    async (allowFallback: boolean) => {
      if (!terminalId || isStartingNvim) return;
      const attemptId = startAttemptRef.current + 1;
      startAttemptRef.current = attemptId;
      setIsStartingNvim(true);
      try {
        const socketPath = await invoke<string>("get_socket_path", {
          terminalId,
        });
        const launch = await invoke<NvimStartLaunchResult>("nvim_start_in_tmux", {
          terminalId,
          socketPath,
          allowFallback,
          cwd: terminalWorkingDirectory ?? null,
        });

        if (startAttemptRef.current !== attemptId) {
          return;
        }

        if (launch.launchMode === "tmuxUnavailable") {
          setTmuxFallbackPrompt(launch.message);
          appendSystemMessage(launch.message, "status-note");
          return;
        }

        setTmuxFallbackPrompt(null);
        await waitForNvimConnection(
          attemptId,
          socketPath,
          launch.launchMode === "tmux"
            ? "Started Neovim in tmux and connected to socket."
            : "Started Neovim and connected to socket."
        );
      } catch (e) {
        if (startAttemptRef.current !== attemptId) {
          return;
        }
        console.error("Failed to start neovim:", e);
        appendSystemMessage(`Failed to start Neovim: ${String(e)}`, "status-note");
      } finally {
        if (startAttemptRef.current === attemptId) {
          setIsStartingNvim(false);
        }
      }
    },
    [
      terminalId,
      terminalWorkingDirectory,
      isStartingNvim,
      appendSystemMessage,
      waitForNvimConnection,
    ]
  );

  const handleStartNvim = useCallback(() => {
    void startNvim(false);
  }, [startNvim]);

  const handleStartWithoutTmux = useCallback(async () => {
    if (!terminalId || isStartingNvim || isStartingWithoutTmux) return;
    setIsStartingWithoutTmux(true);
    try {
      await invoke("tmux_enable_for_terminal", { terminalId, enabled: false });
      appendSystemMessage("Continuing without tmux for this terminal.");
      await startNvim(true);
    } finally {
      setIsStartingWithoutTmux(false);
    }
  }, [terminalId, isStartingNvim, isStartingWithoutTmux, appendSystemMessage, startNvim]);

  const handleRetryTmux = useCallback(async () => {
    if (!terminalId || isStartingNvim) return;
    await invoke("tmux_enable_for_terminal", { terminalId, enabled: true });
    setTmuxFallbackPrompt(null);
    void startNvim(false);
  }, [terminalId, isStartingNvim, startNvim]);

  const ensureAgentSession = useCallback(async (source: "manual" | "auto") => {
    if (!terminalId) return;
    setAgentError(null);
    try {
      let startedAgent = false;
      if (acp.status !== "Running") {
        if (acp.status === "Starting") return;
        await acp.startAgent();
        startedAgent = true;
      }

      // Also create a session with the terminal's working directory
      if (nvim.context?.filePath) {
        const dir = nvim.context.filePath.replace(/\/[^/]+$/, "") || "/";
        await acp.createSession(dir, terminalId);
      } else {
        await acp.createSession("/", terminalId);
      }

      if (source === "manual") {
        if (startedAgent) {
          appendSystemMessage("Started codex-acp agent and created session.");
        } else {
          appendSystemMessage("Created agent session for current Neovim terminal.");
        }
      } else if (startedAgent) {
        appendSystemMessage("Auto-started codex-acp agent and created session.");
      }
    } catch (e) {
      setAgentError(String(e));
      appendSystemMessage(`Failed to ensure agent session: ${String(e)}`, "status-note");
    }
  }, [acp, nvim.context, terminalId, appendSystemMessage]);

  const handleStartAgent = useCallback(async () => {
    await ensureAgentSession("manual");
  }, [ensureAgentSession]);

  const handleReinjectKeymaps = useCallback(async () => {
    setKeymapError(null);
    setIsReinjectingKeymaps(true);
    try {
      await nvim.reinjectKeymaps();
      appendSystemMessage("Re-injected neoai keymaps.");
    } catch (e) {
      setKeymapError(String(e));
      appendSystemMessage(`Failed to re-inject keymaps: ${String(e)}`, "status-note");
    } finally {
      setIsReinjectingKeymaps(false);
    }
  }, [nvim, appendSystemMessage]);

  const isConnected = nvim.status === "Connected";
  const isAgentRunning = acp.status === "Running";
  const isAgentStarting = acp.status === "Starting";
  const hasKeymapIssue =
    isConnected && (nvim.keymapStatus === "missing" || nvim.keymapStatus === "error");
  const isInstallInProgress =
    (acp.installState !== null && acp.installState.phase !== "error") ||
    (isAgentStarting && !isAgentRunning);
  const installMessage = acp.installState?.message ?? "Starting AI agent...";
  const permissionRequest =
    acp.currentPermission &&
    (!acp.currentPermission.terminalId || acp.currentPermission.terminalId === terminalId)
      ? acp.currentPermission
      : null;

  const currentAction = permissionRequest
    ? { type: "permission" as const }
    : tmuxFallbackPrompt
      ? { type: "tmuxFallback" as const }
    : !isConnected
      ? { type: "connect" as const }
      : hasKeymapIssue
        ? { type: "keymaps" as const }
        : isConnected && !isAgentRunning
          ? { type: "agent" as const }
          : null;

  const actionKey =
    currentAction?.type === "permission" && permissionRequest
      ? `permission:${permissionRequest.requestId}`
      : currentAction?.type ?? "none";
  const nvimConnectKey =
    isConnected && terminalId
      ? `${terminalId}:${nvim.health?.socketPath ?? "connected"}`
      : null;

  // Auto-scroll to bottom on new messages, action card transitions, and streaming state.
  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [messages, actionKey, isStreaming]);

  // Auto-start agent + create session once for each Neovim connection key.
  useEffect(() => {
    if (!nvimConnectKey) {
      autoStartedConnectKeyRef.current = null;
      return;
    }

    if (autoStartedConnectKeyRef.current === nvimConnectKey) {
      return;
    }
    if (autoStartInFlightRef.current || acp.status === "Starting") {
      return;
    }

    autoStartInFlightRef.current = true;
    void (async () => {
      try {
        await ensureAgentSession("auto");
      } finally {
        autoStartedConnectKeyRef.current = nvimConnectKey;
        autoStartInFlightRef.current = false;
      }
    })();
  }, [nvimConnectKey, acp.status, ensureAgentSession]);

  const handlePermissionResponse = useCallback(
    async (optionId?: string) => {
      if (!permissionRequest || isRespondingPermission) return;
      setPermissionError(null);
      setIsRespondingPermission(true);
      try {
        const option = optionId
          ? permissionRequest.options.find((candidate) => candidate.optionId === optionId)
          : undefined;
        await acp.respondPermission(permissionRequest.requestId, optionId);
        if (!optionId) {
          appendSystemMessage("Cancelled permission request.");
        } else if (option) {
          const kind = option.kind.toLowerCase();
          if (kind.includes("allow")) {
            appendSystemMessage(`Accepted: ${option.name}.`);
          } else if (kind.includes("reject")) {
            appendSystemMessage(`Rejected: ${option.name}.`);
          } else {
            appendSystemMessage(`Permission response: ${option.name}.`);
          }
        } else {
          appendSystemMessage("Submitted permission response.");
        }
      } catch (e) {
        setPermissionError(String(e));
        appendSystemMessage(`Failed to submit permission response: ${String(e)}`, "status-note");
      } finally {
        setIsRespondingPermission(false);
      }
    },
    [acp, permissionRequest, isRespondingPermission, appendSystemMessage]
  );

  return (
    <div className="ai-chat">
      <div className="ai-chat__header">
        <div className="ai-chat__title-row">
          <div className="ai-chat__title">AI Chat</div>
          {isConnected && isAgentRunning && (
            <label className="ai-chat__auto-apply">
              <input
                type="checkbox"
                checked={autoApply}
                onChange={(e) => setAutoApply(e.target.checked)}
              />
              <span>Auto-apply</span>
            </label>
          )}
        </div>
        <ContextBadge
          nvimStatus={nvim.status}
          agentStatus={acp.status}
          context={nvim.context}
          diagnostics={nvim.diagnostics}
        />
        {traceEvents.length > 0 && (
          <details className="ai-chat__trace">
            <summary>Debug trace ({traceEvents.length})</summary>
            <div className="ai-chat__trace-actions">
              <button
                type="button"
                className="ai-chat__trace-clear"
                onClick={clearTraceEvents}
              >
                Clear
              </button>
            </div>
            <ul className="ai-chat__trace-list">
              {traceEvents.slice(-12).map((event) => (
                <li key={event.id}>
                  <span>{new Date(event.timestamp).toLocaleTimeString()}</span>
                  <span>{event.stage}</span>
                  {event.detail ? <span>{event.detail}</span> : null}
                </li>
              ))}
            </ul>
          </details>
        )}
      </div>

      <div className="ai-chat__messages">
        {messages.map((msg) => (
          <ChatMessage
            key={msg.id}
            message={msg}
            onApplyEdits={applyProposedEdits}
            onRejectEdits={rejectProposedEdits}
          />
        ))}

        {currentAction?.type === "permission" && permissionRequest && (
          <div className="ai-chat__connect-prompt ai-chat__connect-prompt--warning">
            <p>Agent needs approval to continue a tool call.</p>
            {(permissionRequest.title || permissionRequest.kind) && (
              <p>
                {permissionRequest.title ?? "Tool call"}
                {permissionRequest.kind ? ` (${permissionRequest.kind})` : ""}
              </p>
            )}
            <div className="ai-chat__permission-options">
              {permissionRequest.options.map((option) => (
                <button
                  key={option.optionId}
                  type="button"
                  className="ai-chat__connect-btn"
                  onClick={() => {
                    void handlePermissionResponse(option.optionId);
                  }}
                  disabled={isRespondingPermission}
                >
                  {option.name}
                </button>
              ))}
              <button
                type="button"
                className="ai-chat__connect-link"
                onClick={() => {
                  void handlePermissionResponse();
                }}
                disabled={isRespondingPermission}
              >
                Cancel
              </button>
            </div>
            {permissionError && <p className="ai-chat__agent-error">{permissionError}</p>}
          </div>
        )}

        {currentAction?.type === "connect" && (
          <div className="ai-chat__connect-prompt">
            <p>
              {nvim.status === "Error"
                ? "Neovim connection failed. Start it again or reconnect manually."
                : "Connect to neovim to enable AI assistance."}
            </p>
            <button
              type="button"
              className="ai-chat__connect-btn ai-chat__connect-btn--primary"
              onClick={handleStartNvim}
              disabled={!terminalId || isStartingNvim}
            >
              {isStartingNvim ? "Starting Neovim..." : "Start Neovim"}
            </button>
            {!showManualConnect ? (
              <button
                type="button"
                className="ai-chat__connect-link"
                onClick={() => setShowManualConnect(true)}
              >
                Connect to existing...
              </button>
            ) : (
              <div className="ai-chat__connect-row">
                <input
                  className="ai-chat__connect-input"
                  type="text"
                  value={connectInput}
                  onChange={(e) => setConnectInput(e.target.value)}
                  onKeyDown={(e) => e.key === "Enter" && handleConnect()}
                  placeholder="/tmp/libg-nvim.sock"
                />
                <button
                  type="button"
                  className="ai-chat__connect-btn"
                  onClick={handleConnect}
                >
                  Connect
                </button>
              </div>
            )}
            {nvim.lastError && (
              <p className="ai-chat__agent-error">{nvim.lastError}</p>
            )}
          </div>
        )}

        {currentAction?.type === "tmuxFallback" && (
          <div className="ai-chat__connect-prompt ai-chat__connect-prompt--warning">
            <p>{tmuxFallbackPrompt}</p>
            <button
              type="button"
              className="ai-chat__connect-btn ai-chat__connect-btn--primary"
              onClick={handleStartWithoutTmux}
              disabled={!terminalId || isStartingNvim || isStartingWithoutTmux}
            >
              {isStartingNvim || isStartingWithoutTmux
                ? "Starting without tmux..."
                : "Continue Without tmux"}
            </button>
            <button
              type="button"
              className="ai-chat__connect-link"
              onClick={() => {
                void handleRetryTmux();
              }}
              disabled={!terminalId || isStartingNvim}
            >
              Retry with tmux
            </button>
          </div>
        )}

        {currentAction?.type === "keymaps" && (
          <div className="ai-chat__connect-prompt ai-chat__connect-prompt--warning">
            <p>Neovim is connected, but neoai keymaps are not loaded yet.</p>
            <button
              type="button"
              className="ai-chat__connect-btn"
              onClick={handleReinjectKeymaps}
              disabled={isReinjectingKeymaps}
            >
              {isReinjectingKeymaps ? "Re-injecting Keymaps..." : "Re-inject Keymaps"}
            </button>
            {(keymapError || nvim.lastError) && (
              <p className="ai-chat__agent-error">{keymapError ?? nvim.lastError}</p>
            )}
          </div>
        )}

        {currentAction?.type === "agent" && (
          <div className="ai-chat__connect-prompt">
            <p>Start an AI agent to begin chatting.</p>
            {isInstallInProgress ? (
              <>
                <button type="button" className="ai-chat__connect-btn" disabled>
                  Preparing Agent...
                </button>
                <div className="ai-chat__install-status">
                  <span className="ai-chat__install-spinner" />
                  <span>{installMessage}</span>
                </div>
              </>
            ) : (
              <button
                type="button"
                className="ai-chat__connect-btn"
                onClick={handleStartAgent}
              >
                Start Agent (codex-acp)
              </button>
            )}
            {agentError && <p className="ai-chat__agent-error">{agentError}</p>}
          </div>
        )}

        {isStreaming && (
          <div className="ai-chat__streaming-indicator">
            <span className="ai-chat__streaming-dot" />
            Generating...
          </div>
        )}

        <div ref={messagesEndRef} />
      </div>

      <div className="ai-chat__footer">
        <ChatInput
          onSend={sendMessage}
          disabled={!isConnected || !isAgentRunning || isStreaming}
          placeholder={
            !isConnected
              ? "Connect to neovim first..."
              : !isAgentRunning
                ? "Start an agent first..."
                : isStreaming
                  ? "Waiting for response..."
                  : "Ask about your code... (Cmd+Enter to send)"
          }
        />
      </div>
    </div>
  );
}
