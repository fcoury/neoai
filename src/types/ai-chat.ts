import type { NvimContext, Diagnostic, BufferEdit } from "./nvim";

export interface ChatAttachment {
  id: string;
  kind: "image";
  mimeType: string;
  dataBase64: string;
  name?: string;
  sizeBytes: number;
  width?: number;
  height?: number;
}

export interface ChatMessage {
  id: string;
  role: "user" | "assistant" | "system";
  content: string;
  timestamp: number;
  systemKind?: "action-summary" | "status-note";
  context?: NvimContext;
  diagnostics?: Diagnostic[];
  attachments?: ChatAttachment[];
  proposedEdits?: BufferEdit[];
  editStatus?: "pending" | "applied" | "rejected";
}

export interface AgentConfig {
  path: string;
  name?: string;
}
