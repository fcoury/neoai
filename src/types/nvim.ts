export interface CursorPosition {
  line: number;
  col: number;
}

export interface NvimContext {
  cursor: CursorPosition;
  filePath: string;
  fileType: string;
  bufferId: number;
  lineCount: number;
  modified: boolean;
  visibleLines: string[];
  visibleRange: [number, number];
}

export interface Diagnostic {
  line: number;
  col: number;
  severity: number;
  message: string;
  source: string;
}

export interface BufferContent {
  filePath: string;
  lines: string[];
  lineCount: number;
}

export interface BufferEdit {
  startLine: number;
  endLine: number;
  newLines: string[];
  filePath?: string;
  targetLine?: number;
}

/** Simple status for UI display. The backend ConnectionStatus enum is richer
 *  (Connected carries socketPath), but the hook normalizes to these strings. */
export type ConnectionStatus = "Connected" | "Disconnected" | "Error";

export type KeymapStatus = "unknown" | "present" | "missing" | "error";

export interface NvimHealth {
  connected: boolean;
  channelId: number | null;
  keymapsInjected: boolean;
  socketPath: string | null;
  lastError: string | null;
}

// -- Neovim action types (sent from Neovim → Tauri via rpcnotify) --

export interface ActionDiagnostic {
  line: number;
  col: number;
  severity: number;
  message: string;
  source: string;
}

export type NvimAction =
  | {
      action: "fixDiagnostic";
      filePath: string;
      cursorLine: number;
      cursorCol: number;
      diagnostic: ActionDiagnostic;
      contextLines: string[];
      contextStartLine: number;
    }
  | {
      action: "implement";
      filePath: string;
      fileType: string;
      cursorLine: number;
      signatureLines: string[];
      contextLines: string[];
      contextStartLine: number;
    }
  | {
      action: "explain";
      filePath: string;
      fileType: string;
      cursorLine: number;
      targetText: string;
      contextLines: string[];
      contextStartLine: number;
    }
  | {
      action: "ask";
      filePath: string;
      fileType: string;
      cursorLine: number;
      prompt: string;
      selection: string | null;
      contextLines: string[];
      contextStartLine: number;
    };

export interface NvimActionEvent {
  terminalId: string;
  action: NvimAction;
}

export interface NvimBridgeDebugEvent {
  terminalId: string;
  stage: string;
  detail?: string;
}

export interface NvimCursorFollowEvent {
  terminalId: string;
  filePath: string;
  line: number;
  source: string;
}

export interface TmuxStatus {
  terminalId: string;
  available: boolean;
  enabled: boolean;
  mode: "tmux" | "fallback";
  sessionName: string;
  error?: string;
}

export interface NvimStartLaunchResult {
  launchMode: "tmux" | "direct" | "tmuxUnavailable";
  sessionName?: string;
  message: string;
}
