import { useState, useEffect, useCallback } from "react";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { Ghostty } from "./components/Ghostty";
import { ProjectExplorer } from "./components/ProjectExplorer";
import { AiChat } from "./components/AiChat";
import { useNvimBridge } from "./hooks/useNvimBridge";
import { useAiChat } from "./hooks/useAiChat";
import { useTerminalManager } from "./hooks/useTerminalManager";
import { useLocalStorage } from "./hooks/useLocalStorage";
import type { NvimActionEvent, NvimBridgeDebugEvent } from "./types/nvim";
import "./App.css";

type SidePanel = "explorer" | "ai";

function App() {
  const [sidebarWidth, setSidebarWidth] = useLocalStorage<number>('libg:sidebarWidth', 260);
  const [isResizing, setIsResizing] = useState(false);
  const [activePanel, setActivePanel] = useLocalStorage<SidePanel>('libg:activePanel', 'explorer');
  const { activeTerminalId, terminals, switchToFolder, destroyTerminal } = useTerminalManager();
  const [terminalFocused, setTerminalFocused] = useState(false);
  const nvim = useNvimBridge(activeTerminalId);
  const aiChat = useAiChat(activeTerminalId, nvim);

  const blurActiveTerminal = useCallback(() => {
    if (!activeTerminalId) return;
    invoke("ghostty_focus", { id: activeTerminalId, focused: false }).catch(console.error);
  }, [activeTerminalId]);

  const handleAppMouseDownCapture = useCallback((event: React.MouseEvent<HTMLElement>) => {
    const target = event.target as HTMLElement | null;
    if (!target) return;
    if (target.closest(".terminal-panel")) return;
    blurActiveTerminal();
  }, [blurActiveTerminal]);

  // Track native Ghostty focus via becomeFirstResponder / resignFirstResponder
  useEffect(() => {
    setTerminalFocused(false);
    const unlisten = listen<{ terminalId: string; focused: boolean }>(
      "ghostty-focus",
      (event) => {
        if (event.payload.terminalId === activeTerminalId) {
          setTerminalFocused(event.payload.focused);
        }
      }
    );
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [activeTerminalId]);

  // Restore terminal for persisted active folder on mount
  useEffect(() => {
    try {
      const raw = localStorage.getItem('libg:activeFolderId');
      const rawProjects = localStorage.getItem('libg:projects');
      if (!raw || !rawProjects) return;
      const folderId = JSON.parse(raw);
      const projects = JSON.parse(rawProjects);
      for (const project of projects) {
        const folder = project.folders.find((f: { id: string }) => f.id === folderId);
        if (folder) {
          switchToFolder(folder);
          break;
        }
      }
    } catch {
      // ignore parse errors
    }
  }, []);

  // Auto-switch to AI panel when a nvim-action is received
  useEffect(() => {
    const unlisten = listen<NvimActionEvent>("nvim-action", (event) => {
      console.info(
        `[neoai][trace] app.nvim-action: ${event.payload.action.action} @ ${event.payload.terminalId}`
      );
      setActivePanel("ai");
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [setActivePanel]);

  // Surface backend bridge debug events in the webview console.
  useEffect(() => {
    const unlisten = listen<NvimBridgeDebugEvent>("nvim-bridge-debug", (event) => {
      const detail = event.payload.detail ? `: ${event.payload.detail}` : "";
      console.info(
        `[neoai][bridge] ${event.payload.terminalId} ${event.payload.stage}${detail}`
      );
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  const handleResizeStart = (e: React.MouseEvent) => {
    setIsResizing(true);
    e.preventDefault();
  };

  useEffect(() => {
    if (!isResizing) return;

    const handleMouseMove = (e: MouseEvent) => {
      // Account for content padding (16px on left)
      const newWidth = Math.min(Math.max(e.clientX - 16, 200), 500);
      setSidebarWidth(newWidth);
    };

    const handleMouseUp = () => setIsResizing(false);

    document.addEventListener('mousemove', handleMouseMove);
    document.addEventListener('mouseup', handleMouseUp);
    return () => {
      document.removeEventListener('mousemove', handleMouseMove);
      document.removeEventListener('mouseup', handleMouseUp);
    };
  }, [isResizing]);

  return (
    <main
      className="app-shell"
      onMouseDownCapture={handleAppMouseDownCapture}
    >
      <header className="toolbar">
        <div className="toolbar-title">NeoAI</div>
        <div className="toolbar-actions">
          <button
            type="button"
            className={activePanel === "ai" ? "toolbar-btn--active" : ""}
            onClick={() =>
              setActivePanel((p) => (p === "ai" ? "explorer" : "ai"))
            }
          >
            AI
          </button>
          <button type="button">Split</button>
          <button type="button">Settings</button>
        </div>
      </header>

      <section
        className={`content ${isResizing ? 'content--resizing' : ''}`}
        style={{ gridTemplateColumns: `${sidebarWidth}px 6px 1fr` }}
      >
        <div className="side-panel">
          {activePanel === "explorer" ? (
            <ProjectExplorer
              onSelectFolder={switchToFolder}
              onRemoveProject={(folderIds) => {
                folderIds.forEach((id) => destroyTerminal(`terminal-${id}`));
              }}
              onRemoveFolder={(folderId) => {
                destroyTerminal(`terminal-${folderId}`);
              }}
            />
          ) : (
            <AiChat
              terminalId={activeTerminalId}
              terminalWorkingDirectory={
                activeTerminalId ? terminals.get(activeTerminalId)?.path ?? null : null
              }
              ai={aiChat}
            />
          )}
        </div>
        <div
          className={`resize-handle ${isResizing ? 'resize-handle--active' : ''}`}
          onMouseDown={handleResizeStart}
        />
        <div className={`terminal-panel${terminalFocused ? ' terminal-panel--focused' : ''}`}>
          {Array.from(terminals.entries()).map(([termId, entry]) => (
            <Ghostty
              key={termId}
              id={termId}
              className="ghostty-host"
              visible={termId === activeTerminalId}
              options={{ workingDirectory: entry.path }}
            />
          ))}
          {terminals.size === 0 && (
            <div className="empty-terminal-state">
              <p>Select a folder to open a terminal</p>
            </div>
          )}
        </div>
      </section>
    </main>
  );
}

export default App;
