import type { ProjectFolder, PullRequest } from '../../types/project-explorer';
import { PRIndicator } from './PRIndicator';
import { DiffStats } from './DiffStats';
import { CloseIcon } from './icons';

interface FolderItemProps {
  folder: ProjectFolder;
  onSelect: (folder: ProjectFolder) => void;
  onPRClick: (pr: PullRequest) => void;
  onRemove?: (folderId: string) => void;
  globalIndex?: number;
  isFocused?: boolean;
}

export function FolderItem({
  folder,
  onSelect,
  onPRClick,
  onRemove,
  globalIndex,
  isFocused = false,
}: FolderItemProps) {
  const handleClick = () => {
    onSelect(folder);
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      onSelect(folder);
    }
  };

  const classNames = [
    'folder-item',
    folder.isActive ? 'folder-item--active' : '',
    isFocused ? 'folder-item--focused' : '',
  ].filter(Boolean).join(' ');

  return (
    <div
      className={classNames}
      onClick={handleClick}
      onKeyDown={handleKeyDown}
      role="treeitem"
      tabIndex={0}
      aria-selected={folder.isActive}
      data-folder-id={folder.id}
    >
      {/* Status dot */}
      <div className="folder-status-dot" />

      {/* Main content - two rows */}
      <div className="folder-content">
        {/* Row 1: Branch name + diff stats */}
        <div className="folder-row-primary">
          <span className="folder-branch">{folder.branch}</span>
          {folder.diffStats && <DiffStats stats={folder.diffStats} />}
        </div>

        {/* Row 2: Folder name + PR + hotkey */}
        <div className="folder-row-secondary">
          <span className="folder-name">{folder.name}</span>
          {folder.pullRequest && (
            <PRIndicator
              pr={folder.pullRequest}
              onClick={() => onPRClick(folder.pullRequest!)}
            />
          )}
          {globalIndex !== undefined && globalIndex < 9 && (
            <span className="hotkey-badge">⌘{globalIndex + 1}</span>
          )}
        </div>
      </div>
      {onRemove && (
        <button
          className="remove-folder-btn"
          onClick={(e) => {
            e.stopPropagation();
            onRemove(folder.id);
          }}
          title="Remove folder"
        >
          <CloseIcon size={12} />
        </button>
      )}
    </div>
  );
}
