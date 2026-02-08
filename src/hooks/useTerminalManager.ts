import { useState, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { ProjectFolder } from '../types/project-explorer';

interface TerminalEntry {
  folderId: string;
  path: string;
}

export function useTerminalManager() {
  const [activeTerminalId, setActiveTerminalId] = useState<string | null>(null);
  const [terminals, setTerminals] = useState<Map<string, TerminalEntry>>(new Map());

  const switchToFolder = useCallback((folder: ProjectFolder) => {
    const termId = `terminal-${folder.id}`;
    // Lazily add to map (triggers <Ghostty> mount on first select)
    setTerminals(prev => {
      if (prev.has(termId)) return prev;
      const next = new Map(prev);
      next.set(termId, { folderId: folder.id, path: folder.path });
      return next;
    });
    setActiveTerminalId(termId);
  }, []);

  const destroyTerminal = useCallback(async (termId: string) => {
    try {
      await invoke("nvim_disconnect", { terminalId: termId });
    } catch (e) {
      console.error("nvim_disconnect error:", e);
    }

    try {
      await invoke("acp_unbind_terminal", { terminalId: termId });
    } catch (e) {
      console.error("acp_unbind_terminal error:", e);
    }

    try {
      await invoke("remove_socket_path", { terminalId: termId });
    } catch (e) {
      console.error("remove_socket_path error:", e);
    }
    setTerminals(prev => {
      const next = new Map(prev);
      next.delete(termId);
      return next;
    });
    setActiveTerminalId(prev => prev === termId ? null : prev);
  }, []);

  return { activeTerminalId, terminals, switchToFolder, destroyTerminal };
}
