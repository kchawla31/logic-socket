import { ChevronDown, ChevronRight, Copy, Folder, FolderPlus, ListChecks, Pencil, Plug, Plus, Search, Sparkles, Trash2 } from 'lucide-react';
import { useEffect, useMemo, useRef, useState } from 'react';

import { IconButton, useDialog, useToast } from '../components/ui';
import { api, errorText, type TreeNode } from '../lib/api';
import { cn, methodColor, methodShort } from '../lib/utils';

interface Props {
  workspaceId: string;
  tree: TreeNode[];
  activeId: string | null;
  onOpen: (node: TreeNode) => void;
  onCreated: (id: string, kind: TreeNode['kind'], name: string) => void;
  onDeleted: (ids: string[]) => void;
  onRun: (targetId: string) => void;
}

function useExpanded(workspaceId: string) {
  const key = `irs-expanded-${workspaceId}`;
  const [set, setSet] = useState<Set<string>>(() => {
    try {
      return new Set(JSON.parse(localStorage.getItem(key) || '[]'));
    } catch {
      return new Set();
    }
  });
  const toggle = (id: string, open?: boolean) =>
    setSet(prev => {
      const next = new Set(prev);
      if (open ?? !next.has(id)) next.add(id);
      else next.delete(id);
      try {
        localStorage.setItem(key, JSON.stringify([...next]));
      } catch {
        /* ignore */
      }
      return next;
    });
  return [set, toggle] as const;
}

function filterTree(nodes: TreeNode[], q: string): TreeNode[] {
  if (!q) return nodes;
  const ql = q.toLowerCase();
  return nodes.flatMap(n => {
    const kids = filterTree(n.children, q);
    if (n.name.toLowerCase().includes(ql) || kids.length) return [{ ...n, children: kids.length ? kids : n.children }];
    return [];
  });
}

function collectIds(n: TreeNode): string[] {
  return [n.id, ...n.children.flatMap(collectIds)];
}

interface Menu {
  x: number;
  y: number;
  node: TreeNode | null;
}

export function Sidebar({ workspaceId, tree, activeId, onOpen, onCreated, onDeleted, onRun }: Props) {
  const toast = useToast();
  const dialog = useDialog();
  const [q, setQ] = useState('');
  const [expanded, toggle] = useExpanded(workspaceId);
  const [menu, setMenu] = useState<Menu | null>(null);
  const [renaming, setRenaming] = useState<string | null>(null);
  const [dragOver, setDragOver] = useState<string | null>(null);
  const visible = useMemo(() => filterTree(tree, q), [tree, q]);

  useEffect(() => {
    const close = () => setMenu(null);
    window.addEventListener('click', close);
    return () => window.removeEventListener('click', close);
  }, []);

  const create = async (kind: 'request' | 'folder' | 'mcp' | 'llm', parentId: string) => {
    try {
      if (parentId !== workspaceId) toggle(parentId, true);
      if (kind === 'request') {
        const r = await api.requestCreate(parentId);
        onCreated(r.id, 'request', r.name);
      } else if (kind === 'folder') {
        const f = await api.folderCreate(parentId, 'New Folder');
        setRenaming(f.id);
      } else if (kind === 'llm') {
        const r = await api.llmRequestCreate(parentId, 'New AI Request');
        onCreated(r.id, 'llm', r.name);
      } else {
        const s = await api.mcpServerCreate(parentId, 'New MCP Server');
        onCreated(s.id, 'mcp', s.name);
      }
    } catch (e) {
      toast(errorText(e), 'error');
    }
  };

  const remove = async (n: TreeNode) => {
    const what = n.kind === 'folder' ? `folder "${n.name}" and everything in it` : `"${n.name}"`;
    if (!(await dialog.confirm(`Delete ${what}?`, { danger: true, confirmLabel: 'Delete', message: 'This cannot be undone.' }))) return;
    try {
      await api.itemDelete(n.id);
      onDeleted(collectIds(n));
    } catch (e) {
      toast(errorText(e), 'error');
    }
  };

  // Drop onto folder = move inside (end); onto request/server = place before it.
  const drop = async (dragId: string, target: TreeNode | null, siblings: TreeNode[], parentId: string) => {
    if (!dragId || dragId === target?.id) return;
    try {
      if (!target) {
        await api.itemMove(dragId, workspaceId, Date.now());
      } else if (target.kind === 'folder') {
        await api.itemMove(dragId, target.id, Date.now());
        toggle(target.id, true);
      } else {
        const i = siblings.findIndex(s => s.id === target.id);
        const prev = i > 0 ? siblings[i - 1].sortKey : target.sortKey - 1;
        await api.itemMove(dragId, parentId, (prev + target.sortKey) / 2);
      }
    } catch (e) {
      toast(errorText(e), 'error');
    }
  };

  const renderNodes = (nodes: TreeNode[], depth: number, parentId: string) =>
    nodes.map(n => {
      const open = n.kind === 'folder' && (expanded.has(n.id) || !!q);
      return (
        <div key={n.id}>
          <div
            role="treeitem"
            aria-selected={activeId === n.id}
            aria-expanded={n.kind === 'folder' ? open : undefined}
            draggable={renaming !== n.id}
            onDragStart={e => e.dataTransfer.setData('text/irs-id', n.id)}
            onDragOver={e => {
              e.preventDefault();
              setDragOver(n.id);
            }}
            onDragLeave={() => setDragOver(d => (d === n.id ? null : d))}
            onDrop={e => {
              e.preventDefault();
              e.stopPropagation();
              setDragOver(null);
              drop(e.dataTransfer.getData('text/irs-id'), n, nodes, parentId);
            }}
            onClick={() => (n.kind === 'folder' ? (toggle(n.id), onOpen(n)) : onOpen(n))}
            onDoubleClick={() => setRenaming(n.id)}
            onContextMenu={e => {
              e.preventDefault();
              setMenu({ x: e.clientX, y: e.clientY, node: n });
            }}
            style={{ paddingLeft: 8 + depth * 14 }}
            className={cn(
              'group flex h-7 cursor-pointer items-center gap-1.5 rounded-md pr-1 text-[13px]',
              activeId === n.id ? 'bg-accent-soft text-app' : 'hover:bg-muted',
              dragOver === n.id && (n.kind === 'folder' ? 'ring-1 ring-accent' : 'border-t-2 border-accent'),
            )}
          >
            {n.kind === 'folder' ? (
              <>
                {open ? <ChevronDown className="size-3.5 shrink-0 text-muted" /> : <ChevronRight className="size-3.5 shrink-0 text-muted" />}
                <Folder className="size-3.5 shrink-0 text-muted" />
              </>
            ) : n.kind === 'mcp' ? (
              <span className="w-9 shrink-0 text-right font-mono text-[10px] font-bold text-violet-600 dark:text-violet-400">MCP</span>
            ) : n.kind === 'llm' ? (
              <span className="w-9 shrink-0 text-right font-mono text-[10px] font-bold text-fuchsia-600 dark:text-fuchsia-400">AI</span>
            ) : (
              <span className={cn('w-9 shrink-0 text-right font-mono text-[10px] font-bold', methodColor(n.method))}>
                {methodShort(n.method)}
              </span>
            )}
            {renaming === n.id ? (
              <RenameInput
                initial={n.name}
                onDone={async name => {
                  setRenaming(null);
                  if (name && name !== n.name) await api.itemRename(n.id, name).catch(e => toast(errorText(e), 'error'));
                }}
              />
            ) : (
              <span className="min-w-0 flex-1 truncate">{n.name || <span className="text-muted italic">Untitled</span>}</span>
            )}
            {n.kind === 'folder' && renaming !== n.id && (
              <IconButton
                label="New request in folder"
                className="invisible size-5 group-hover:visible"
                onClick={e => {
                  e.stopPropagation();
                  create('request', n.id);
                }}
              >
                <Plus className="size-3.5" />
              </IconButton>
            )}
          </div>
          {open && renderNodes(n.children, depth + 1, n.id)}
        </div>
      );
    });

  return (
    <div className="flex h-full min-h-0 flex-col bg-subtle">
      <div className="flex items-center gap-1 px-2 pt-2 pb-1.5">
        <div className="relative flex-1">
          <Search className="pointer-events-none absolute top-2 left-2 size-3.5 text-muted" />
          <input
            value={q}
            onChange={e => setQ(e.target.value)}
            placeholder="Filter"
            aria-label="Filter requests"
            className="h-7 w-full rounded-md border border-app bg-app pr-2 pl-7 text-[12.5px] outline-none focus:border-accent"
          />
        </div>
        <IconButton label="New request" onClick={() => create('request', workspaceId)}>
          <Plus className="size-4" />
        </IconButton>
        <IconButton label="New folder" onClick={() => create('folder', workspaceId)}>
          <FolderPlus className="size-4" />
        </IconButton>
        <IconButton label="New MCP server" onClick={() => create('mcp', workspaceId)}>
          <Plug className="size-4" />
        </IconButton>
        <IconButton label="New AI request" onClick={() => create('llm', workspaceId)}>
          <Sparkles className="size-4" />
        </IconButton>
        <IconButton label="Run collection" onClick={() => onRun(workspaceId)}>
          <ListChecks className="size-4" />
        </IconButton>
      </div>
      <div
        role="tree"
        className="min-h-0 flex-1 overflow-auto px-1.5 pb-4"
        onDragOver={e => e.preventDefault()}
        onDrop={e => {
          e.preventDefault();
          drop(e.dataTransfer.getData('text/irs-id'), null, tree, workspaceId);
        }}
        onContextMenu={e => {
          e.preventDefault();
          setMenu({ x: e.clientX, y: e.clientY, node: null });
        }}
      >
        {renderNodes(visible, 0, workspaceId)}
        {visible.length === 0 && (
          <div className="p-4 text-center text-[12.5px] text-muted">
            {q ? 'No matches' : 'Nothing here yet — create a request, folder or MCP server above.'}
          </div>
        )}
      </div>
      {menu && (
        <div
          role="menu"
          style={{ left: menu.x, top: menu.y }}
          className="fixed z-50 min-w-44 rounded-lg border border-app bg-app py-1 text-[13px] shadow-xl"
        >
          {(menu.node === null || menu.node.kind === 'folder') && (
            <>
              <MenuItem icon={<Plus className="size-3.5" />} onClick={() => create('request', menu.node?.id ?? workspaceId)}>
                New request
              </MenuItem>
              <MenuItem icon={<FolderPlus className="size-3.5" />} onClick={() => create('folder', menu.node?.id ?? workspaceId)}>
                New folder
              </MenuItem>
              <MenuItem icon={<Plug className="size-3.5" />} onClick={() => create('mcp', menu.node?.id ?? workspaceId)}>
                New MCP server
              </MenuItem>
              <MenuItem icon={<Sparkles className="size-3.5" />} onClick={() => create('llm', menu.node?.id ?? workspaceId)}>
                New AI request
              </MenuItem>
              <MenuItem icon={<ListChecks className="size-3.5" />} onClick={() => onRun(menu.node?.id ?? workspaceId)}>
                {menu.node ? 'Run folder' : 'Run collection'}
              </MenuItem>
            </>
          )}
          {menu.node && (
            <>
              {menu.node.kind === 'folder' && <div className="my-1 border-t border-app" />}
              <MenuItem icon={<Pencil className="size-3.5" />} onClick={() => setRenaming(menu.node!.id)}>
                Rename
              </MenuItem>
              <MenuItem
                icon={<Copy className="size-3.5" />}
                onClick={() => api.itemDuplicate(menu.node!.id).catch(e => toast(errorText(e), 'error'))}
              >
                Duplicate
              </MenuItem>
              <MenuItem icon={<Trash2 className="size-3.5" />} danger onClick={() => remove(menu.node!)}>
                Delete
              </MenuItem>
            </>
          )}
        </div>
      )}
    </div>
  );
}

function MenuItem({
  children,
  icon,
  onClick,
  danger,
}: {
  children: React.ReactNode;
  icon: React.ReactNode;
  onClick: () => void;
  danger?: boolean;
}) {
  return (
    <button
      role="menuitem"
      onClick={onClick}
      className={cn('flex w-full items-center gap-2 px-3 py-1.5 text-left hover:bg-muted', danger && 'text-rose-600 dark:text-rose-400')}
    >
      {icon}
      {children}
    </button>
  );
}

function RenameInput({ initial, onDone }: { initial: string; onDone: (name: string) => void }) {
  const [v, setV] = useState(initial);
  const ref = useRef<HTMLInputElement>(null);
  useEffect(() => {
    ref.current?.select();
  }, []);
  return (
    <input
      ref={ref}
      autoFocus
      value={v}
      onClick={e => e.stopPropagation()}
      onChange={e => setV(e.target.value)}
      onBlur={() => onDone(v.trim())}
      onKeyDown={e => {
        if (e.key === 'Enter') onDone(v.trim());
        if (e.key === 'Escape') onDone(initial);
      }}
      className="h-6 min-w-0 flex-1 rounded border border-accent bg-app px-1.5 text-[13px] outline-none"
    />
  );
}
