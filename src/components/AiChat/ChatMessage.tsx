import { useState, useCallback, type ReactNode } from "react";
import { Copy, Check } from "lucide-react";
import hljs from "highlight.js/lib/core";
import javascript from "highlight.js/lib/languages/javascript";
import typescript from "highlight.js/lib/languages/typescript";
import python from "highlight.js/lib/languages/python";
import rust from "highlight.js/lib/languages/rust";
import bash from "highlight.js/lib/languages/bash";
import json from "highlight.js/lib/languages/json";
import css from "highlight.js/lib/languages/css";
import xml from "highlight.js/lib/languages/xml";
import go from "highlight.js/lib/languages/go";
import lua from "highlight.js/lib/languages/lua";
import type { ChatMessage as ChatMessageType } from "../../types/ai-chat";
import { DiffPreview } from "./DiffPreview";

// Register languages once
hljs.registerLanguage("javascript", javascript);
hljs.registerLanguage("js", javascript);
hljs.registerLanguage("typescript", typescript);
hljs.registerLanguage("ts", typescript);
hljs.registerLanguage("python", python);
hljs.registerLanguage("py", python);
hljs.registerLanguage("rust", rust);
hljs.registerLanguage("bash", bash);
hljs.registerLanguage("sh", bash);
hljs.registerLanguage("shell", bash);
hljs.registerLanguage("json", json);
hljs.registerLanguage("css", css);
hljs.registerLanguage("html", xml);
hljs.registerLanguage("xml", xml);
hljs.registerLanguage("go", go);
hljs.registerLanguage("lua", lua);

type Props = {
  message: ChatMessageType;
  onApplyEdits?: (messageId: string) => void;
  onRejectEdits?: (messageId: string) => void;
};

export function ChatMessage({ message, onApplyEdits, onRejectEdits }: Props) {
  const isUser = message.role === "user";
  const isSystem = message.role === "system";
  const roleLabel = isUser ? "You" : isSystem ? "System" : "Assistant";

  return (
    <div className={`chat-message chat-message--${message.role}`}>
      <div className="chat-message__header">
        <span className="chat-message__role">
          {roleLabel}
        </span>
        <span className="chat-message__time">
          {new Date(message.timestamp).toLocaleTimeString()}
        </span>
      </div>

      <div className="chat-message__content">
        {renderContent(message.content)}
      </div>

      {message.attachments && message.attachments.length > 0 && (
        <div className="chat-message__attachments">
          {message.attachments.map((attachment) => (
            <a
              key={attachment.id}
              className="chat-message__attachment"
              href={`data:${attachment.mimeType};base64,${attachment.dataBase64}`}
              target="_blank"
              rel="noreferrer"
              title={attachment.name ?? "Attached image"}
            >
              <img
                src={`data:${attachment.mimeType};base64,${attachment.dataBase64}`}
                alt={attachment.name ?? "Attached image"}
                loading="lazy"
              />
            </a>
          ))}
        </div>
      )}

      {message.proposedEdits && message.proposedEdits.length > 0 && (
        <div className="chat-message__edits">
          {message.proposedEdits.map((edit, i) => (
            <DiffPreview key={i} edit={edit} />
          ))}
          {message.editStatus === "pending" && (
            <div className="chat-message__edit-actions">
              <button
                type="button"
                className="chat-message__btn chat-message__btn--apply"
                onClick={() => onApplyEdits?.(message.id)}
              >
                Apply
              </button>
              <button
                type="button"
                className="chat-message__btn chat-message__btn--reject"
                onClick={() => onRejectEdits?.(message.id)}
              >
                Reject
              </button>
            </div>
          )}
          {message.editStatus === "applied" && (
            <span className="chat-message__edit-status chat-message__edit-status--applied">
              Applied
            </span>
          )}
          {message.editStatus === "rejected" && (
            <span className="chat-message__edit-status chat-message__edit-status--rejected">
              Rejected
            </span>
          )}
        </div>
      )}
    </div>
  );
}

// Highlighted code block with language label and copy button
function CodeBlock({ code, language }: { code: string; language: string }) {
  const [copied, setCopied] = useState(false);

  const handleCopy = useCallback(() => {
    navigator.clipboard.writeText(code).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    });
  }, [code]);

  // Highlight the code
  let highlighted: string;
  try {
    if (language && hljs.getLanguage(language)) {
      highlighted = hljs.highlight(code, { language }).value;
    } else {
      highlighted = hljs.highlightAuto(code).value;
    }
  } catch {
    highlighted = code
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;");
  }

  const displayLang = language || "code";

  return (
    <div className="code-block-wrapper">
      <div className="code-block-header">
        <span className="code-block-lang">{displayLang}</span>
        <button
          type="button"
          className="code-block-copy"
          onClick={handleCopy}
          title="Copy code"
        >
          {copied ? <Check size={14} /> : <Copy size={14} />}
        </button>
      </div>
      <pre className="chat-message__code-block">
        <code dangerouslySetInnerHTML={{ __html: highlighted }} />
      </pre>
    </div>
  );
}

// Inline markdown: `code`, **bold**, *italic*
function FormattedText({ text }: { text: string }) {
  // Split by inline code first, then handle bold/italic in non-code segments
  const parts = text.split(/(`[^`]+`)/g);

  return (
    <>
      {parts.map((part, i) => {
        // Inline code spans
        if (part.startsWith("`") && part.endsWith("`") && part.length > 1) {
          return (
            <code key={i} className="chat-message__inline-code">
              {part.slice(1, -1)}
            </code>
          );
        }

        // Bold and italic in non-code text
        return <span key={i}>{formatBoldItalic(part)}</span>;
      })}
    </>
  );
}

// Process **bold** and *italic* markers
function formatBoldItalic(text: string): (string | ReactNode)[] {
  // Match **bold** first, then *italic*
  const result: (string | ReactNode)[] = [];
  const regex = /(\*\*(.+?)\*\*|\*(.+?)\*)/g;
  let lastIndex = 0;
  let match: RegExpExecArray | null;

  while ((match = regex.exec(text)) !== null) {
    // Text before the match
    if (match.index > lastIndex) {
      result.push(text.slice(lastIndex, match.index));
    }

    if (match[2]) {
      // **bold**
      result.push(<strong key={match.index}>{match[2]}</strong>);
    } else if (match[3]) {
      // *italic*
      result.push(<em key={match.index}>{match[3]}</em>);
    }

    lastIndex = match.index + match[0].length;
  }

  // Remaining text
  if (lastIndex < text.length) {
    result.push(text.slice(lastIndex));
  }

  return result.length > 0 ? result : [text];
}

function renderContent(content: string) {
  // Split on fenced code blocks
  const parts = content.split(/(```[\s\S]*?```)/g);
  return parts.map((part, i) => {
    if (part.startsWith("```") && part.endsWith("```")) {
      const inner = part.slice(3, -3);
      const newlineIdx = inner.indexOf("\n");
      const language = newlineIdx >= 0 ? inner.slice(0, newlineIdx).trim() : "";
      const code = newlineIdx >= 0 ? inner.slice(newlineIdx + 1) : inner;
      return <CodeBlock key={i} code={code} language={language} />;
    }
    return (
      <span key={i} className="chat-message__text">
        <FormattedText text={part} />
      </span>
    );
  });
}
