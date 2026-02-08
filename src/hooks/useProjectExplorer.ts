import { useState, useCallback, useEffect, useMemo } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Project, ProjectFolder } from "../types/project-explorer";

function stableId(prefix: string, path: string): string {
  let hash = 0;
  for (let i = 0; i < path.length; i += 1) {
    hash = ((hash << 5) - hash + path.charCodeAt(i)) | 0;
  }
  return `${prefix}-${(hash >>> 0).toString(36)}`;
}

function normalizeFolder(folder: ProjectFolder): ProjectFolder {
  return {
    ...folder,
    branch: folder.branch ?? "",
    diffStats: folder.diffStats ?? null,
    pullRequest: folder.pullRequest ?? null,
    screenshotPath: folder.screenshotPath ?? null,
    lastUsedAt: folder.lastUsedAt ?? null,
  };
}

function normalizeProject(project: Project): Project {
  return {
    ...project,
    isExpanded: project.isExpanded ?? true,
    folders: project.folders.map(normalizeFolder),
  };
}

export interface UseProjectExplorerReturn {
  projects: Project[];
  activeFolder: ProjectFolder | null;
  activeFolderId: string | null;
  focusedFolderId: string | null;
  isLoading: boolean;
  showHotkeys: boolean;
  applyBootstrap: (projects: Project[], activeFolderId: string | null) => void;
  clearActiveFolder: () => void;
  markFolderSession: (folderId: string, screenshotPath: string | null) => void;
  toggleProject: (projectId: string) => void;
  selectFolder: (folder: ProjectFolder) => void;
  openPullRequest: (pr: { url?: string; number: number }) => void;
  handleKeyDown: (e: KeyboardEvent) => void;
  handleKeyUp: (e: KeyboardEvent) => void;
  getAllFolders: () => ProjectFolder[];
  addProject: (path: string, name: string) => Promise<void>;
  addFolder: (projectId: string, path: string, name: string) => Promise<void>;
  removeProject: (projectId: string) => Promise<void>;
  removeFolder: (folderId: string) => Promise<void>;
}

export function useProjectExplorer(): UseProjectExplorerReturn {
  const [projects, setProjects] = useState<Project[]>([]);
  const [activeFolderId, setActiveFolderId] = useState<string | null>(null);
  const [focusedFolderId, setFocusedFolderId] = useState<string | null>(null);
  const [showHotkeys, setShowHotkeys] = useState(false);
  const [isLoading, setIsLoading] = useState(true);

  const applyBootstrap = useCallback((nextProjects: Project[], nextActiveFolderId: string | null) => {
    setProjects(nextProjects.map(normalizeProject));
    setActiveFolderId(nextActiveFolderId);
    setFocusedFolderId(nextActiveFolderId);
    setIsLoading(false);
  }, []);

  const clearActiveFolder = useCallback(() => {
    setActiveFolderId(null);
    setProjects((prev) =>
      prev.map((project) => ({
        ...project,
        folders: project.folders.map((folder) => ({ ...folder, isActive: false })),
      }))
    );
  }, []);

  const markFolderSession = useCallback((folderId: string, screenshotPath: string | null) => {
    const now = Math.floor(Date.now() / 1000);
    setProjects((prev) =>
      prev.map((project) => ({
        ...project,
        folders: project.folders.map((folder) =>
          folder.id === folderId
            ? {
                ...folder,
                screenshotPath: screenshotPath ?? folder.screenshotPath ?? null,
                lastUsedAt: now,
                isActive: false,
              }
            : { ...folder, isActive: false }
        ),
      }))
    );
  }, []);

  const activeFolder = useMemo(() => {
    if (!activeFolderId) return null;
    for (const project of projects) {
      const found = project.folders.find((folder) => folder.id === activeFolderId);
      if (found) return found;
    }
    return null;
  }, [activeFolderId, projects]);

  const getAllFolders = useCallback((): ProjectFolder[] => {
    return projects.filter((project) => project.isExpanded).flatMap((project) => project.folders);
  }, [projects]);

  const allFolders = useMemo(() => projects.flatMap((project) => project.folders), [projects]);

  const toggleProject = useCallback((projectId: string) => {
    setProjects((prev) =>
      prev.map((project) =>
        project.id === projectId ? { ...project, isExpanded: !project.isExpanded } : project
      )
    );

    void invoke("db_toggle_project", { projectId }).catch((error) => {
      console.error("db_toggle_project error:", error);
    });
  }, []);

  const selectFolder = useCallback((folder: ProjectFolder) => {
    setActiveFolderId(folder.id);
    setFocusedFolderId(folder.id);
    setProjects((prev) =>
      prev.map((project) => ({
        ...project,
        folders: project.folders.map((candidate) => ({
          ...candidate,
          isActive: candidate.id === folder.id,
        })),
      }))
    );

    void invoke("db_set_active_folder", { folderId: folder.id }).catch((error) => {
      console.error("db_set_active_folder error:", error);
    });
  }, []);

  const openPullRequest = useCallback((pr: { url?: string; number: number }) => {
    if (pr.url) {
      window.open(pr.url, "_blank");
    }
  }, []);

  const addProject = useCallback(async (path: string, name: string) => {
    const projectId = stableId("proj", path);
    const folderId = stableId("folder", path);

    try {
      const created = await invoke<Project>("db_add_project", {
        id: projectId,
        name,
        rootPath: path,
        folderId,
        folderName: name,
        folderPath: path,
      });
      const normalized = normalizeProject(created);
      setProjects((prev) => {
        if (prev.some((project) => project.id === normalized.id)) {
          return prev;
        }
        return [...prev, normalized];
      });
    } catch (error) {
      console.error("db_add_project error:", error);
    }
  }, []);

  const addFolder = useCallback(async (projectId: string, path: string, name: string) => {
    const folderId = stableId("folder", path);
    try {
      const created = await invoke<ProjectFolder>("db_add_folder", {
        id: folderId,
        projectId,
        name,
        path,
      });
      const normalized = normalizeFolder(created);
      setProjects((prev) =>
        prev.map((project) => {
          if (project.id !== projectId) return project;
          if (project.folders.some((folder) => folder.id === normalized.id)) {
            return project;
          }
          return { ...project, folders: [...project.folders, normalized] };
        })
      );
    } catch (error) {
      console.error("db_add_folder error:", error);
    }
  }, []);

  const removeProject = useCallback(async (projectId: string) => {
    try {
      await invoke("db_remove_project", { projectId });
      setProjects((prev) => {
        const removed = prev.find((project) => project.id === projectId);
        const next = prev.filter((project) => project.id !== projectId);
        if (removed && activeFolderId && removed.folders.some((folder) => folder.id === activeFolderId)) {
          setActiveFolderId(null);
        }
        return next;
      });
    } catch (error) {
      console.error("db_remove_project error:", error);
    }
  }, [activeFolderId]);

  const removeFolder = useCallback(async (folderId: string) => {
    try {
      await invoke("db_remove_folder", { folderId });
      setProjects((prev) =>
        prev.map((project) => ({
          ...project,
          folders: project.folders.filter((folder) => folder.id !== folderId),
        }))
      );
      if (activeFolderId === folderId) {
        setActiveFolderId(null);
      }
    } catch (error) {
      console.error("db_remove_folder error:", error);
    }
  }, [activeFolderId]);

  const handleKeyDown = useCallback((event: KeyboardEvent) => {
    if (event.key === "Meta") {
      setShowHotkeys(true);
    }

    if (event.metaKey && event.key >= "1" && event.key <= "9") {
      event.preventDefault();
      const index = parseInt(event.key, 10) - 1;
      const targetFolder = allFolders[index];
      if (targetFolder) {
        const containingProject = projects.find((project) =>
          project.folders.some((folder) => folder.id === targetFolder.id)
        );

        if (containingProject && !containingProject.isExpanded) {
          setProjects((prev) =>
            prev.map((project) =>
              project.id === containingProject.id ? { ...project, isExpanded: true } : project
            )
          );
          void invoke("db_toggle_project", { projectId: containingProject.id }).catch((error) => {
            console.error("db_toggle_project error:", error);
          });
        }

        selectFolder(targetFolder);
      }
      return;
    }

    const visibleFolders = getAllFolders();
    if (visibleFolders.length === 0) return;

    const currentIndex = focusedFolderId
      ? visibleFolders.findIndex((folder) => folder.id === focusedFolderId)
      : -1;

    if (event.key === "ArrowDown") {
      event.preventDefault();
      const nextIndex = currentIndex < visibleFolders.length - 1 ? currentIndex + 1 : 0;
      setFocusedFolderId(visibleFolders[nextIndex].id);
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      const prevIndex = currentIndex > 0 ? currentIndex - 1 : visibleFolders.length - 1;
      setFocusedFolderId(visibleFolders[prevIndex].id);
    } else if ((event.key === "Enter" || event.key === " ") && focusedFolderId) {
      event.preventDefault();
      const folder = visibleFolders.find((candidate) => candidate.id === focusedFolderId);
      if (folder) {
        selectFolder(folder);
      }
    }
  }, [allFolders, projects, getAllFolders, focusedFolderId, selectFolder]);

  const handleKeyUp = useCallback((event: KeyboardEvent) => {
    if (event.key === "Meta") {
      setShowHotkeys(false);
    }
  }, []);

  useEffect(() => {
    if (activeFolder && !focusedFolderId) {
      setFocusedFolderId(activeFolder.id);
    }
  }, [activeFolder, focusedFolderId]);

  return {
    projects,
    activeFolder,
    activeFolderId,
    focusedFolderId,
    isLoading,
    showHotkeys,
    applyBootstrap,
    clearActiveFolder,
    markFolderSession,
    toggleProject,
    selectFolder,
    openPullRequest,
    handleKeyDown,
    handleKeyUp,
    getAllFolders,
    addProject,
    addFolder,
    removeProject,
    removeFolder,
  };
}
