import {
  useState,
  useRef,
  useCallback,
  type ChangeEvent,
  type DragEvent,
  type KeyboardEvent,
  type ClipboardEvent,
} from "react";
import { Send, ImagePlus, X } from "lucide-react";
import type { ChatAttachment } from "../../types/ai-chat";

const MAX_ATTACHMENTS_PER_MESSAGE = 4;
const MAX_ATTACHMENT_BYTES = 5 * 1024 * 1024;
const MAX_TOTAL_ATTACHMENT_BYTES = 20 * 1024 * 1024;
const ALLOWED_IMAGE_MIME_TYPES = new Set([
  "image/png",
  "image/jpeg",
  "image/webp",
  "image/gif",
]);

type SendPayload = {
  content: string;
  attachments: ChatAttachment[];
};

type Props = {
  onSend: (payload: SendPayload) => void;
  disabled?: boolean;
  placeholder?: string;
};

function nextAttachmentId(): string {
  return `att-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
}

function readBlobAsDataUrl(blob: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onerror = () => reject(new Error("Failed to read image"));
    reader.onload = () => {
      if (typeof reader.result !== "string") {
        reject(new Error("Invalid image data"));
        return;
      }
      resolve(reader.result);
    };
    reader.readAsDataURL(blob);
  });
}

function loadImageDimensions(dataUrl: string): Promise<{ width: number; height: number }> {
  return new Promise((resolve, reject) => {
    const img = new Image();
    img.onload = () => resolve({ width: img.naturalWidth, height: img.naturalHeight });
    img.onerror = () => reject(new Error("Failed to decode image dimensions"));
    img.src = dataUrl;
  });
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function dataUrlToBase64(dataUrl: string): string {
  const idx = dataUrl.indexOf(",");
  return idx >= 0 ? dataUrl.slice(idx + 1) : dataUrl;
}

export function ChatInput({
  onSend,
  disabled = false,
  placeholder = "Ask about your code...",
}: Props) {
  const [value, setValue] = useState("");
  const [attachments, setAttachments] = useState<ChatAttachment[]>([]);
  const [attachmentError, setAttachmentError] = useState<string | null>(null);
  const [isDragOver, setIsDragOver] = useState(false);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);

  const totalAttachmentBytes = attachments.reduce((sum, att) => sum + att.sizeBytes, 0);

  const handleSend = useCallback(() => {
    const trimmed = value.trim();
    if ((!trimmed && attachments.length === 0) || disabled) return;
    onSend({ content: trimmed, attachments });
    setValue("");
    setAttachments([]);
    setAttachmentError(null);
    if (textareaRef.current) {
      textareaRef.current.style.height = "auto";
    }
  }, [value, attachments, disabled, onSend]);

  const validateCandidate = useCallback(
    (mimeType: string, sizeBytes: number, name: string | undefined, currentCount: number) => {
      if (!ALLOWED_IMAGE_MIME_TYPES.has(mimeType)) {
        return `Unsupported image type: ${mimeType || "unknown"}.`;
      }
      if (sizeBytes > MAX_ATTACHMENT_BYTES) {
        return `Image "${name ?? "unnamed"}" exceeds 5MB.`;
      }
      if (currentCount >= MAX_ATTACHMENTS_PER_MESSAGE) {
        return `You can attach up to ${MAX_ATTACHMENTS_PER_MESSAGE} images.`;
      }
      return null;
    },
    []
  );

  const ingestFiles = useCallback(
    async (incoming: File[]) => {
      if (incoming.length === 0) return;
      setAttachmentError(null);

      let next = [...attachments];
      for (const file of incoming) {
        const validationError = validateCandidate(file.type, file.size, file.name, next.length);
        if (validationError) {
          setAttachmentError(validationError);
          continue;
        }

        const candidateTotalBytes = next.reduce((sum, att) => sum + att.sizeBytes, 0) + file.size;
        if (candidateTotalBytes > MAX_TOTAL_ATTACHMENT_BYTES) {
          setAttachmentError("Total image attachments exceed 20MB.");
          continue;
        }

        try {
          const dataUrl = await readBlobAsDataUrl(file);
          const { width, height } = await loadImageDimensions(dataUrl);
          next.push({
            id: nextAttachmentId(),
            kind: "image",
            mimeType: file.type,
            dataBase64: dataUrlToBase64(dataUrl),
            name: file.name,
            sizeBytes: file.size,
            width,
            height,
          });
        } catch (error) {
          setAttachmentError(`Failed to load image \"${file.name}\": ${String(error)}`);
        }
      }

      setAttachments(next);
    },
    [attachments, validateCandidate]
  );

  const handleFileInput = useCallback(
    async (event: ChangeEvent<HTMLInputElement>) => {
      const files = event.target.files ? Array.from(event.target.files) : [];
      await ingestFiles(files);
      event.target.value = "";
    },
    [ingestFiles]
  );

  const handlePaste = useCallback(
    async (event: ClipboardEvent<HTMLTextAreaElement>) => {
      const files: File[] = [];
      for (const item of Array.from(event.clipboardData.items)) {
        if (item.kind !== "file") continue;
        const blob = item.getAsFile();
        if (!blob) continue;
        if (!blob.type.startsWith("image/")) continue;
        files.push(blob);
      }
      if (files.length === 0) return;
      event.preventDefault();
      await ingestFiles(files);
    },
    [ingestFiles]
  );

  // Enter sends, Shift+Enter or Option+Enter inserts newline
  const handleKeyDown = useCallback(
    (e: KeyboardEvent<HTMLTextAreaElement>) => {
      if (e.key === "Enter" && !e.shiftKey && !e.altKey) {
        e.preventDefault();
        handleSend();
      }
    },
    [handleSend]
  );

  const handleInput = useCallback(() => {
    const el = textareaRef.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = Math.min(el.scrollHeight, 300) + "px";
  }, []);

  const handleDrop = useCallback(
    async (event: DragEvent<HTMLDivElement>) => {
      event.preventDefault();
      setIsDragOver(false);
      if (disabled) return;
      const files = Array.from(event.dataTransfer.files).filter((f) =>
        f.type.startsWith("image/")
      );
      await ingestFiles(files);
    },
    [disabled, ingestFiles]
  );

  const handleDragOver = useCallback((event: DragEvent<HTMLDivElement>) => {
    event.preventDefault();
    setIsDragOver(true);
  }, []);

  const handleDragLeave = useCallback((event: DragEvent<HTMLDivElement>) => {
    event.preventDefault();
    setIsDragOver(false);
  }, []);

  const removeAttachment = useCallback((id: string) => {
    setAttachments((prev) => prev.filter((att) => att.id !== id));
    setAttachmentError(null);
  }, []);

  return (
    <div
      className={`chat-input${isDragOver ? " chat-input--drag-over" : ""}`}
      onDrop={handleDrop}
      onDragOver={handleDragOver}
      onDragLeave={handleDragLeave}
    >
      {attachments.length > 0 && (
        <div className="chat-input__attachments" aria-label="Pending image attachments">
          {attachments.map((attachment) => (
            <div key={attachment.id} className="chat-input__attachment">
              <img
                className="chat-input__attachment-thumb"
                src={`data:${attachment.mimeType};base64,${attachment.dataBase64}`}
                alt={attachment.name ?? "Attached image"}
              />
              <div className="chat-input__attachment-meta">
                <span className="chat-input__attachment-name" title={attachment.name}>
                  {attachment.name ?? "Pasted image"}
                </span>
                <span className="chat-input__attachment-size">{formatBytes(attachment.sizeBytes)}</span>
              </div>
              <button
                type="button"
                className="chat-input__attachment-remove"
                title="Remove image"
                onClick={() => removeAttachment(attachment.id)}
                disabled={disabled}
              >
                <X size={12} />
              </button>
            </div>
          ))}
        </div>
      )}

      {attachmentError && <div className="chat-input__error">{attachmentError}</div>}

      <textarea
        ref={textareaRef}
        className="chat-input__textarea"
        value={value}
        onChange={(e) => {
          setValue(e.target.value);
          handleInput();
        }}
        onKeyDown={handleKeyDown}
        onPaste={handlePaste}
        placeholder={placeholder}
        disabled={disabled}
        rows={3}
      />

      <div className="chat-input__actions">
        <input
          ref={fileInputRef}
          type="file"
          accept="image/png,image/jpeg,image/webp,image/gif"
          multiple
          className="chat-input__file-input"
          onChange={handleFileInput}
          disabled={disabled}
        />
        <button
          type="button"
          className="chat-input__attach"
          onClick={() => fileInputRef.current?.click()}
          disabled={disabled || attachments.length >= MAX_ATTACHMENTS_PER_MESSAGE}
          title="Attach image"
        >
          <ImagePlus size={16} />
        </button>
        <button
          type="button"
          className="chat-input__send"
          onClick={handleSend}
          disabled={disabled || (!value.trim() && attachments.length === 0) || totalAttachmentBytes > MAX_TOTAL_ATTACHMENT_BYTES}
          title="Send (Enter)"
        >
          <Send size={16} />
        </button>
      </div>
    </div>
  );
}
