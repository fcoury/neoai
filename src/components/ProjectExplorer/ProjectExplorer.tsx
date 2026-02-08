import { useEffect, useRef } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import type { UseProjectExplorerReturn } from "../../hooks/useProjectExplorer";
import type { ProjectFolder } from "../../types/project-explorer";
import { ProjectList } from "./ProjectList";
import "./ProjectExplorer.css";

interface ProjectExplorerProps {
  explorer: UseProjectExplorerReturn;
  onSelectFolder?: (folder: ProjectFolder) => void;
  onRemoveProject?: (folderIds: string[]) => void;
  onRemoveFolder?: (folderId: string) => void;
}

export function ProjectExplorer({
  explorer,
  onSelectFolder,
  onRemoveProject,
  onRemoveFolder,
}: ProjectExplorerProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const {
    projects,
    focusedFolderId,
    showHotkeys,
    toggleProject,
    selectFolder,
    openPullRequest,
    handleKeyDown,
    handleKeyUp,
    addProject,
    addFolder,
    removeProject,
    removeFolder,
  } = explorer;

  const handleSelectFolder = (folder: ProjectFolder) => {
    selectFolder(folder);
    onSelectFolder?.(folder);
  };

  const handleAddProject = async () => {
    const selected = await open({
      directory: true,
      multiple: false,
      title: "Select Project Folder",
    });
    if (selected) {
      const name = selected.split("/").pop() || selected;
      await addProject(selected, name);
    }
  };

  const handleRemoveProject = async (projectId: string) => {
    const project = projects.find((candidate) => candidate.id === projectId);
    const folderIds = project ? project.folders.map((folder) => folder.id) : [];
    await removeProject(projectId);
    onRemoveProject?.(folderIds);
  };

  const handleRemoveFolder = async (folderId: string) => {
    await removeFolder(folderId);
    onRemoveFolder?.(folderId);
  };

  const handleAddFolder = async (projectId: string) => {
    const selected = await open({
      directory: true,
      multiple: false,
      title: "Add Folder to Project",
    });
    if (selected) {
      const name = selected.split("/").pop() || selected;
      await addFolder(projectId, selected, name);
    }
  };

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;

    window.addEventListener("keydown", handleKeyDown);
    window.addEventListener("keyup", handleKeyUp);

    return () => {
      window.removeEventListener("keydown", handleKeyDown);
      window.removeEventListener("keyup", handleKeyUp);
    };
  }, [handleKeyDown, handleKeyUp]);

  const containerClass = [
    "project-explorer",
    showHotkeys ? "project-explorer--show-hotkeys" : "",
  ]
    .filter(Boolean)
    .join(" ");

  return (
    <div ref={containerRef} className={containerClass} tabIndex={-1}>
      <div className="project-explorer-header">
        <h3 className="project-explorer-title">Projects</h3>
        <button className="add-project-btn" onClick={handleAddProject} title="Add project">
          +
        </button>
      </div>

      {projects.length === 0 ? (
        <div className="project-explorer-empty">
          <p>No projects yet</p>
          <button className="add-project-empty-btn" onClick={handleAddProject}>
            Add Project
          </button>
        </div>
      ) : (
        <ProjectList
          projects={projects}
          onToggleProject={toggleProject}
          onSelectFolder={handleSelectFolder}
          onPRClick={openPullRequest}
          onAddFolder={handleAddFolder}
          onRemoveProject={(projectId) => {
            void handleRemoveProject(projectId);
          }}
          onRemoveFolder={(folderId) => {
            void handleRemoveFolder(folderId);
          }}
          focusedFolderId={focusedFolderId}
        />
      )}
    </div>
  );
}
