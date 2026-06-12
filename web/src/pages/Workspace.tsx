import { useState, useEffect } from 'react';
import {
  Folder,
  FolderOpen,
  FileText,
  ChevronRight,
  ChevronDown,
  X,
} from 'lucide-react';
import type { WorkspaceFileNode, WorkspaceTree, WorkspaceFileContent } from '@/types/api';
import { getWorkspaceFiles, getWorkspaceFile } from '@/lib/api';

const VIEWABLE = new Set(['md', 'json', 'jsonl']);

function ext(name: string): string {
  return name.split('.').pop()?.toLowerCase() ?? '';
}

interface TreeNodeProps {
  node: WorkspaceFileNode;
  onOpen: (node: WorkspaceFileNode) => void;
  depth: number;
}

function TreeNode({ node, onOpen, depth }: TreeNodeProps) {
  const [open, setOpen] = useState(depth === 0);
  const isDir = node.kind === 'dir';
  const fileExt = isDir ? '' : ext(node.name);
  const canView = !isDir && VIEWABLE.has(fileExt);

  if (isDir) {
    return (
      <div>
        <button
          type="button"
          onClick={() => setOpen((o) => !o)}
          className="flex w-full items-center gap-1.5 rounded px-2 py-1 text-left text-sm text-[#9bb7eb] hover:bg-[#0b1f4a] hover:text-white"
          style={{ paddingLeft: `${8 + depth * 16}px` }}
        >
          {open ? (
            <ChevronDown className="h-3.5 w-3.5 shrink-0 text-[#5f84cc]" />
          ) : (
            <ChevronRight className="h-3.5 w-3.5 shrink-0 text-[#5f84cc]" />
          )}
          {open ? (
            <FolderOpen className="h-4 w-4 shrink-0 text-yellow-400" />
          ) : (
            <Folder className="h-4 w-4 shrink-0 text-yellow-400" />
          )}
          <span className="truncate">{node.name}</span>
        </button>
        {open && node.children && node.children.length > 0 && (
          <div>
            {node.children.map((child) => (
              <TreeNode key={child.path} node={child} onOpen={onOpen} depth={depth + 1} />
            ))}
          </div>
        )}
      </div>
    );
  }

  return (
    <button
      type="button"
      onClick={() => canView && onOpen(node)}
      disabled={!canView}
      className={[
        'flex w-full items-center gap-1.5 rounded px-2 py-1 text-left text-sm',
        canView
          ? 'cursor-pointer text-[#9bb7eb] hover:bg-[#0b1f4a] hover:text-white'
          : 'cursor-default text-[#4a5c7a]',
      ].join(' ')}
      style={{ paddingLeft: `${8 + depth * 16}px` }}
      title={canView ? `View ${node.name}` : `${node.name} (not viewable)`}
    >
      <span className="h-3.5 w-3.5 shrink-0" />
      <FileText
        className={['h-4 w-4 shrink-0', canView ? 'text-blue-400' : 'text-[#4a5c7a]'].join(' ')}
      />
      <span className="truncate">{node.name}</span>
      {canView && (
        <span className="ml-auto shrink-0 rounded bg-[#0f2151] px-1.5 py-0.5 text-[10px] uppercase tracking-wider text-[#5f84cc]">
          {fileExt}
        </span>
      )}
    </button>
  );
}

function renderContent(content: string, fileExt: string) {
  if (fileExt === 'md') {
    return (
      <pre className="whitespace-pre-wrap font-sans text-sm leading-relaxed text-[#c8d8f0]">
        {content}
      </pre>
    );
  }
  if (fileExt === 'json') {
    let formatted = content;
    try {
      formatted = JSON.stringify(JSON.parse(content), null, 2);
    } catch {
      // show raw if parse fails
    }
    return (
      <pre className="whitespace-pre-wrap font-mono text-sm leading-relaxed text-[#c8d8f0]">
        {formatted}
      </pre>
    );
  }
  // jsonl — one JSON object per line
  return (
    <div className="space-y-1">
      {content
        .split('\n')
        .filter(Boolean)
        .map((line, i) => {
          let display = line;
          try {
            display = JSON.stringify(JSON.parse(line), null, 2);
          } catch {
            // keep raw line
          }
          return (
            <details key={i} className="rounded border border-[#1b3670] bg-[#070f27]">
              <summary className="cursor-pointer px-3 py-1.5 font-mono text-xs text-[#5f84cc] hover:text-white">
                Line {i + 1}
              </summary>
              <pre className="whitespace-pre-wrap px-3 pb-2 font-mono text-xs leading-relaxed text-[#c8d8f0]">
                {display}
              </pre>
            </details>
          );
        })}
    </div>
  );
}

export default function Workspace() {
  const [treeData, setTreeData] = useState<WorkspaceTree | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const [activeFile, setActiveFile] = useState<WorkspaceFileContent | null>(null);
  const [fileLoading, setFileLoading] = useState(false);
  const [fileError, setFileError] = useState<string | null>(null);

  useEffect(() => {
    getWorkspaceFiles()
      .then(setTreeData)
      .catch((e) => setError(e.message))
      .finally(() => setLoading(false));
  }, []);

  function openFile(node: WorkspaceFileNode) {
    setFileLoading(true);
    setFileError(null);
    setActiveFile(null);
    getWorkspaceFile(node.path)
      .then(setActiveFile)
      .catch((e) => setFileError(e.message))
      .finally(() => setFileLoading(false));
  }

  return (
    <div className="flex h-[calc(100vh-4rem)] flex-col p-6">
      <div className="mb-4 flex items-center justify-between">
        <div>
          <h1 className="text-xl font-semibold text-white">Workspace</h1>
          {treeData && (
            <p className="mt-0.5 font-mono text-xs text-[#5f84cc]">{treeData.workspace}</p>
          )}
        </div>
      </div>

      {error && (
        <div className="rounded-xl border border-rose-800/60 bg-rose-900/20 px-4 py-3 text-sm text-rose-300">
          {error}
        </div>
      )}

      {loading && (
        <div className="flex items-center justify-center py-20">
          <div className="electric-loader h-8 w-8 rounded-full" />
        </div>
      )}

      {!loading && !error && treeData && (
        <div className="flex min-h-0 flex-1 gap-4">
          {/* Tree panel */}
          <div className="flex w-64 shrink-0 flex-col overflow-hidden rounded-xl border border-[#1e2f5d] bg-[#050b1a]/95">
            <div className="border-b border-[#1e2f5d] px-3 py-2.5 text-xs font-medium uppercase tracking-wider text-[#5f84cc]">
              Files
            </div>
            <div className="flex-1 overflow-y-auto p-1.5">
              {treeData.tree.length === 0 ? (
                <p className="px-3 py-4 text-sm text-[#4a5c7a]">Workspace is empty</p>
              ) : (
                treeData.tree.map((node) => (
                  <TreeNode key={node.path} node={node} onOpen={openFile} depth={0} />
                ))
              )}
            </div>
          </div>

          {/* Viewer panel */}
          <div className="flex min-w-0 flex-1 flex-col overflow-hidden rounded-xl border border-[#1e2f5d] bg-[#050b1a]/95">
            {!activeFile && !fileLoading && !fileError && (
              <div className="flex flex-1 items-center justify-center text-sm text-[#4a5c7a]">
                Select a .md, .json, or .jsonl file to view it
              </div>
            )}

            {fileLoading && (
              <div className="flex flex-1 items-center justify-center">
                <div className="electric-loader h-6 w-6 rounded-full" />
              </div>
            )}

            {fileError && (
              <div className="m-4 rounded-xl border border-rose-800/60 bg-rose-900/20 px-4 py-3 text-sm text-rose-300">
                {fileError}
              </div>
            )}

            {activeFile && (
              <>
                <div className="flex items-center justify-between border-b border-[#1e2f5d] px-4 py-2.5">
                  <span className="font-mono text-sm text-[#9bb7eb]">{activeFile.path}</span>
                  <button
                    type="button"
                    onClick={() => setActiveFile(null)}
                    className="rounded p-1 text-[#5f84cc] hover:bg-[#0b1f4a] hover:text-white"
                    aria-label="Close file"
                  >
                    <X className="h-4 w-4" />
                  </button>
                </div>
                <div className="flex-1 overflow-auto p-4">
                  {renderContent(activeFile.content, activeFile.ext)}
                </div>
              </>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
