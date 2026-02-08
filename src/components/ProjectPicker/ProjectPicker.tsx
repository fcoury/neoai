import { useEffect, useMemo } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import type { Project, ProjectFolder } from "../../types/project-explorer";
import "./ProjectPicker.css";

interface ProjectPickerProps {
  isOpen: boolean;
  projects: Project[];
  onSelectFolder: (folder: ProjectFolder) => void;
  onClose: () => void;
  onAddProject: () => void;
}

function formatRelativeTime(timestamp: number | null | undefined): string {
  if (!timestamp) return "Never used";
  const now = Date.now();
  const diffMs = Math.max(0, now - timestamp * 1000);
  const diffMin = Math.floor(diffMs / 60000);
  if (diffMin < 1) return "Just now";
  if (diffMin < 60) return `${diffMin}m ago`;
  const diffHours = Math.floor(diffMin / 60);
  if (diffHours < 24) return `${diffHours}h ago`;
  const diffDays = Math.floor(diffHours / 24);
  if (diffDays < 30) return `${diffDays}d ago`;
  const diffMonths = Math.floor(diffDays / 30);
  return `${diffMonths}mo ago`;
}

export function ProjectPicker({
  isOpen,
  projects,
  onSelectFolder,
  onClose,
  onAddProject,
}: ProjectPickerProps) {
  const folders = useMemo(() => {
    return projects
      .flatMap((project) => project.folders)
      .sort((a, b) => {
        const aValue = a.lastUsedAt ?? -1;
        const bValue = b.lastUsedAt ?? -1;
        return bValue - aValue;
      });
  }, [projects]);

  useEffect(() => {
    if (!isOpen) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        onClose();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [isOpen, onClose]);

  if (!isOpen) return null;

  return (
    <div className="modal-overlay" onMouseDown={onClose}>
      <div className="modal project-picker" onMouseDown={(event) => event.stopPropagation()}>
        <div className="project-picker__header">
          <h3>Choose Project</h3>
          <button type="button" className="project-picker__close" onClick={onClose}>
            Close
          </button>
        </div>

        {folders.length === 0 ? (
          <div className="project-picker__empty">
            <p>No projects available.</p>
            <button type="button" onClick={onAddProject}>
              Add Project
            </button>
          </div>
        ) : (
          <div className="project-picker__grid">
            {folders.map((folder) => {
              const imageSrc = folder.screenshotPath ? convertFileSrc(folder.screenshotPath) : null;
              return (
                <button
                  key={folder.id}
                  type="button"
                  className="project-picker__card"
                  onClick={() => onSelectFolder(folder)}
                >
                  <div className="project-picker__thumb">
                    {imageSrc ? (
                      <img src={imageSrc} alt={`${folder.name} screenshot`} />
                    ) : (
                      <div className="project-picker__thumb-placeholder" />
                    )}
                  </div>
                  <div className="project-picker__body">
                    <div className="project-picker__title-row">
                      <strong>{folder.name}</strong>
                      <span className="project-picker__branch">{folder.branch || "-"}</span>
                    </div>
                    <div className="project-picker__path" title={folder.path}>
                      {folder.path}
                    </div>
                    <div className="project-picker__meta">{formatRelativeTime(folder.lastUsedAt)}</div>
                  </div>
                </button>
              );
            })}
          </div>
        )}
      </div>
    </div>
  );
}
