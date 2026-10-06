import { ArrowDownLeft, ArrowUpRight, ChevronDown, ChevronRight, Circle, Info, Plug, Power, RefreshCw, Trash2, TriangleAlert, Zap } from 'lucide-react';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';

import { CodeEditor, JsonTree, KeyValueEditor, Markdown } from '../components/editors';
import { Badge, Button, CopyButton, Empty, Input, Select, Split, Tabs, useToast } from '../components/ui';
import { VarInput } from '../components/VarInput';
import {
  api,
  errorText,
  type Listed,
  type LogEntry,
  type McpServer,
  type McpStatus,
  type Notification,
  onMcpLog,
  type Prompt,
  type Resource,
  type ResourceTemplate,
  type ToolView,
} from '../lib/api';
import { clockTime, cn, formatMs } from '../lib/utils';
import { useProviders } from './AiProviders';
import { useAutosave } from './RequestView';
import { ToolsPanel } from './McpTools';

/** Shell-like split honoring quotes, for the stdio command line. */
export function splitArgs(s: string): string[] {
  const out: string[] = [];
  let cur = '';
  let quote: string | null = null;
  let any = false;
  for (const c of s) {
    if (quote) {
      if (c === quote) quote = null;
      else cur += c;
    } else if (c === '"' || c === "'") {
      quote = c;
      any = true;
    } else if (/\s/.test(c)) {
      if (cur || any) out.push(cur);
      cur = '';
      any = false;
    } else cur += c;
  }
  if (cur || any) out.push(cur);
  return out;
}

function joinArgs(a: string[]): string {
  return a.map(x => (/\s|^$/.test(x) ? `"${x}"` : x)).join(' ');
}

export function McpView({ id }: { id: string }) {
  const toast = useToast();
  const [server, setServer] = useAutosave<McpServer>(id, api.mcpServerUpdate);
  const [status, setStatus] = useState<McpStatus>({ connected: false });
  const [connecting, setConnecting] = useState(false);
  const [tools, setTools] = useState<Listed<ToolView> | null>(null);
  const [resources, setResources] = useState<Resource[]>([]);
  const [templates, setTemplates] = useState<ResourceTemplate[]>([]);
  const [prompts, setPrompts] = useState<Prompt[]>([]);
  const [notifications, setNotifications] = useState<Notification[]>([]);
  const [log, setLog] = useState<LogEntry[]>([]);
  const [tab, setTab] = useState('tools');
  const [showConfig, setShowConfig] = useState(false);
  const [lastError, setLastError] = useState<string | null>(null);

  const loadPrimitives = useCallback(async () => {
    const caps = (await api.mcpStatus(id)).server?.capabilities ?? {};
    const [t, r, rt, p] = await Promise.all([
      'tools' in caps ? api.mcpListTools(id).catch(e => (toast(`tools/list: ${errorText(e)}`, 'error'), null)) : null,
      'resources' in caps ? api.mcpListResources(id).catch(() => null) : null,
      'resources' in caps ? api.mcpListTemplates(id).catch(() => null) : null,
      'prompts' in caps ? api.mcpListPrompts(id).catch(() => null) : null,
    ]);
    setTools(t);
    setResources(r?.items ?? []);
    setTemplates(rt?.items ?? []);
    setPrompts(p?.items ?? []);
  }, [id, toast]);

  useEffect(() => {
    setTools(null);
    setResources([]);
    setTemplates([]);
    setPrompts([]);
    setLastError(null);
    api.mcpStatus(id).then(s => {
      setStatus(s);
      if (s.connected) loadPrimitives();
      else setShowConfig(true);
    });
    api.mcpLog(id).then(initial => setLog(l => [...initial, ...l.filter(x => !initial.some(i => i.seq === x.seq))].sort((a, b) => a.seq - b.seq)));
    const un = onMcpLog((sid, entry) => {
      if (sid !== id) return;
      // The initial mcp_log fetch and live events can overlap; seq is unique.
      setLog(l => (l.some(x => x.seq === entry.seq) ? l : [...l, entry]));
      if (entry.kind === 'notification' && entry.direction === 'in') api.mcpNotifications(id).then(setNotifications);
    });
    return () => {
      un.then(f => f());
    };
  }, [id, loadPrimitives]);

  const connect = async () => {
    setConnecting(true);
    setLastError(null);
    try {
      await new Promise(r => setTimeout(r, 400)); // let autosave flush
      const s = await api.mcpConnect(id);
      setStatus(s);
      setShowConfig(false);
      await loadPrimitives();
      toast(`Connected to ${s.server?.serverInfo.title || s.server?.serverInfo.name}`, 'success');
    } catch (e) {
      setLastError(errorText(e));
      setStatus({ connected: false });
    } finally {
      setConnecting(false);
    }
  };

  const disconnect = async () => {
    await api.mcpDisconnect(id);
    setStatus({ connected: false });
    setTools(null);
  };

  if (!server) return <Empty title="Loading…" />;
  const t = server.transport;
  const update = (patch: Partial<McpServer>) => setServer({ ...server, ...patch });
  const info = status.server;

  return (
    <div className="flex h-full min-h-0 flex-col">
      {/* connection bar */}
      <div className="flex shrink-0 items-center gap-2 border-b border-app p-2">
        <Select
          aria-label="Transport"
          value={t.kind}
          onChange={e =>
            update({
              transport:
                e.target.value === 'stdio'
                  ? { kind: 'stdio', command: '', args: [], cwd: null }
                  : { kind: 'streamable-http', url: 'http://localhost:3333/mcp' },
            })
          }
          className="w-[132px] text-[12.5px]"
        >
          <option value="streamable-http">Streamable HTTP</option>
          <option value="stdio">stdio</option>
        </Select>
        {t.kind === 'streamable-http' ? (
          <VarInput
            className="flex-1"
            value={t.url}
            contextId={id}
            onChange={url => update({ transport: { ...t, url } })}
            onEnter={connect}
            placeholder="http://localhost:3333/mcp"
          />
        ) : (
          <VarInput
            className="flex-1"
            value={[t.command, joinArgs(t.args)].filter(Boolean).join(' ')}
            contextId={id}
            onChange={v => {
              const [command = '', ...args] = splitArgs(v);
              update({ transport: { ...t, command, args } });
            }}
            onEnter={connect}
            placeholder="npx -y @modelcontextprotocol/server-everything"
          />
        )}
        <Button variant="ghost" onClick={() => setShowConfig(!showConfig)} title="Headers, auth, env, roots">
          {showConfig ? <ChevronDown className="size-3.5" /> : <ChevronRight className="size-3.5" />} Settings
        </Button>
        {status.connected ? (
          <Button onClick={disconnect}>
            <Power className="size-3.5" /> Disconnect
          </Button>
        ) : (
          <Button variant="primary" onClick={connect} loading={connecting}>
            {!connecting && <Plug className="size-3.5" />} Connect
          </Button>
        )}
      </div>

      {showConfig && <ConnectionSettings server={server} update={update} contextId={id} />}

      {lastError && (
        <div className="m-3 rounded-lg border border-rose-500/30 bg-rose-500/5 p-3 text-[12.5px]">
          <div className="mb-1 flex items-center gap-1.5 font-semibold text-rose-700 dark:text-rose-400">
            <TriangleAlert className="size-4" /> Could not connect
          </div>
          <div className="selectable font-mono whitespace-pre-wrap">{lastError}</div>
          <div className="mt-2 text-muted">The Protocol log tab shows every frame that was exchanged before the failure.</div>
        </div>
      )}

      {status.connected && info ? (
        <ServerCard status={status} onRefresh={loadPrimitives} serverId={id} />
      ) : !lastError ? (
        <div className="flex items-center gap-2 border-b border-app px-4 py-2 text-[12.5px] text-muted">
          <Circle className="size-2.5 fill-zinc-400 text-zinc-400" /> Not connected
        </div>
      ) : null}

      <Tabs
        value={tab}
        onChange={setTab}
        tabs={[
          { id: 'tools', label: 'Tools', count: tools?.items.length ?? 0 },
          { id: 'resources', label: 'Resources', count: resources.length + templates.length },
          { id: 'prompts', label: 'Prompts', count: prompts.length },
          { id: 'notifications', label: 'Notifications', count: notifications.length },
          { id: 'log', label: 'Protocol log', count: log.filter(l => l.kind !== 'other').length },
        ]}
        right={
          tab === 'tools' && tools ? (
            <span className="text-[11.5px] text-muted">
              {tools.pages} page{tools.pages === 1 ? '' : 's'} · {formatMs(tools.elapsedMs)}
            </span>
          ) : undefined
        }
      />
      <div className="min-h-0 flex-1">
        {tab === 'tools' &&
          (tools ? (
            <ToolsPanel serverId={id} tools={tools.items} />
          ) : (
            <Empty icon={<Plug className="size-8" />} title={status.connected ? 'Loading tools…' : 'Connect to inspect tools'}>
              Each tool is shown as a readable card with a parameter table, behavior badges and a generated form for calling it.
            </Empty>
          ))}
        {tab === 'resources' && <ResourcesPanel serverId={id} resources={resources} templates={templates} connected={status.connected} />}
        {tab === 'prompts' && <PromptsPanel serverId={id} prompts={prompts} connected={status.connected} />}
        {tab === 'notifications' && <NotificationsPanel items={notifications} />}
        {tab === 'log' && <ProtocolLog entries={log} onClear={() => api.mcpLogClear(id).then(() => setLog([]))} />}
      </div>
    </div>
  );
}

function ServerCard({ status, onRefresh, serverId }: { status: McpStatus; onRefresh: () => void; serverId: string }) {
  const toast = useToast();
  const info = status.server!;
  const [open, setOpen] = useState(false);
  const caps = Object.entries(info.capabilities ?? {});
  return (
    <div className="shrink-0 border-b border-app bg-subtle px-4 py-2.5">
      <div className="flex items-center gap-2">
        <Circle className="size-2.5 fill-emerald-500 text-emerald-500" />
        <span className="font-semibold">{info.serverInfo.title || info.serverInfo.name}</span>
        <span className="font-mono text-[12px] text-muted">
          {info.serverInfo.name}
          {info.serverInfo.version && `@${info.serverInfo.version}`}
        </span>
        <Badge title="Negotiated protocol version">protocol {info.protocolVersion}</Badge>
        {caps.map(([k, v]) => (
          <Badge key={k} tone="violet" title={JSON.stringify(v)}>
            {k}
          </Badge>
        ))}
        <div className="flex-1" />
        {status.sessionId && (
          <span className="hidden font-mono text-[11px] text-muted lg:inline" title="Mcp-Session-Id">
            session {status.sessionId.slice(0, 18)}…
          </span>
        )}
        <Button
          size="sm"
          variant="ghost"
          onClick={() => api.mcpPing(serverId).then(ms => toast(`Pong in ${formatMs(ms)}`, 'success'), e => toast(errorText(e), 'error'))}
        >
          <Zap className="size-3.5" /> Ping
        </Button>
        <Button size="sm" variant="ghost" onClick={onRefresh}>
          <RefreshCw className="size-3.5" /> Refresh
        </Button>
        {info.instructions && (
          <Button size="sm" variant="ghost" onClick={() => setOpen(!open)}>
            <Info className="size-3.5" /> Instructions
          </Button>
        )}
      </div>
      {open && info.instructions && (
        <div className="mt-2 rounded-md border border-app bg-app p-3 text-[12.5px]">
          <Markdown>{info.instructions}</Markdown>
        </div>
      )}
    </div>
  );
}

function ConnectionSettings({ server, update, contextId }: { server: McpServer; update: (p: Partial<McpServer>) => void; contextId: string }) {
  const [tab, setTab] = useState(server.transport.kind === 'stdio' ? 'env' : 'headers');
  const auth = server.authentication;
  return (
    <div className="max-h-[40vh] shrink-0 overflow-auto border-b border-app bg-subtle">
      <Tabs
        value={tab}
        onChange={setTab}
        tabs={[
          { id: 'headers', label: 'Headers', count: server.headers.length },
          { id: 'auth', label: 'Auth', dot: auth.type === 'bearer' || auth.type === 'apikey' },
          { id: 'env', label: 'Env (stdio)', count: server.env.length },
          { id: 'roots', label: 'Roots', count: server.roots.length },
          { id: 'sampling', label: 'Sampling', dot: !!server.sampling?.enabled },
          { id: 'general', label: 'General' },
        ]}
      />
      {tab === 'headers' && <KeyValueEditor items={server.headers} onChange={headers => update({ headers })} contextId={contextId} namePlaceholder="Header" />}
      {tab === 'auth' && (
        <div className="flex max-w-xl flex-col gap-2 p-3">
          <Select
            value={auth.type === 'bearer' ? 'bearer' : 'none'}
            onChange={e => update({ authentication: e.target.value === 'bearer' ? { type: 'bearer', token: '' } : { type: 'none' } })}
            className="w-48"
          >
            <option value="none">No auth</option>
            <option value="bearer">Bearer token</option>
          </Select>
          {auth.type === 'bearer' && (
            <VarInput contextId={contextId} value={auth.token} onChange={token => update({ authentication: { ...auth, token } })} placeholder="{{ _.mcp_token }}" />
          )}
          <p className="text-[12px] text-muted">Sent as an Authorization header on every Streamable HTTP request. Use environment variables to keep tokens out of the config.</p>
        </div>
      )}
      {tab === 'env' && (
        <KeyValueEditor items={server.env} onChange={env => update({ env })} contextId={contextId} namePlaceholder="VARIABLE" />
      )}
      {tab === 'roots' && (
        <KeyValueEditor
          items={server.roots.map(r => ({ name: r.uri, value: r.name ?? '' }))}
          onChange={items => update({ roots: items.map(i => ({ uri: i.name, name: i.value || null })) })}
          namePlaceholder="file:///path/to/project"
          valuePlaceholder="name (optional)"
        />
      )}
      {tab === 'sampling' && <SamplingSettings server={server} update={update} />}
      {tab === 'general' && (
        <div className="flex max-w-xl flex-col gap-2 p-3">
          <label className="text-muted">Name</label>
          <Input value={server.name} onChange={e => update({ name: e.target.value })} />
          {server.transport.kind === 'stdio' && (
            <>
              <label className="text-muted">Working directory</label>
              <Input
                value={server.transport.cwd ?? ''}
                placeholder="(inherit)"
                onChange={e => server.transport.kind === 'stdio' && update({ transport: { ...server.transport, cwd: e.target.value || null } })}
              />
            </>
          )}
        </div>
      )}
    </div>
  );
}

function ResourcesPanel({
  serverId,
  resources,
  templates,
  connected,
}: {
  serverId: string;
  resources: Resource[];
  templates: ResourceTemplate[];
  connected: boolean;
}) {
  const toast = useToast();
  const [uri, setUri] = useState('');
  const [content, setContent] = useState<unknown>(null);
  const read = async (u: string) => {
    setUri(u);
    try {
      setContent(await api.mcpReadResource(serverId, u));
    } catch (e) {
      toast(errorText(e), 'error');
    }
  };
  if (!connected) return <Empty title="Connect to browse resources" />;
  return (
    <Split direction="row" initial={340} min={220} storageKey="lsock-split-resources">
      <div className="h-full overflow-auto bg-subtle p-2">
        <div className="px-1 pb-1 text-[11px] font-semibold tracking-wide text-muted uppercase">Resources</div>
        {resources.map(r => (
          <button key={r.uri} onClick={() => read(r.uri)} className={cn('mb-0.5 w-full rounded-md px-2.5 py-1.5 text-left', uri === r.uri ? 'bg-accent-soft' : 'hover:bg-muted')}>
            <div className="text-[13px] font-medium">{r.title || r.name}</div>
            <div className="truncate font-mono text-[11.5px] text-muted">{r.uri}</div>
            {r.mimeType && <Badge>{r.mimeType}</Badge>}
          </button>
        ))}
        {resources.length === 0 && <div className="px-2 py-1 text-[12px] text-muted">None</div>}
        <div className="px-1 pt-3 pb-1 text-[11px] font-semibold tracking-wide text-muted uppercase">Templates</div>
        {templates.map(t => (
          <button key={t.uriTemplate} onClick={() => setUri(t.uriTemplate)} className="mb-0.5 w-full rounded-md px-2.5 py-1.5 text-left hover:bg-muted">
            <div className="text-[13px] font-medium">{t.title || t.name}</div>
            <div className="truncate font-mono text-[11.5px] text-muted">{t.uriTemplate}</div>
            {t.description && <div className="text-[12px] text-muted">{t.description}</div>}
          </button>
        ))}
        {templates.length === 0 && <div className="px-2 py-1 text-[12px] text-muted">None</div>}
      </div>
      <div className="flex h-full min-h-0 flex-col">
        <div className="flex shrink-0 gap-2 border-b border-app p-2">
          <Input className="font-mono" value={uri} onChange={e => setUri(e.target.value)} placeholder="resource URI (fill in template variables)" onKeyDown={e => e.key === 'Enter' && read(uri)} />
          <Button variant="primary" onClick={() => read(uri)} disabled={!uri}>
            Read
          </Button>
        </div>
        <div className="min-h-0 flex-1 overflow-auto p-3">
          {content ? <JsonTree value={content} onPath={() => {}} defaultOpen={4} /> : <Empty title="Select a resource to read it" />}
        </div>
      </div>
    </Split>
  );
}

function PromptsPanel({ serverId, prompts, connected }: { serverId: string; prompts: Prompt[]; connected: boolean }) {
  const toast = useToast();
  const [sel, setSel] = useState<string | null>(null);
  const [args, setArgs] = useState<Record<string, string>>({});
  const [out, setOut] = useState<unknown>(null);
  const p = prompts.find(x => x.name === sel) ?? prompts[0];
  if (!connected) return <Empty title="Connect to browse prompts" />;
  if (!p) return <Empty title="This server exposes no prompts" />;
  return (
    <Split direction="row" initial={300} min={200} storageKey="lsock-split-prompts">
      <div className="h-full overflow-auto bg-subtle p-2">
        {prompts.map(x => (
          <button
            key={x.name}
            onClick={() => (setSel(x.name), setOut(null), setArgs({}))}
            className={cn('mb-0.5 w-full rounded-md px-2.5 py-1.5 text-left', p.name === x.name ? 'bg-accent-soft' : 'hover:bg-muted')}
          >
            <div className="font-mono text-[12.5px] font-medium">{x.name}</div>
            {x.description && <div className="text-[12px] text-muted">{x.description}</div>}
          </button>
        ))}
      </div>
      <div className="h-full overflow-auto p-4">
        <h2 className="text-[16px] font-semibold">{p.title || p.name}</h2>
        {p.description && <p className="mt-1 text-muted">{p.description}</p>}
        <div className="mt-4 flex max-w-xl flex-col gap-3">
          {p.arguments.map(a => (
            <label key={a.name} className="flex flex-col gap-1">
              <span className="font-mono text-[12.5px]">
                {a.name} {a.required && <span className="text-[11px] text-amber-600">required</span>}
              </span>
              <Input value={args[a.name] ?? ''} onChange={e => setArgs({ ...args, [a.name]: e.target.value })} />
              {a.description && <span className="text-[12px] text-muted">{a.description}</span>}
            </label>
          ))}
          <div>
            <Button variant="primary" onClick={() => api.mcpGetPrompt(serverId, p.name, args).then(setOut, e => toast(errorText(e), 'error'))}>
              Get prompt
            </Button>
          </div>
        </div>
        {out !== null && (
          <div className="mt-4 flex flex-col gap-2">
            {((out as { messages?: { role: string; content: { type: string; text?: string } }[] }).messages ?? []).map((m, i) => (
              <div key={i} className="rounded-md border border-app p-3">
                <Badge tone={m.role === 'user' ? 'blue' : 'violet'}>{m.role}</Badge>
                <pre className="selectable mt-2 font-mono text-[12.5px] whitespace-pre-wrap">{m.content.text ?? JSON.stringify(m.content, null, 2)}</pre>
              </div>
            ))}
          </div>
        )}
      </div>
    </Split>
  );
}

function NotificationsPanel({ items }: { items: Notification[] }) {
  if (!items.length)
    return <Empty title="No notifications yet">Server notifications (logging messages, progress, list changes) appear here as they arrive.</Empty>;
  return (
    <div className="h-full overflow-auto">
      {[...items].reverse().map((n, i) => (
        <div key={i} className="flex gap-3 border-b border-app px-4 py-2">
          <span className="shrink-0 font-mono text-[11.5px] text-muted">{clockTime(n.timestampMs)}</span>
          <span className="shrink-0 font-mono text-[12.5px] text-violet-700 dark:text-violet-300">{n.method}</span>
          <span className="selectable min-w-0 flex-1 font-mono text-[12px] break-all text-muted">{JSON.stringify(n.params)}</span>
        </div>
      ))}
    </div>
  );
}

const KIND_TONE: Record<string, string> = { request: 'blue', response: 'green', notification: 'violet', error: 'red', other: 'neutral' };

function ProtocolLog({ entries, onClear }: { entries: LogEntry[]; onClear: () => void }) {
  const [filter, setFilter] = useState<string>('all');
  const [q, setQ] = useState('');
  const [selected, setSelected] = useState<number | null>(null);
  const [follow, setFollow] = useState(true);
  const list = useRef<HTMLDivElement>(null);
  const shown = useMemo(
    () =>
      entries.filter(
        e =>
          (filter === 'all' || e.kind === filter || (filter === 'transport' && e.kind === 'other')) &&
          (!q || JSON.stringify(e.message).toLowerCase().includes(q.toLowerCase()) || (e.method ?? '').includes(q)),
      ),
    [entries, filter, q],
  );
  useEffect(() => {
    if (follow && list.current) list.current.scrollTop = list.current.scrollHeight;
  }, [shown.length, follow]);
  const sel = entries.find(e => e.seq === selected);
  return (
    <Split direction="row" initial={620} min={360} storageKey="lsock-split-log">
      <div className="flex h-full min-h-0 flex-col overflow-hidden">
        <div className="flex shrink-0 flex-wrap items-center gap-1.5 border-b border-app p-2">
          {['all', 'request', 'response', 'notification', 'error', 'transport'].map(f => (
            <button
              key={f}
              onClick={() => setFilter(f)}
              className={cn('rounded-full px-2.5 py-0.5 text-[12px] capitalize', filter === f ? 'bg-accent text-white' : 'bg-muted text-muted hover:text-app')}
            >
              {f}
            </button>
          ))}
          <Input className="h-7 w-36 text-[12px]" placeholder="Search frames" value={q} onChange={e => setQ(e.target.value)} />
          <div className="flex-1" />
          <label className="flex items-center gap-1 text-[12px] text-muted">
            <input type="checkbox" checked={follow} onChange={e => setFollow(e.target.checked)} /> Follow
          </label>
          <Button size="sm" variant="ghost" onClick={onClear}>
            <Trash2 className="size-3.5" /> Clear
          </Button>
        </div>
        <div ref={list} className="min-h-0 flex-1 overflow-auto font-mono text-[12px]">
          {shown.map(e => (
            <button
              key={e.seq}
              onClick={() => setSelected(e.seq)}
              className={cn(
                'grid w-full grid-cols-[34px_92px_18px_96px_1fr_70px] items-center gap-2 border-b border-app px-2 py-1 text-left',
                selected === e.seq ? 'bg-accent-soft' : 'hover:bg-muted',
              )}
            >
              <span className="text-right text-muted">{e.seq}</span>
              <span className="text-muted">{clockTime(e.timestampMs)}</span>
              {e.direction === 'out' ? (
                <ArrowUpRight className="size-3.5 text-sky-500" aria-label="sent" />
              ) : e.direction === 'in' ? (
                <ArrowDownLeft className="size-3.5 text-emerald-500" aria-label="received" />
              ) : e.direction === 'error' ? (
                <TriangleAlert className="size-3.5 text-rose-500" aria-label="error" />
              ) : (
                <Info className="size-3.5 text-muted" aria-label="info" />
              )}
              <span>
                <Badge tone={KIND_TONE[e.kind]}>{e.kind === 'other' ? e.direction : e.kind}</Badge>
              </span>
              <span className="truncate">
                {e.method ?? String((e.message as { text?: string }).text ?? '')}
                {e.id !== undefined && e.id !== null && <span className="ml-1.5 text-muted">#{String(e.id)}</span>}
              </span>
              <span className="text-right text-amber-600 dark:text-amber-400">{e.latencyMs != null ? formatMs(e.latencyMs) : ''}</span>
            </button>
          ))}
          {shown.length === 0 && <Empty title="No frames">Connect to a server — every JSON-RPC message in both directions is recorded here.</Empty>}
        </div>
      </div>
      <div className="flex h-full min-h-0 flex-col">
        {sel ? (
          <>
            <div className="flex shrink-0 items-center gap-2 border-b border-app px-3 py-1.5">
              <Badge tone={KIND_TONE[sel.kind]}>{sel.kind}</Badge>
              <span className="font-mono text-[12.5px]">{sel.method}</span>
              {sel.latencyMs != null && <span className="text-[12px] text-muted">round trip {formatMs(sel.latencyMs)}</span>}
              <div className="flex-1" />
              <CopyButton text={JSON.stringify(sel.message, null, 2)} />
            </div>
            <CodeEditor value={JSON.stringify(sel.message, null, 2)} readOnly />
          </>
        ) : (
          <Empty title="Select a frame">The full JSON of the selected message is shown here.</Empty>
        )}
      </div>
    </Split>
  );
}

function SamplingSettings({ server, update }: { server: McpServer; update: (p: Partial<McpServer>) => void }) {
  const [providers] = useProviders();
  const s = server.sampling ?? { enabled: false, providerId: null, model: null, maxTokens: 1024 };
  const set = (p: Partial<typeof s>) => update({ sampling: { ...s, ...p } });
  return (
    <div className="flex max-w-xl flex-col gap-2.5 p-3">
      <p className="text-[12.5px] text-muted">
        Some MCP servers ask the client's LLM to generate text (<code className="font-mono">sampling/createMessage</code>). Allow it with a provider of your choice — every request is
        recorded in the Protocol log. Reconnect after changing this.
      </p>
      <label className="flex items-center gap-2">
        <input type="checkbox" className="accent-[var(--color-accent)]" checked={s.enabled} onChange={e => set({ enabled: e.target.checked, providerId: s.providerId ?? providers[0]?.id ?? null })} />
        Allow this server to use my AI provider
      </label>
      {s.enabled && (
        <div className="grid grid-cols-3 gap-2">
          <Select value={s.providerId ?? ''} onChange={e => set({ providerId: e.target.value })}>
            {!providers.length && <option value="">Add a provider first</option>}
            {providers.map(p => (
              <option key={p.id} value={p.id}>
                {p.name}
              </option>
            ))}
          </Select>
          <Input value={s.model ?? ''} placeholder={providers.find(p => p.id === s.providerId)?.defaultModel ?? 'model'} onChange={e => set({ model: e.target.value || null })} />
          <Input type="number" value={s.maxTokens || 1024} onChange={e => set({ maxTokens: Number(e.target.value) || 1024 })} title="Max tokens per request" />
        </div>
      )}
    </div>
  );
}
