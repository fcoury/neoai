export type AcpEvent =
  | { type: "contentChunk"; data: string }
  | { type: "thoughtChunk"; data: string }
  | { type: "toolCallStarted"; data: { id: string; title: string; kind: string } }
  | { type: "toolCallUpdated"; data: { id: string; status: string } }
  | { type: "done"; data: { stopReason: string } }
  | { type: "error"; data: string };

export type AgentStatus = "Stopped" | "Starting" | "Running" | { Error: string };

export type AcpInstallPhase =
  | "resolving"
  | "downloading"
  | "verifying"
  | "extracting"
  | "installing"
  | "starting"
  | "done"
  | "error";

export type AcpInstallStatus = {
  phase: AcpInstallPhase;
  message: string;
  version?: string | null;
};

export type AcpPermissionOptionKind =
  | "allow_once"
  | "allow_always"
  | "reject_once"
  | "reject_always";

export type AcpPermissionOption = {
  optionId: string;
  name: string;
  kind: AcpPermissionOptionKind | string;
};

export type AcpPermissionRequest = {
  requestId: string;
  sessionId: string;
  terminalId: string | null;
  toolCallId: string;
  title: string | null;
  kind: string | null;
  options: AcpPermissionOption[];
};
