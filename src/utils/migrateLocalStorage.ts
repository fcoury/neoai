import { invoke } from "@tauri-apps/api/core";
import type { ChatMessage } from "../types/ai-chat";
import type { Project } from "../types/project-explorer";

const MIGRATED_KEY = "libg:migrated";
const LEGACY_PREFIX = "libg:";

interface MigrationPayload {
  projects?: Project[];
  activeFolderId?: string | null;
  chatMessages?: ChatMessage[];
  autoApply?: boolean;
  sidebarWidth?: number;
  activePanel?: string;
}

function parseValue<T>(raw: string | null): T | undefined {
  if (!raw) return undefined;
  try {
    return JSON.parse(raw) as T;
  } catch {
    return undefined;
  }
}

function clearLegacyKeys() {
  const keysToRemove: string[] = [];
  for (let i = 0; i < localStorage.length; i += 1) {
    const key = localStorage.key(i);
    if (key?.startsWith(LEGACY_PREFIX)) {
      keysToRemove.push(key);
    }
  }
  keysToRemove.forEach((key) => localStorage.removeItem(key));
}

export async function migrateLocalStorageOnce(): Promise<void> {
  if (localStorage.getItem(MIGRATED_KEY) === "1") {
    return;
  }

  const projects = parseValue<Project[]>(localStorage.getItem("libg:projects"));
  const activeFolderId = parseValue<string | null>(localStorage.getItem("libg:activeFolderId"));
  const chatMessages = parseValue<ChatMessage[]>(localStorage.getItem("libg:chatMessages"));
  const autoApply = parseValue<boolean>(localStorage.getItem("libg:autoApply"));
  const sidebarWidth = parseValue<number>(localStorage.getItem("libg:sidebarWidth"));
  const activePanel = parseValue<string>(localStorage.getItem("libg:activePanel"));

  const hasLegacyData =
    projects !== undefined ||
    activeFolderId !== undefined ||
    chatMessages !== undefined ||
    autoApply !== undefined ||
    sidebarWidth !== undefined ||
    activePanel !== undefined;

  if (!hasLegacyData) {
    localStorage.setItem(MIGRATED_KEY, "1");
    return;
  }

  const payload: MigrationPayload = {
    projects,
    activeFolderId,
    chatMessages,
    autoApply,
    sidebarWidth,
    activePanel,
  };

  await invoke("db_migrate_from_localstorage", { payload });
  clearLegacyKeys();
  localStorage.setItem(MIGRATED_KEY, "1");
}
