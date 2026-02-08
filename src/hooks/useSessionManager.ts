import { useCallback, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

interface UseSessionManagerOpts {
  activeTerminalId: string | null;
  destroyTerminal: (termId: string) => Promise<void>;
  onSessionClosed?: (folderId: string, screenshotPath: string | null) => void;
}

interface UseSessionManagerReturn {
  isPickerOpen: boolean;
  isClosing: boolean;
  closeSession: () => Promise<void>;
  openPicker: () => void;
  closePicker: () => void;
}

function folderIdFromTerminalId(terminalId: string): string {
  return terminalId.replace(/^terminal-/, "");
}

export function useSessionManager({
  activeTerminalId,
  destroyTerminal,
  onSessionClosed,
}: UseSessionManagerOpts): UseSessionManagerReturn {
  const [isPickerOpen, setIsPickerOpen] = useState(false);
  const [isClosing, setIsClosing] = useState(false);
  const isClosingRef = useRef(false);

  const closeSession = useCallback(async () => {
    if (!activeTerminalId || isClosingRef.current) return;

    isClosingRef.current = true;
    setIsClosing(true);

    const folderId = folderIdFromTerminalId(activeTerminalId);
    let screenshotPath: string | null = null;

    try {
      try {
        screenshotPath = await invoke<string>("ghostty_screenshot", { id: activeTerminalId });
      } catch (error) {
        console.warn("ghostty_screenshot failed:", error);
      }

      await invoke("db_update_folder_session", { folderId, screenshotPath });
      await destroyTerminal(activeTerminalId);
      await invoke("db_set_active_folder", { folderId: null });
      onSessionClosed?.(folderId, screenshotPath);
      setIsPickerOpen(true);
    } finally {
      isClosingRef.current = false;
      setIsClosing(false);
    }
  }, [activeTerminalId, destroyTerminal, onSessionClosed]);

  const openPicker = useCallback(() => {
    setIsPickerOpen(true);
  }, []);

  const closePicker = useCallback(() => {
    setIsPickerOpen(false);
  }, []);

  return {
    isPickerOpen,
    isClosing,
    closeSession,
    openPicker,
    closePicker,
  };
}
