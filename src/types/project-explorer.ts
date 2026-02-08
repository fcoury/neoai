// Project Explorer Data Types

export interface GitDiffStats {
  additions: number;
  deletions: number;
}

export interface PullRequest {
  number: number;
  url?: string;
  state: 'open' | 'closed' | 'merged';
}

export interface ProjectFolder {
  id: string;
  name: string;           // basename only
  path: string;           // full path (internal)
  branch: string;
  diffStats: GitDiffStats | null;
  pullRequest: PullRequest | null;
  isActive?: boolean;
  screenshotPath?: string | null;
  lastUsedAt?: number | null;
}

export interface Project {
  id: string;
  name: string;
  rootPath: string;
  folders: ProjectFolder[];
  isExpanded?: boolean;
}
