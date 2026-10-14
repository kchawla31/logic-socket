import { ChevronDown, ChevronRight, Copy, Download, Folder, MoreHorizontal, FolderPlus, ListChecks, Network, Pencil, Plug, Plus, Radio, Search, Sparkles, Trash2 } from 'lucide-react';
import { useEffect, useMemo, useRef, useState } from 'react';

import { IconButton, useDialog, useToast } from '../components/ui';
import { api, errorText, type TreeNode } from '../lib/api';
import { cn, dropSortKey, methodColor, methodShort } from '../lib/utils';

interface Props {
  workspaceId: string;
  tree: TreeNode[];
  activeId: string | null;
  onOpen: (node: TreeNode) => void;
  onCreated: (id: string, kind: TreeNode['kind'], name: string) => void;
  onDeleted: (ids: string[]) => void;
  onRun: (targetId: string) => void;
  onImport?: () => void;
  onRenameCollection?: () => void;
  onDeleteCollection?: () => void;
}

function useExpanded(workspaceId: string) {
  const key = `lsock-expanded-${workspaceId}`;
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

function findNode(nodes: TreeNode[], id: string): TreeNode | undefined {
  for (const n of nodes) {
    if (n.id === id) return n;
    const hit = findNode(n.children, id);
    if (hit) return hit;
  }
}

function containsId(n: TreeNode, id: string): boolean {
  return n.id === id || n.children.some(c => containsId(c, id));
}

type DropPlace = 'before' | 'after' | 'inside';

function placeOf(e: { currentTarget: Element; clientY: number }, kind: string): DropPlace {
  const r = e.currentTarget.getBoundingClientRect();
  const ratio = r.height ? (e.clientY - r.top) / r.height : 0.5;
  if (kind === 'folder') {
    if (ratio < 0.28) return 'before';
    if (ratio > 0.72) return 'after';
    return 'inside';
  }
  return ratio < 0.5 ? 'before' : 'after';
}

// WKWebView fires dragend before drop and returns "" from getData(). The id lives here, not in dataTransfer.
let dragId: string | null = null;

function clearDropMark() {
  document.querySelectorAll<HTMLElement>('[data-drop-place]').forEach(el => {
    el.style.boxShadow = '';
    el.style.background = '';
    delete el.dataset.dropPlace;
  });
}

function markDrop(el: HTMLElement, place: DropPlace) {
  if (el.dataset.dropPlace === place) return;
  clearDropMark();
  el.dataset.dropPlace = place;
  el.style.boxShadow =
    place === 'before' ? 'inset 0 2px 0 0 var(--accent)' : place === 'after' ? 'inset 0 -2px 0 0 var(--accent)' : 'inset 0 0 0 1px var(--accent)';
  if (place === 'inside') el.style.background = 'var(--accent-soft)';
}

function readDragId(data: DataTransfer | null): string {
  return dragId || data?.getData('text/plain') || '';
}

interface Menu {
  x: number;
  y: number;
  node: TreeNode | null;
  /** Background click: offer rename and delete for the open collection. */
  collection?: boolean;
}

export function Sidebar({ workspaceId, tree, activeId, onOpen, onCreated, onDeleted, onRun, onImport, onRenameCollection, onDeleteCollection }: Props) {
  const toast = useToast();
  const dialog = useDialog();
  const [q, setQ] = useState('');
  const [expanded, toggle] = useExpanded(workspaceId);
  const [menu, setMenu] = useState<Menu | null>(null);
  const [renaming, setRenaming] = useState<string | null>(null);
  const visible = useMemo(() => filterTree(tree, q), [tree, q]);

  useEffect(() => {
    const close = () => setMenu(null);
    window.addEventListener('click', close);
    return () => window.removeEventListener('click', close);
  }, []);

  const create = async (kind: 'request' | 'folder' | 'mcp' | 'llm' | 'websocket' | 'socketio' | 'sse' | 'grpc', parentId: string) => {
    try {
      if (parentId !== workspaceId) toggle(parentId, true);
      if (kind === 'request') {
        const r = await api.requestCreate(parentId);
        onCreated(r.id, 'request', r.name);
      } else if (kind === 'folder') {
        const f = await api.folderCreate(parentId, 'New Folder');
        setRenaming(f.id);
      } else if (kind === 'grpc') {
        const r = await api.grpcCreate(parentId);
        onCreated(r.id, 'grpc', r.name);
      } else if (kind === 'websocket' || kind === 'socketio' || kind === 'sse') {
        const r = await api.rtCreate(parentId, kind);
        onCreated(r.id, 'realtime', r.name);
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

  // ⌘⌫ / Ctrl+Backspace deletes the selected item (not while typing in a field or editor)
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.metaKey || e.ctrlKey) || (e.key !== 'Backspace' && e.key !== 'Delete') || !activeId) return;
      const t = e.target as HTMLElement | null;
      if (t && (t.closest('input, textarea, select, [contenteditable="true"], .cm-editor'))) return;
      const find = (list: TreeNode[]): TreeNode | undefined => {
        for (const n of list) {
          if (n.id === activeId) return n;
          const hit = find(n.children);
          if (hit) return hit;
        }
      };
      const node = find(tree);
      if (!node) return;
      e.preventDefault();
      remove(node);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  });

  // Folders: top/bottom edge reorders, the middle moves inside. Other rows reorder before or after.
  const drop = async (dragId: string, target: TreeNode | null, siblings: TreeNode[], parentId: string, place: DropPlace) => {
    if (!dragId || dragId === target?.id) return;
    try {
      if (target && place === 'inside' && target.kind === 'folder') {
        const dragged = findNode(tree, dragId);
        if (dragged && containsId(dragged, target.id)) {
          toast('Cannot move a folder into itself', 'error');
          return;
        }
        const kids = target.children.filter(c => c.id !== dragId);
        const last = kids.length ? kids[kids.length - 1].sortKey : -1;
        await api.itemMove(dragId, target.id, last + 1);
        toggle(target.id, true);
        return;
      }
      const sortKey = dropSortKey(siblings, dragId, target?.id ?? null, place === 'before' ? 'before' : 'after');
      await api.itemMove(dragId, parentId, sortKey);
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
            draggable={renaming !== n.id && !q}
            onDragStart={e => {
              if (q) {
                e.preventDefault();
                return;
              }
              dragId = n.id;
              e.dataTransfer.effectAllowed = 'move';
              e.dataTransfer.setData('text/plain', n.id);
              e.stopPropagation();
            }}
            onDragEnd={() => {
              // Drop can be delivered after dragend. Keep the id until that handler has read it.
              const id = dragId;
              window.setTimeout(() => {
                if (dragId === id) dragId = null;
                clearDropMark();
              }, 50);
            }}
            onDragOver={e => {
              if (!dragId || dragId === n.id) return;
              e.preventDefault();
              e.stopPropagation();
              e.dataTransfer.dropEffect = 'move';
              markDrop(e.currentTarget, placeOf(e, n.kind));
            }}
            onDrop={e => {
              e.preventDefault();
              e.stopPropagation();
              const id = readDragId(e.dataTransfer);
              const place = placeOf(e, n.kind);
              dragId = null;
              clearDropMark();
              void drop(id, n, nodes, parentId, place);
            }}
            onClick={() => (n.kind === 'folder' ? (toggle(n.id), onOpen(n)) : onOpen(n))}
            onDoubleClick={() => setRenaming(n.id)}
            onContextMenu={e => {
              // stop here, or the tree's own handler replaces this item's menu with the "New…" menu
              e.preventDefault();
              e.stopPropagation();
              setMenu({ x: e.clientX, y: e.clientY, node: n });
            }}
            style={{ paddingLeft: 8 + depth * 14 }}
            className={cn(
              'group flex h-7 cursor-pointer items-center gap-1.5 rounded-md pr-1 text-[13px]',
              activeId === n.id ? 'bg-accent-soft text-app' : 'hover:bg-muted',
            )}
          >
            {n.kind === 'folder' ? (
              <>
                {open ? <ChevronDown className="size-3.5 shrink-0 text-muted" /> : <ChevronRight className="size-3.5 shrink-0 text-muted" />}
                <Folder className="size-3.5 shrink-0 text-muted" />
              </>
            ) : n.kind === 'mcp' ? (
              <span className="w-9 shrink-0 text-right font-mono text-[10px] font-bold text-violet-600 dark:text-violet-400">MCP</span>
            ) : n.kind === 'grpc' ? (
              <span className="w-9 shrink-0 text-right font-mono text-[10px] font-bold text-teal-600 dark:text-teal-400">gRPC</span>
            ) : n.kind === 'realtime' ? (
              <span className="w-9 shrink-0 text-right font-mono text-[10px] font-bold text-sky-600 dark:text-sky-400">
                {n.transport === 'sse' ? 'SSE' : n.transport === 'socketio' ? 'SIO' : 'WS'}
              </span>
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
            {renaming !== n.id && (
              <IconButton
                label={`More actions for ${n.name || 'item'}`}
                className={cn('size-5 group-hover:visible', menu?.node?.id === n.id ? 'visible' : 'invisible')}
                onClick={e => {
                  e.stopPropagation();
                  const r = e.currentTarget.getBoundingClientRect();
                  setMenu({ x: r.left, y: r.bottom + 4, node: n });
                }}
              >
                <MoreHorizontal className="size-3.5" />
              </IconButton>
            )}
          </div>
          {open && renderNodes(n.children, depth + 1, n.id)}
        </div>
      );
    });

  return (
    <div className="panel flex h-full min-h-0 flex-col">
      <div className="flex items-center gap-1 px-2 pt-2 pb-1.5">
        <div className="relative flex-1">
          <Search className="pointer-events-none absolute top-2 left-2 size-3.5 text-muted" />
          <input
            value={q}
            onChange={e => setQ(e.target.value)}
            placeholder="Filter"
            aria-label="Filter requests"
            className="h-7 w-full rounded-full border border-app bg-app pr-3 pl-7 text-[12.5px] outline-none focus:border-accent"
          />
        </div>
        <IconButton
          label="New…"
          onClick={e => {
            e.stopPropagation();
            const r = e.currentTarget.getBoundingClientRect();
            setMenu({ x: r.left, y: r.bottom + 4, node: null });
          }}
        >
          <Plus className="size-4" />
        </IconButton>
        <IconButton label="Run collection" onClick={() => onRun(workspaceId)}>
          <ListChecks className="size-4" />
        </IconButton>
      </div>
      <div
        role="tree"
        className="min-h-0 flex-1 overflow-auto px-1.5 pb-4"
        onDragOver={e => {
          if (!dragId) return;
          e.preventDefault();
          e.dataTransfer.dropEffect = 'move';
        }}
        onDrop={e => {
          e.preventDefault();
          const id = readDragId(e.dataTransfer);
          dragId = null;
          clearDropMark();
          void drop(id, null, tree, workspaceId, 'after');
        }}
        onContextMenu={e => {
          e.preventDefault();
          setMenu({ x: e.clientX, y: e.clientY, node: null, collection: true });
        }}
      >
        {renderNodes(visible, 0, workspaceId)}
        {visible.length === 0 && (
          <div className="p-4 text-center text-[12.5px] text-muted">
            {q ? 'No matches' : 'Nothing here yet — create a request, folder or MCP server above, or import a Postman or OpenAPI file.'}
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
              <MenuItem icon={<Network className="size-3.5" />} onClick={() => create('grpc', menu.node?.id ?? workspaceId)}>
                New gRPC request
              </MenuItem>
              <MenuItem icon={<Radio className="size-3.5" />} onClick={() => create('websocket', menu.node?.id ?? workspaceId)}>
                New WebSocket
              </MenuItem>
              <MenuItem icon={<Radio className="size-3.5" />} onClick={() => create('socketio', menu.node?.id ?? workspaceId)}>
                New Socket.IO
              </MenuItem>
              <MenuItem icon={<Radio className="size-3.5" />} onClick={() => create('sse', menu.node?.id ?? workspaceId)}>
                New event stream (SSE)
              </MenuItem>
              <MenuItem icon={<ListChecks className="size-3.5" />} onClick={() => onRun(menu.node?.id ?? workspaceId)}>
                {menu.node ? 'Run folder' : 'Run collection'}
              </MenuItem>
              {!menu.node && onImport && (
                <MenuItem icon={<Download className="size-3.5" />} onClick={onImport}>
                  Import…
                </MenuItem>
              )}
              {!menu.node && menu.collection && (onRenameCollection || onDeleteCollection) && <div className="my-1 border-t border-app" />}
              {!menu.node && menu.collection && onRenameCollection && (
                <MenuItem icon={<Pencil className="size-3.5" />} onClick={onRenameCollection}>
                  Rename collection
                </MenuItem>
              )}
              {!menu.node && menu.collection && onDeleteCollection && (
                <MenuItem icon={<Trash2 className="size-3.5" />} danger onClick={onDeleteCollection}>
                  Delete collection
                </MenuItem>
              )}
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
              <MenuItem icon={<Trash2 className="size-3.5" />} danger onClick={() => remove(menu.node!)} hint="⌘⌫">
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
  hint,
}: {
  children: React.ReactNode;
  icon: React.ReactNode;
  onClick: () => void;
  danger?: boolean;
  hint?: string;
}) {
  return (
    <button
      role="menuitem"
      onClick={onClick}
      className={cn('flex w-full items-center gap-2 px-3 py-1.5 text-left hover:bg-muted', danger && 'text-rose-600 dark:text-rose-400')}
    >
      {icon}
      {children}
      {hint && <span className="ml-auto pl-4 text-[11px] text-muted">{hint}</span>}
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
