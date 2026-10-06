import { Command as CommandIcon, Download, FileCode2, Folder, GitBranch, Layers, Upload, ListChecks, Moon, Network, Plug, Plus, Radio, Send, Settings as SettingsIcon, Sparkles, Sun, X } from 'lucide-react';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';

import { type Command, CommandPalette } from './components/CommandPalette';
import { Button, DialogProvider, Empty, IconButton, Kbd, Select, Split, ToastProvider, useDialog, useToast } from './components/ui';
import { api, type EnvList, errorText, type GitRepo, onDbChanged, type TreeNode, type Workspace } from './lib/api';
import { cn, methodColor, methodShort, modKey } from './lib/utils';
import { AiProvidersModal } from './views/AiProviders';
import { FolderView } from './views/FolderView';
import { GrpcView } from './views/GrpcView';
import { LlmView } from './views/LlmView';
import { RealtimeView } from './views/RealtimeView';
import { McpView } from './views/McpView';
import { GitModal } from './views/GitPanel';
import { CodeModal, ExportModal, ImportModal } from './views/ImportExport';
import { EnvironmentModal, SettingsModal } from './views/Modals';
import { RequestView } from './views/RequestView';
import { RunnerView } from './views/RunnerView';
import { Sidebar } from './views/Sidebar';

interface Tab {
  id: string;
  kind: TreeNode['kind'] | 'runner';
}

const RUNNER = 'runner:';

function load<T>(key: string, fallback: T): T {
  try {
    const v = localStorage.getItem(key);
    return v ? (JSON.parse(v) as T) : fallback;
  } catch {
    return fallback;
  }
}

function store(key: string, v: unknown) {
  try {
    localStorage.setItem(key, JSON.stringify(v));
  } catch {
    /* ignore */
  }
}

function flatten(nodes: TreeNode[], path: string[] = []): (TreeNode & { path: string })[] {
  return nodes.flatMap(n => [{ ...n, path: path.join(' / ') }, ...flatten(n.children, [...path, n.name])]);
}

function useTheme() {
  const [theme, setThemeState] = useState<string>(() => {
    try {
      return localStorage.getItem('lsock-theme') || 'system';
    } catch {
      return 'system';
    }
  });
  useEffect(() => {
    const apply = () => {
      const dark = theme === 'dark' || (theme === 'system' && matchMedia('(prefers-color-scheme: dark)').matches);
      document.documentElement.classList.toggle('dark', dark);
    };
    apply();
    const mq = matchMedia('(prefers-color-scheme: dark)');
    mq.addEventListener('change', apply);
    return () => mq.removeEventListener('change', apply);
  }, [theme]);
  const setTheme = (t: string) => {
    setThemeState(t);
    try {
      localStorage.setItem('lsock-theme', t);
    } catch {
      /* ignore */
    }
  };
  return [theme, setTheme] as const;
}

function Shell() {
  const toast = useToast();
  const dialog = useDialog();
  const [theme, setTheme] = useTheme();
  const [workspaces, setWorkspaces] = useState<Workspace[]>([]);
  const [wsId, setWsId] = useState<string | null>(() => load('lsock-ws', null));
  const [tree, setTree] = useState<TreeNode[]>([]);
  const [envs, setEnvs] = useState<EnvList | null>(null);
  const [tabs, setTabs] = useState<Tab[]>([]);
  const [active, setActive] = useState<string | null>(null);
  const [palette, setPalette] = useState(false);
  const [envOpen, setEnvOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [aiOpen, setAiOpen] = useState(false);
  const [importOpen, setImportOpen] = useState(false);
  const [exportOpen, setExportOpen] = useState(false);
  const [gitOpen, setGitOpen] = useState(false);
  const [codeFor, setCodeFor] = useState<string | null>(null);
  const [repos, setRepos] = useState<GitRepo[]>([]);

  // Only the newest refresh may write state (a slow one for the previous
  // workspace must not overwrite the tree after a switch).
  const refreshSeq = useRef(0);
  const refresh = useCallback(async () => {
    const seq = ++refreshSeq.current;
    const ws = await api.workspaceList();
    if (seq !== refreshSeq.current) return;
    setWorkspaces(ws);
    const current = ws.find(w => w.id === wsId) ?? ws[0];
    if (!current) return;
    if (current.id !== wsId) setWsId(current.id);
    const [t, e, r] = await Promise.all([api.treeGet(current.id), api.envList(current.id), api.gitRepoList().catch(() => [])]);
    if (seq !== refreshSeq.current) return;
    setTree(t);
    setEnvs(e);
    setRepos(r);
  }, [wsId]);

  useEffect(() => {
    refresh().catch(e => toast(errorText(e), 'error'));
    let timer: ReturnType<typeof setTimeout> | undefined;
    const un = onDbChanged(() => {
      clearTimeout(timer);
      timer = setTimeout(() => refresh().catch(() => {}), 120);
    });
    return () => {
      un.then(f => f());
    };
  }, [refresh, toast]);

  // Per-workspace tabs persistence. Only save once the workspace's tabs were restored.
  const restoredFor = useRef<string | null>(null);
  useEffect(() => {
    if (!wsId) return;
    store('lsock-ws', wsId);
    const saved = load<{ tabs: Tab[]; active: string | null }>(`lsock-tabs-${wsId}`, { tabs: [], active: null });
    setTabs(saved.tabs);
    setActive(saved.active);
    restoredFor.current = wsId;
  }, [wsId]);
  useEffect(() => {
    if (wsId && restoredFor.current === wsId) store(`lsock-tabs-${wsId}`, { tabs, active });
  }, [tabs, active, wsId]);

  const nodes = useMemo(() => flatten(tree), [tree]);
  const byId = useMemo(() => new Map(nodes.map(n => [n.id, n])), [nodes]);

  // Drop tabs whose items were deleted (once the tree has loaded).
  useEffect(() => {
    if (!nodes.length) return;
    const alive = (id: string) => byId.has(id) || (id.startsWith(RUNNER) && (id.slice(RUNNER.length) === wsId || byId.has(id.slice(RUNNER.length))));
    setTabs(ts => ts.filter(t => alive(t.id)));
    setActive(a => (a && !alive(a) ? null : a));
  }, [byId, nodes.length, wsId]);

  const open = useCallback((id: string, kind: Tab['kind']) => {
    setTabs(ts => (ts.some(t => t.id === id) ? ts : [...ts, { id, kind }]));
    setActive(id);
  }, []);

  const close = useCallback(
    (id: string) => {
      setTabs(ts => {
        const i = ts.findIndex(t => t.id === id);
        const next = ts.filter(t => t.id !== id);
        if (active === id) setActive(next[Math.min(i, next.length - 1)]?.id ?? null);
        return next;
      });
    },
    [active],
  );

  const newRequest = useCallback(async () => {
    if (!wsId) return;
    const r = await api.requestCreate(wsId);
    open(r.id, 'request');
  }, [wsId, open]);

  const newWorkspace = useCallback(async () => {
    const name = await dialog.prompt('New collection', 'My API');
    if (!name) return;
    const w = await api.workspaceCreate(name);
    setWsId(w.id);
  }, [dialog]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const mod = e.metaKey || e.ctrlKey;
      if (!mod) return;
      const k = e.key.toLowerCase();
      if (k === 'k' || (k === 'p' && e.shiftKey)) {
        e.preventDefault();
        setPalette(p => !p);
      } else if (k === 'n') {
        e.preventDefault();
        newRequest();
      } else if (k === 'e') {
        e.preventDefault();
        setEnvOpen(true);
      } else if (k === ',') {
        e.preventDefault();
        setSettingsOpen(true);
      } else if (k === 'w' && active) {
        e.preventDefault();
        close(active);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [active, close, newRequest]);

  const commands: Command[] = useMemo(() => {
    const cmds: Command[] = [
      { id: 'new-req', group: 'Actions', label: 'New HTTP request', icon: <Plus className="size-4" />, hint: <Kbd>{modKey()} N</Kbd>, run: newRequest },
      {
        id: 'new-mcp',
        group: 'Actions',
        label: 'New MCP server',
        icon: <Plug className="size-4" />,
        keywords: 'model context protocol tools',
        run: async () => wsId && open((await api.mcpServerCreate(wsId, 'New MCP Server')).id, 'mcp'),
      },
      { id: 'new-folder', group: 'Actions', label: 'New folder', icon: <Folder className="size-4" />, run: async () => wsId && (await api.folderCreate(wsId, 'New Folder')) },
      { id: 'new-ws', group: 'Actions', label: 'New collection', icon: <Layers className="size-4" />, keywords: 'workspace', run: newWorkspace },
      {
        id: 'new-ai',
        group: 'Actions',
        label: 'New AI request',
        icon: <Sparkles className="size-4" />,
        keywords: 'llm claude gpt chat agent mcp tools',
        run: async () => wsId && open((await api.llmRequestCreate(wsId, 'New AI Request')).id, 'llm'),
      },
      ...(['websocket', 'socketio', 'sse'] as const).map(k => ({
        id: `new-rt-${k}`,
        group: 'Actions',
        label: `New ${k === 'websocket' ? 'WebSocket' : k === 'socketio' ? 'Socket.IO connection' : 'event stream (SSE)'}`,
        icon: <Radio className="size-4" />,
        keywords: 'realtime ws socket stream',
        run: async () => wsId && open((await api.rtCreate(wsId, k)).id, 'realtime'),
      })),
      {
        id: 'new-grpc',
        group: 'Actions',
        label: 'New gRPC request',
        icon: <Network className="size-4" />,
        keywords: 'grpc protobuf proto rpc',
        run: async () => wsId && open((await api.grpcCreate(wsId)).id, 'grpc'),
      },
      { id: 'import', group: 'Actions', label: 'Import…', icon: <Download className="size-4" />, keywords: 'postman openapi swagger har curl yaml json', run: () => setImportOpen(true) },
      { id: 'export', group: 'Actions', label: 'Export collection…', icon: <Upload className="size-4" />, keywords: 'postman har yaml json backup', run: () => setExportOpen(true) },
      { id: 'git', group: 'Actions', label: 'Git sync', icon: <GitBranch className="size-4" />, keywords: 'commit pull push branch repository github gitlab', run: () => setGitOpen(true) },
      ...(active && byId.get(active)?.kind === 'request'
        ? [{ id: 'code', group: 'Actions', label: 'Generate code for this request', icon: <FileCode2 className="size-4" />, keywords: 'curl python javascript fetch go rust httpie snippet', run: () => setCodeFor(active) }]
        : []),
      { id: 'ai-providers', group: 'Actions', label: 'AI providers', icon: <Sparkles className="size-4" />, keywords: 'llm api key anthropic openai ollama', run: () => setAiOpen(true) },
      {
        id: 'run-collection',
        group: 'Actions',
        label: 'Run collection',
        icon: <ListChecks className="size-4" />,
        keywords: 'runner tests iterations data',
        run: () => wsId && open(`${RUNNER}${wsId}`, 'runner'),
      },
      { id: 'envs', group: 'Actions', label: 'Manage environments', icon: <Layers className="size-4" />, hint: <Kbd>{modKey()} E</Kbd>, keywords: 'variables', run: () => setEnvOpen(true) },
      { id: 'settings', group: 'Actions', label: 'Settings', icon: <SettingsIcon className="size-4" />, hint: <Kbd>{modKey()} ,</Kbd>, run: () => setSettingsOpen(true) },
      {
        id: 'theme',
        group: 'Actions',
        label: `Switch to ${document.documentElement.classList.contains('dark') ? 'light' : 'dark'} theme`,
        icon: <Moon className="size-4" />,
        run: () => setTheme(document.documentElement.classList.contains('dark') ? 'light' : 'dark'),
      },
    ];
    for (const e of envs?.subs ?? [])
      cmds.push({
        id: `env-${e.id}`,
        group: 'Environment',
        label: `Use environment: ${e.name}`,
        icon: <Layers className="size-4" />,
        run: () => wsId && api.envSetActive(wsId, e.id),
      });
    for (const w of workspaces)
      if (w.id !== wsId) cmds.push({ id: `ws-${w.id}`, group: 'Collections', label: `Open ${w.name}`, icon: <Layers className="size-4" />, run: () => setWsId(w.id) });
    for (const n of nodes)
      cmds.push({
        id: `go-${n.id}`,
        group: 'Go to',
        label: n.name || 'Untitled',
        keywords: `${n.path} ${n.method ?? ''}`,
        hint: n.path,
        icon:
          n.kind === 'request' ? (
            <span className={cn('font-mono text-[9.5px] font-bold', methodColor(n.method))}>{methodShort(n.method)}</span>
          ) : n.kind === 'mcp' ? (
            <Plug className="size-4" />
          ) : n.kind === 'llm' ? (
            <Sparkles className="size-4" />
          ) : n.kind === 'realtime' ? (
            <Radio className="size-4" />
          ) : n.kind === 'grpc' ? (
            <Network className="size-4" />
          ) : (
            <Folder className="size-4" />
          ),
        run: () => open(n.id, n.kind),
      });
    return cmds;
  }, [nodes, envs, workspaces, wsId, newRequest, newWorkspace, open, setTheme, active, byId]);

  const ws = workspaces.find(w => w.id === wsId);
  const activeTab = tabs.find(t => t.id === active);

  return (
    <div className="flex h-full flex-col">
      {/* top bar */}
      <header data-tauri-drag-region className="flex h-11 shrink-0 items-center gap-2 border-b border-app bg-subtle px-3">
        <div className="flex items-center gap-1.5 pr-2 font-semibold tracking-tight">
          <span className="flex size-5 items-center justify-center rounded-md bg-accent text-[10px] font-bold text-white">LS</span>
          Logic Socket
        </div>
        <Select aria-label="Collection" value={wsId ?? ''} onChange={e => {
            const v = e.target.value;
            if (v === '__new') newWorkspace();
            else if (v === '__import') setImportOpen(true);
            else if (v === '__export') setExportOpen(true);
            else setWsId(v);
          }} className="h-7 max-w-52 text-[12.5px]">
          {workspaces.map(w => (
            <option key={w.id} value={w.id}>
              {w.name}
            </option>
          ))}
          <option value="__new">+ New collection…</option>
          <option value="__import">↓ Import…</option>
          <option value="__export">↑ Export this collection…</option>
        </Select>
        <Select
          aria-label="Environment"
          value={envs?.activeId ?? ''}
          onChange={e => wsId && api.envSetActive(wsId, e.target.value || null)}
          className="h-7 max-w-48 text-[12.5px]"
        >
          <option value="">No environment</option>
          {envs?.subs.map(e => (
            <option key={e.id} value={e.id}>
              {e.name}
            </option>
          ))}
        </Select>
        <Button size="sm" variant="ghost" onClick={() => setEnvOpen(true)} title={`Manage environments (${modKey()}+E)`}>
          <Layers className="size-3.5" /> Environments
        </Button>
        <div data-tauri-drag-region className="flex-1" />
        {(() => {
          const repo = repos.find(r => r.files.some(f => f.workspaceId === wsId));
          return (
            <Button size="sm" variant="ghost" onClick={() => setGitOpen(true)} title="Git sync">
              <GitBranch className="size-3.5" /> {repo ? repo.name : 'Git'}
            </Button>
          );
        })()}
        <button
          onClick={() => setPalette(true)}
          className="flex h-7 w-64 items-center gap-2 rounded-md border border-app bg-app px-2.5 text-[12.5px] text-muted hover:border-accent"
        >
          <CommandIcon className="size-3.5" /> Search or run a command
          <span className="flex-1" />
          <Kbd>{modKey()} K</Kbd>
        </button>
        <IconButton label="Toggle theme" onClick={() => setTheme(document.documentElement.classList.contains('dark') ? 'light' : 'dark')}>
          {theme === 'dark' || (theme === 'system' && document.documentElement.classList.contains('dark')) ? <Sun className="size-4" /> : <Moon className="size-4" />}
        </IconButton>
        <IconButton label="AI providers" onClick={() => setAiOpen(true)}>
          <Sparkles className="size-4" />
        </IconButton>
        <IconButton label="Settings" onClick={() => setSettingsOpen(true)}>
          <SettingsIcon className="size-4" />
        </IconButton>
      </header>

      {wsId ? (
        <Split direction="row" initial={270} min={200} storageKey="lsock-split-sidebar">
          <Sidebar
            workspaceId={wsId}
            tree={tree}
            activeId={active}
            onOpen={n => open(n.id, n.kind)}
            onCreated={(id, kind) => open(id, kind)}
            onDeleted={ids => ids.forEach(close)}
            onRun={targetId => open(`${RUNNER}${targetId}`, 'runner')}
            onImport={() => setImportOpen(true)}
          />
          <main className="flex h-full min-h-0 flex-col">
            {tabs.length > 0 && (
              <div className="flex h-9 shrink-0 items-end gap-0.5 overflow-x-auto border-b border-app bg-subtle px-1.5">
                {tabs.map(t => {
                  const n = byId.get(t.id);
                  const runnerTarget = t.kind === 'runner' ? t.id.slice(RUNNER.length) : null;
                  const label = runnerTarget ? `Run: ${runnerTarget === wsId ? (ws?.name ?? 'Collection') : (byId.get(runnerTarget)?.name ?? 'Folder')}` : n?.name || 'Untitled';
                  return (
                    <div
                      key={t.id}
                      onClick={() => setActive(t.id)}
                      onAuxClick={e => e.button === 1 && close(t.id)}
                      className={cn(
                        'group -mb-px flex h-8 max-w-52 min-w-0 cursor-pointer items-center gap-1.5 rounded-t-md border border-b-0 px-2.5 text-[12.5px]',
                        active === t.id ? 'border-app bg-app' : 'border-transparent text-muted hover:text-app',
                      )}
                    >
                      {t.kind === 'request' && <span className={cn('font-mono text-[9.5px] font-bold', methodColor(n?.method))}>{methodShort(n?.method)}</span>}
                      {t.kind === 'mcp' && <Plug className="size-3.5 shrink-0 text-violet-500" />}
                      {t.kind === 'folder' && <Folder className="size-3.5 shrink-0 text-muted" />}
                      {t.kind === 'runner' && <ListChecks className="size-3.5 shrink-0 text-emerald-500" />}
                      {t.kind === 'llm' && <Sparkles className="size-3.5 shrink-0 text-accent" />}
                      {t.kind === 'realtime' && <Radio className="size-3.5 shrink-0 text-sky-500" />}
                      {t.kind === 'grpc' && <Network className="size-3.5 shrink-0 text-teal-500" />}
                      <span className="truncate">{label}</span>
                      <button
                        aria-label="Close tab"
                        onClick={e => (e.stopPropagation(), close(t.id))}
                        className="invisible rounded p-0.5 group-hover:visible hover:bg-muted"
                      >
                        <X className="size-3" />
                      </button>
                    </div>
                  );
                })}
              </div>
            )}
            <div className="flex min-h-0 flex-1 flex-col">
              {activeTab?.kind === 'request' && <RequestView key={activeTab.id} id={activeTab.id} onRenamed={() => refresh()} />}
              {activeTab?.kind === 'folder' && <FolderView key={activeTab.id} id={activeTab.id} />}
              {activeTab?.kind === 'mcp' && <McpView key={activeTab.id} id={activeTab.id} />}
              {activeTab?.kind === 'realtime' && <RealtimeView key={activeTab.id} id={activeTab.id} />}
              {activeTab?.kind === 'grpc' && wsId && <GrpcView key={activeTab.id} id={activeTab.id} workspaceId={wsId} />}
              {activeTab?.kind === 'llm' && <LlmView key={activeTab.id} id={activeTab.id} tree={tree} onOpenProviders={() => setAiOpen(true)} />}
              {activeTab?.kind === 'runner' && wsId && (
                <RunnerView
                  key={activeTab.id}
                  targetId={activeTab.id.slice(RUNNER.length)}
                  workspaceId={wsId}
                  workspaceName={ws?.name ?? 'Collection'}
                  tree={tree}
                />
              )}
              {!activeTab && (
                <Empty icon={<Send className="size-9" />} title={ws ? ws.name : 'Welcome'}>
                  <div className="flex flex-col items-center gap-3">
                    <span>Open a request from the sidebar, or start something new.</span>
                    <div className="flex gap-2">
                      <Button variant="primary" onClick={newRequest}>
                        <Plus className="size-3.5" /> HTTP request
                      </Button>
                      <Button onClick={async () => wsId && open((await api.mcpServerCreate(wsId, 'New MCP Server')).id, 'mcp')}>
                        <Plug className="size-3.5" /> MCP server
                      </Button>
                      <Button onClick={async () => wsId && open((await api.llmRequestCreate(wsId, 'New AI Request')).id, 'llm')}>
                        <Sparkles className="size-3.5" /> AI request
                      </Button>
                      <Button onClick={() => setImportOpen(true)}>
                        <Download className="size-3.5" /> Import
                      </Button>
                    </div>
                    <span className="text-[12px]">
                      <Kbd>{modKey()} K</Kbd> command palette · <Kbd>{modKey()} N</Kbd> new request · <Kbd>{modKey()} E</Kbd> environments · paste a cURL into the URL bar
                    </span>
                  </div>
                </Empty>
              )}
            </div>
          </main>
        </Split>
      ) : (
        <Empty title="Loading…" />
      )}

      <CommandPalette open={palette} onClose={() => setPalette(false)} commands={commands} />
      {wsId && <EnvironmentModal open={envOpen} onClose={() => setEnvOpen(false)} workspaceId={wsId} />}
      <AiProvidersModal open={aiOpen} onClose={() => setAiOpen(false)} />
      <ImportModal open={importOpen} onClose={() => setImportOpen(false)} workspaceId={wsId} workspaceName={ws?.name} onImported={id => setWsId(id)} />
      {wsId && <ExportModal open={exportOpen} onClose={() => setExportOpen(false)} workspaceId={wsId} workspaceName={ws?.name ?? 'Collection'} />}
      {wsId && <GitModal open={gitOpen} onClose={() => (setGitOpen(false), refresh())} workspaceId={wsId} workspaceName={ws?.name ?? 'Collection'} onOpenWorkspace={setWsId} />}
      {codeFor && <CodeModal open onClose={() => setCodeFor(null)} requestId={codeFor} requestName={byId.get(codeFor)?.name ?? 'request'} />}
      <SettingsModal open={settingsOpen} onClose={() => setSettingsOpen(false)} theme={theme} setTheme={setTheme} />
    </div>
  );
}

export function App() {
  return (
    <ToastProvider>
      <DialogProvider>
        <Shell />
      </DialogProvider>
    </ToastProvider>
  );
}
