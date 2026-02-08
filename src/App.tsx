import { useState, useEffect, useCallback, useRef } from "react";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { Ghostty } from "./components/Ghostty";
import { ProjectExplorer } from "./components/ProjectExplorer";
import { ProjectPicker } from "./components/ProjectPicker/ProjectPicker";
import { AiChat } from "./components/AiChat";
import { useNvimBridge } from "./hooks/useNvimBridge";
import { useAiChat } from "./hooks/useAiChat";
import { useTerminalManager } from "./hooks/useTerminalManager";
import { useProjectExplorer } from "./hooks/useProjectExplorer";
import { useSessionManager } from "./hooks/useSessionManager";
import { migrateLocalStorageOnce } from "./utils/migrateLocalStorage";
import type { NvimActionEvent, NvimBridgeDebugEvent } from "./types/nvim";
import type { Project } from "./types/project-explorer";
import "./App.css";

type SidePanel = "explorer" | "ai";

interface BootstrapState {
  projects: Project[];
  activeFolderId: string | null;
  settings: Record<string, string>;
}

function findFolder(projects: Project[], folderId: string) {
  for (const project of projects) {
    const folder = project.folders.find((candidate) => candidate.id === folderId);
    if (folder) return folder;
  }
  return null;
}

function App() {
  const [sidebarWidth, setSidebarWidth] = useState(260);
  const [isResizing, setIsResizing] = useState(false);
  const [activePanel, setActivePanel] = useState<SidePanel>("explorer");
  const [isBootstrapping, setIsBootstrapping] = useState(true);
  const settingsLoadedRef = useRef(false);

  const explorer = useProjectExplorer();
  const { activeTerminalId, terminals, switchToFolder, destroyTerminal } = useTerminalManager();
  const { addProject, applyBootstrap, clearActiveFolder, markFolderSession } = explorer;
  const sessionManager = useSessionManager({
    activeTerminalId,
    destroyTerminal,
    onSessionClosed: (folderId, screenshotPath) => {
      markFolderSession(folderId, screenshotPath);
      clearActiveFolder();
    },
  });
  const { openPicker, closePicker, closeSession } = sessionManager;

  const [terminalFocused, setTerminalFocused] = useState(false);
  const nvim = useNvimBridge(activeTerminalId);
  const aiChat = useAiChat(activeTerminalId, nvim);

  const blurActiveTerminal = useCallback(() => {
    if (!activeTerminalId) return;
    invoke("ghostty_focus", { id: activeTerminalId, focused: false }).catch(console.error);
  }, [activeTerminalId]);

  const handleAddProject = useCallback(async () => {
    const selected = await open({ directory: true, multiple: false, title: "Select Project Folder" });
    if (!selected) return;
    const name = selected.split("/").pop() || selected;
    await addProject(selected, name);
  }, [addProject]);

  const handleAppMouseDownCapture = useCallback(
    (event: React.MouseEvent<HTMLElement>) => {
      const target = event.target as HTMLElement | null;
      if (!target) return;
      if (target.closest(".terminal-panel")) return;
      blurActiveTerminal();
    },
    [blurActiveTerminal]
  );

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        await migrateLocalStorageOnce();
        const bootstrap = await invoke<BootstrapState>("db_bootstrap_state");
        if (cancelled) return;

        applyBootstrap(bootstrap.projects, bootstrap.activeFolderId);

        const width = Number(bootstrap.settings.sidebar_width);
        if (Number.isFinite(width)) {
          setSidebarWidth(Math.max(200, Math.min(500, width)));
        }

        const panel = bootstrap.settings.active_panel;
        if (panel === "explorer" || panel === "ai") {
          setActivePanel(panel);
        }

        settingsLoadedRef.current = true;

        if (bootstrap.activeFolderId) {
          const folder = findFolder(bootstrap.projects, bootstrap.activeFolderId);
          if (folder) {
            switchToFolder(folder);
          } else {
            openPicker();
          }
        } else {
          openPicker();
        }
      } catch (error) {
        console.error("bootstrap failed:", error);
        applyBootstrap([], null);
        settingsLoadedRef.current = true;
      } finally {
        if (!cancelled) {
          setIsBootstrapping(false);
        }
      }
    })();

    return () => {
      cancelled = true;
    };
  }, [applyBootstrap, switchToFolder, openPicker]);

  useEffect(() => {
    if (!settingsLoadedRef.current) return;
    void invoke("db_set_setting", { key: "sidebar_width", value: String(sidebarWidth) }).catch((error) => {
      console.error("db_set_setting(sidebar_width) error:", error);
    });
  }, [sidebarWidth]);

  useEffect(() => {
    if (!settingsLoadedRef.current) return;
    void invoke("db_set_setting", { key: "active_panel", value: activePanel }).catch((error) => {
      console.error("db_set_setting(active_panel) error:", error);
    });
  }, [activePanel]);

  useEffect(() => {
    setTerminalFocused(false);
    const unlisten = listen<{ terminalId: string; focused: boolean }>("ghostty-focus", (event) => {
      if (event.payload.terminalId === activeTerminalId) {
        setTerminalFocused(event.payload.focused);
      }
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [activeTerminalId]);

  useEffect(() => {
    const unlisten = listen<NvimActionEvent>("nvim-action", () => {
      setActivePanel("ai");
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  useEffect(() => {
    const unlisten = listen<NvimBridgeDebugEvent>("nvim-bridge-debug", (event) => {
      const detail = event.payload.detail ? `: ${event.payload.detail}` : "";
      console.info(`[neoai][bridge] ${event.payload.terminalId} ${event.payload.stage}${detail}`);
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  useEffect(() => {
    const handler = (event: KeyboardEvent) => {
      if (!(event.metaKey && event.shiftKey && event.key.toLowerCase() === "w")) {
        return;
      }
      const activeElement = document.activeElement as HTMLElement | null;
      const tag = (activeElement?.tagName ?? "").toLowerCase();
      const isEditable = Boolean(activeElement?.isContentEditable);
      if (tag === "input" || tag === "textarea" || isEditable) {
        return;
      }
      event.preventDefault();
      void closeSession();
    };

    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [closeSession]);

  const handleResizeStart = (event: React.MouseEvent) => {
    setIsResizing(true);
    event.preventDefault();
  };

  useEffect(() => {
    if (!isResizing) return;

    const handleMouseMove = (event: MouseEvent) => {
      const newWidth = Math.min(Math.max(event.clientX - 16, 200), 500);
      setSidebarWidth(newWidth);
    };

    const handleMouseUp = () => setIsResizing(false);

    document.addEventListener("mousemove", handleMouseMove);
    document.addEventListener("mouseup", handleMouseUp);
    return () => {
      document.removeEventListener("mousemove", handleMouseMove);
      document.removeEventListener("mouseup", handleMouseUp);
    };
  }, [isResizing]);

  if (isBootstrapping || explorer.isLoading) {
    return <main className="app-shell" />;
  }

  return (
    <main className="app-shell" onMouseDownCapture={handleAppMouseDownCapture}>
      <header className="toolbar">
        <div className="toolbar-title">NeoAI</div>
        <div className="toolbar-actions">
          <button
            type="button"
            className={activePanel === "ai" ? "toolbar-btn--active" : ""}
            onClick={() => setActivePanel((panel) => (panel === "ai" ? "explorer" : "ai"))}
          >
            AI
          </button>
          {activeTerminalId && (
            <button type="button" onClick={() => void closeSession()} disabled={sessionManager.isClosing}>
              Close
            </button>
          )}
          <button type="button">Split</button>
          <button type="button">Settings</button>
        </div>
      </header>

      <section
        className={`content ${isResizing ? "content--resizing" : ""}`}
        style={{ gridTemplateColumns: `${sidebarWidth}px 6px 1fr` }}
      >
        <div className="side-panel">
          {activePanel === "explorer" ? (
            <ProjectExplorer
              explorer={explorer}
              onSelectFolder={switchToFolder}
              onRemoveProject={(folderIds) => {
                folderIds.forEach((id) => {
                  void destroyTerminal(`terminal-${id}`);
                });
              }}
              onRemoveFolder={(folderId) => {
                void destroyTerminal(`terminal-${folderId}`);
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
          className={`resize-handle ${isResizing ? "resize-handle--active" : ""}`}
          onMouseDown={handleResizeStart}
        />

        <div className={`terminal-panel${terminalFocused ? " terminal-panel--focused" : ""}`}>
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

      <ProjectPicker
        isOpen={sessionManager.isPickerOpen}
        projects={explorer.projects}
        onSelectFolder={(folder) => {
          explorer.selectFolder(folder);
          switchToFolder(folder);
          closePicker();
        }}
        onClose={closePicker}
        onAddProject={() => {
          void handleAddProject();
        }}
      />
    </main>
  );
}

export default App;
