import { AlertTriangle, Braces, CheckCircle2, FileCode2, FileJson, PanelLeftClose, PanelLeftOpen, Play, Search, ShieldAlert, Wrench, XCircle } from 'lucide-react';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';

import { CodeEditor, JsonTree, Markdown } from '../components/editors';
import { SchemaForm } from '../components/SchemaForm';
import { Badge, Button, CopyButton, Empty, IconButton, Kbd, Modal, Split, Tabs, useToast } from '../components/ui';
import { api, type CallToolResult, errorText, type ParamRow, type ToolActionInfo, type ToolView } from '../lib/api';
import { cn, formatMs, methodColor, modKey } from '../lib/utils';
import { CodeModal } from './ImportExport';

const ACTION_STYLE: Record<ToolActionInfo['action'], { label: string; method: string; meaning: string }> = {
  read: { label: 'Read', method: 'GET', meaning: 'Only looks things up' },
  create: { label: 'Create', method: 'POST', meaning: 'Adds or submits something new' },
  update: { label: 'Update', method: 'PUT', meaning: 'Changes something that exists' },
  delete: { label: 'Delete', method: 'DELETE', meaning: 'Removes or cancels something' },
};
const SOURCE: Record<ToolActionInfo['source'], string> = {
  server: "from the server's hints",
  ai: 'inferred from the description by AI',
  name: "guessed from the tool's name",
};

/** What a tool does, coloured like HTTP methods. Nothing when it is unknown. */
export function ActionBadge({ action }: { action?: ToolActionInfo | null }) {
  if (!action) return null;
  const a = ACTION_STYLE[action.action];
  return (
    <span
      className={cn('inline-flex items-center rounded-full border border-current/25 px-2 py-px text-[10.5px] font-semibold', methodColor(a.method))}
      title={`${a.label}: ${a.meaning.toLowerCase()} (${SOURCE[action.source]})`}
    >
      {a.label}
    </span>
  );
}

/** Why the AI chat asks before running a tool, in one sentence. */
export function actionReason(action?: ToolActionInfo | null): string {
  if (!action) return 'Logic Socket can’t tell what this tool does, so it asks first.';
  const a = ACTION_STYLE[action.action];
  return `${a.label}: ${a.meaning.toLowerCase()} (${SOURCE[action.source]}).`;
}

/** Delete tools confirmed this session (server id::tool name), so each asks only once. */
const confirmedDeletes = new Set<string>();

function fmt(v: unknown): string {
  return typeof v === 'string' ? `"${v}"` : JSON.stringify(v);
}

export function ParamTable({ rows, empty = 'No parameters' }: { rows: ParamRow[]; empty?: string }) {
  if (!rows.length) return <div className="px-1 py-2 text-[12.5px] text-muted">{empty}</div>;
  return (
    <div className="overflow-hidden rounded-lg border border-app">
      <table className="selectable w-full text-[12.5px]">
        <thead className="bg-subtle text-left text-[11px] tracking-wide text-muted uppercase">
          <tr>
            <th className="px-3 py-1.5 font-semibold">Parameter</th>
            <th className="px-3 py-1.5 font-semibold">Type</th>
            <th className="px-3 py-1.5 font-semibold">Default</th>
            <th className="w-1/2 px-3 py-1.5 font-semibold">Details</th>
          </tr>
        </thead>
        <tbody>
          {rows.map(r => (
            <tr key={r.path} className="border-t border-app align-top">
              <td className="px-3 py-2 whitespace-nowrap" style={{ paddingLeft: 12 + r.depth * 16 }}>
                {r.depth > 0 && <span className="mr-1 text-muted">└</span>}
                <span className={cn('font-mono', r.required && 'font-semibold')}>{r.name}</span>
                {r.required && (
                  <span className="ml-1 text-amber-600 dark:text-amber-400" title="Required">
                    *
                  </span>
                )}
              </td>
              <td className="px-3 py-2">
                <code className="rounded bg-muted px-1.5 py-0.5 font-mono text-[11.5px] whitespace-nowrap">{r.typeLabel}</code>
              </td>
              <td className="px-3 py-2 font-mono text-[11.5px] text-muted">{r.default !== undefined && r.default !== null ? fmt(r.default) : ''}</td>
              <td className="px-3 py-2">
                {r.description && <div className="mb-1">{r.description}</div>}
                <div className="flex flex-wrap gap-1">
                  {r.constraints.map(c => (
                    <Badge key={c}>{c}</Badge>
                  ))}
                  {r.enumValues.length > 0 && (
                    <span className="text-[11.5px] text-muted">
                      one of{' '}
                      {r.enumValues.map((v, i) => (
                        <code key={i} className="mx-0.5 rounded bg-accent-soft px-1 font-mono text-accent">
                          {typeof v === 'string' ? v : JSON.stringify(v)}
                        </code>
                      ))}
                    </span>
                  )}
                </div>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function ContentBlock({ block }: { block: Record<string, unknown> }) {
  const toast = useToast();
  const type = block.type as string;
  if (type === 'text') {
    const text = String(block.text ?? '');
    try {
      const parsed = JSON.parse(text);
      if (parsed && typeof parsed === 'object')
        return <JsonTree value={parsed} onPath={p => (navigator.clipboard?.writeText(p), toast(`Copied ${p}`))} />;
    } catch {
      /* plain text */
    }
    return <pre className="selectable font-mono text-[12.5px] whitespace-pre-wrap">{text}</pre>;
  }
  if (type === 'image') return <img alt="Tool output" className="max-w-full rounded" src={`data:${block.mimeType};base64,${block.data}`} />;
  if (type === 'audio') return <audio controls src={`data:${block.mimeType};base64,${block.data}`} />;
  if (type === 'resource_link' || type === 'resource') {
    const res = (block.resource as Record<string, unknown>) ?? block;
    return (
      <div className="rounded-md border border-app p-2">
        <div className="font-mono text-[12px] text-accent">{String(res.uri ?? '')}</div>
        {res.text ? <pre className="selectable mt-1 font-mono text-[12px] whitespace-pre-wrap">{String(res.text)}</pre> : null}
      </div>
    );
  }
  return <JsonTree value={block} onPath={() => {}} />;
}

function Result({ result, latency, error }: { result: CallToolResult | null; latency: number; error: string | null }) {
  const [raw, setRaw] = useState(false);
  if (error)
    return (
      <div className="rounded-lg border border-rose-500/30 bg-rose-500/5 p-3">
        <div className="mb-1 flex items-center gap-1.5 font-semibold text-rose-700 dark:text-rose-400">
          <XCircle className="size-4" /> Protocol error
        </div>
        <div className="selectable font-mono text-[12.5px] whitespace-pre-wrap">{error}</div>
      </div>
    );
  if (!result) return null;
  return (
    <div className="flex flex-col gap-2">
      <div className="flex items-center gap-2">
        {result.isError ? (
          <span className="flex items-center gap-1.5 font-semibold text-rose-700 dark:text-rose-400">
            <AlertTriangle className="size-4" /> Tool returned an error
          </span>
        ) : (
          <span className="flex items-center gap-1.5 font-semibold text-emerald-700 dark:text-emerald-400">
            <CheckCircle2 className="size-4" /> Success
          </span>
        )}
        <span className="text-[12px] text-muted">{formatMs(latency)}</span>
        <span className="text-[12px] text-muted">
          · {result.content.length} content block{result.content.length === 1 ? '' : 's'}
        </span>
        <div className="flex-1" />
        <Button size="sm" variant="ghost" onClick={() => setRaw(!raw)}>
          <FileJson className="size-3.5" /> {raw ? 'Readable' : 'Raw JSON'}
        </Button>
        <CopyButton text={JSON.stringify(result, null, 2)} />
      </div>
      {raw ? (
        <div className="h-72 overflow-hidden rounded-md border border-app">
          <CodeEditor value={JSON.stringify(result, null, 2)} readOnly />
        </div>
      ) : (
        <>
          {result.content.map((b, i) => (
            <div key={i} className="rounded-md border border-app bg-subtle p-3">
              <div className="mb-1.5 text-[13px] font-semibold tracking-tight text-app">{String(b.type)}</div>
              <ContentBlock block={b} />
            </div>
          ))}
          {result.structuredContent !== undefined && result.structuredContent !== null && (
            <div className="rounded-md border border-app bg-subtle p-3">
              <div className="mb-1.5 text-[13px] font-semibold tracking-tight text-app">Structured content</div>
              <JsonTree value={result.structuredContent} onPath={() => {}} defaultOpen={3} />
            </div>
          )}
        </>
      )}
    </div>
  );
}

/** Arguments and result side by side when the pane is wide enough, stacked otherwise. */
const SIDE_BY_SIDE = 760;

function ToolDetail({ serverId, tool, onShowList }: { serverId: string; tool: ToolView; onShowList?: () => void }) {
  const pane = useRef<HTMLDivElement>(null);
  const [wide, setWide] = useState(true);
  useEffect(() => {
    const el = pane.current;
    if (!el) return;
    const ro = new ResizeObserver(([entry]) => setWide(entry.contentRect.width >= SIDE_BY_SIDE));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  const [view, setView] = useState('overview');
  const [mode, setMode] = useState<'form' | 'json'>('form');
  const [args, setArgs] = useState<unknown>(tool.example ?? {});
  const [jsonText, setJsonText] = useState(JSON.stringify(tool.example ?? {}, null, 2));
  const [errors, setErrors] = useState<string[]>([]);
  const [running, setRunning] = useState(false);
  const [result, setResult] = useState<CallToolResult | null>(null);
  const [callError, setCallError] = useState<string | null>(null);
  const [latency, setLatency] = useState(0);
  const [confirm, setConfirm] = useState(false);
  const [codeOpen, setCodeOpen] = useState(false);

  useEffect(() => {
    setArgs(tool.example ?? {});
    setJsonText(JSON.stringify(tool.example ?? {}, null, 2));
    setResult(null);
    setCallError(null);
    setView('overview');
  }, [tool.name, tool.example]);

  useEffect(() => {
    const t = setTimeout(() => api.mcpValidate(tool.inputSchema, args).then(setErrors).catch(() => {}), 150);
    return () => clearTimeout(t);
  }, [args, tool.inputSchema]);

  const run = useCallback(
    async (confirmed = false) => {
      // You chose this tool and clicked Call tool; only a Delete asks, once per session.
      const key = `${serverId}::${tool.name}`;
      if (tool.action?.action === 'delete' && !confirmed && !confirmedDeletes.has(key)) {
        setConfirm(true);
        return;
      }
      if (confirmed) confirmedDeletes.add(key);
      setConfirm(false);
      setRunning(true);
      setCallError(null);
      setView('try');
      try {
        const r = await api.mcpCallTool(serverId, tool.name, args);
        setResult(r.result);
        setLatency(r.latencyMs);
      } catch (e) {
        setResult(null);
        setCallError(errorText(e));
      } finally {
        setRunning(false);
      }
    },
    [serverId, tool, args],
  );

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === 'Enter' && view === 'try') {
        e.preventDefault();
        run();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [run, view]);

  const definition = useMemo(() => {
    const { displayName: _d, hints: _h, params: _p, outputParams: _o, example: _e, ...rest } = tool;
    return JSON.stringify(rest, null, 2);
  }, [tool]);

  return (
    <div ref={pane} className="flex h-full min-h-0 flex-col">
      <div className="shrink-0 border-b border-app px-5 pt-4 pb-3">
        <div className="flex items-start gap-3">
          {onShowList && (
            <IconButton label="Show tool list" onClick={onShowList} className="-ml-2">
              <PanelLeftOpen className="size-4" />
            </IconButton>
          )}
          <div className="min-w-0 flex-1">
            <h2 className="text-[17px] leading-tight font-semibold">{tool.displayName}</h2>
            {tool.displayName !== tool.name && <div className="selectable mt-0.5 font-mono text-[12px] text-muted">{tool.name}</div>}
          </div>
          <Button variant="primary" onClick={() => setView('try')}>
            <Play className="size-3.5" /> Try it
          </Button>
        </div>
        <div className="mt-2">
          {tool.action ? (
            <ActionBadge action={tool.action} />
          ) : (
            <span className="text-[12px] text-muted">
              Logic Socket can&rsquo;t tell what this tool does from its server or its name. Turn on AI labels in Settings › General to read its description.
            </span>
          )}
        </div>
      </div>
      <Tabs
        value={view}
        onChange={setView}
        tabs={[
          { id: 'overview', label: 'Overview' },
          { id: 'try', label: 'Try it', dot: !!result || !!callError },
          { id: 'json', label: 'Definition JSON' },
        ]}
      />
      {/* A flex column, so the Try it split fills the height instead of shrinking to its content. */}
      <div className="flex min-h-0 flex-1 flex-col overflow-auto">
        {view === 'overview' && (
          <div className="flex max-w-4xl flex-col gap-5 p-5">
            {tool.description ? <Markdown>{tool.description}</Markdown> : <div className="text-muted italic">No description provided.</div>}
            <section>
              <h3 className="mb-2 text-[13px] font-semibold tracking-tight text-app">
                Input parameters <span className="font-normal normal-case">· {tool.params.filter(p => p.depth === 0).length} top-level</span>
              </h3>
              <ParamTable rows={tool.params} empty="This tool takes no arguments." />
            </section>
            {tool.outputSchema && (
              <section>
                <h3 className="mb-2 text-[13px] font-semibold tracking-tight text-app">Returns (structured output)</h3>
                <ParamTable rows={tool.outputParams} empty="Output schema has no named properties." />
              </section>
            )}
          </div>
        )}
        {view === 'try' && (
          <Split
            key={wide ? 'row' : 'col'}
            direction={wide ? 'row' : 'col'}
            initial={wide ? 560 : 260}
            min={wide ? 300 : 140}
            storageKey={wide ? 'lsock-split-try-row' : 'lsock-split-try'}
          >
            <div className="flex h-full min-h-0 flex-col">
              <div className="flex shrink-0 flex-wrap items-center gap-x-2 gap-y-1.5 border-b border-app px-4 py-2">
                <div className="flex rounded-md border border-app p-0.5">
                  {(['form', 'json'] as const).map(m => (
                    <button
                      key={m}
                      onClick={() => {
                        if (m === 'json') setJsonText(JSON.stringify(args ?? {}, null, 2));
                        setMode(m);
                      }}
                      className={cn('rounded px-2.5 py-0.5 text-[12px]', mode === m ? 'bg-accent text-on-accent' : 'text-muted hover:text-app')}
                    >
                      {m === 'form' ? 'Form' : 'JSON'}
                    </button>
                  ))}
                </div>
                <Button size="sm" variant="ghost" onClick={() => (setArgs(tool.example ?? {}), setJsonText(JSON.stringify(tool.example ?? {}, null, 2)))}>
                  <Braces className="size-3.5" /> Reset to example
                </Button>
                <div className="flex-1" />
                {errors.length > 0 ? (
                  <span className="text-[12px] whitespace-nowrap text-amber-600 dark:text-amber-400">
                    {errors.length} issue{errors.length > 1 ? 's' : ''}
                  </span>
                ) : (
                  <span className="text-[12px] whitespace-nowrap text-emerald-600 dark:text-emerald-400">Arguments valid</span>
                )}
                <Button variant="ghost" onClick={() => setCodeOpen(true)} title="This call as cURL or code">
                  <FileCode2 className="size-3.5" /> Code
                </Button>
                <Button variant="primary" onClick={() => run()} loading={running} title={`Call tool (${modKey()}+Enter)`}>
                  {!running && <Play className="size-3.5" />} Call tool
                </Button>
                <CodeModal
                  open={codeOpen}
                  onClose={() => setCodeOpen(false)}
                  requestId={`${serverId}:${tool.name}:${JSON.stringify(args)}`}
                  requestName={tool.name}
                  intro="This tools/call with the arguments above, using the server's URL, headers, and auth. When connected, it includes the live session ID so it works as is. Secrets appear in plain text, so share carefully."
                  generate={async target => {
                    const st = await api.mcpStatus(serverId);
                    const message = { jsonrpc: '2.0', id: 1, method: 'tools/call', params: { name: tool.name, arguments: args ?? {} } };
                    return api.mcpCodeGenerate(serverId, target, message, st.sessionId, st.server?.protocolVersion);
                  }}
                />
              </div>
              <div className="min-h-0 flex-1 overflow-auto">
                {mode === 'form' ? (
                  <div className="p-4">
                    <SchemaForm root={tool.inputSchema} schema={tool.inputSchema} value={args} onChange={setArgs} />
                  </div>
                ) : (
                  <CodeEditor
                    value={jsonText}
                    onChange={t => {
                      setJsonText(t);
                      try {
                        setArgs(JSON.parse(t));
                      } catch {
                        /* keep last valid */
                      }
                    }}
                  />
                )}
              </div>
              {errors.length > 0 && (
                <ul className="shrink-0 border-t border-app bg-amber-500/5 px-4 py-2 text-[12px] text-amber-700 dark:text-amber-400">
                  {errors.map(e => (
                    <li key={e}>• {e}</li>
                  ))}
                </ul>
              )}
            </div>
            <div className="h-full overflow-auto p-4">
              {running ? (
                <div className="flex items-center gap-2 text-muted">
                  <span className="size-2 animate-ping rounded-full bg-accent" /> Calling {tool.name}…
                </div>
              ) : result || callError ? (
                <Result result={result} latency={latency} error={callError} />
              ) : (
                <div className="text-[12.5px] text-muted">
                  Fill in the arguments and press <Kbd>{modKey()}</Kbd> <Kbd>Enter</Kbd>. Every frame is recorded in the Protocol log tab.
                </div>
              )}
            </div>
          </Split>
        )}
        {view === 'json' && (
          <div className="flex h-full flex-col">
            <div className="flex shrink-0 justify-end border-b border-app px-2 py-1">
              <CopyButton text={definition} />
            </div>
            <CodeEditor value={definition} readOnly />
          </div>
        )}
      </div>
      <Modal
        open={confirm}
        onClose={() => setConfirm(false)}
        width="max-w-md"
        title={
          <span className="flex items-center gap-2">
            <ShieldAlert className="size-4 text-rose-500" /> Run a Delete tool?
          </span>
        }
        footer={
          <>
            <Button onClick={() => setConfirm(false)}>Cancel</Button>
            <Button variant="danger" onClick={() => run(true)}>
              Run {tool.name}
            </Button>
          </>
        }
      >
        <div className="p-4 text-[13px] leading-relaxed">
          <b className="font-mono">{tool.name}</b> removes or cancels something ({tool.action ? SOURCE[tool.action.source] : 'unknown'}). You won&rsquo;t be asked again for
          this tool until you restart Logic Socket.
        </div>
      </Modal>
    </div>
  );
}

export function ToolsPanel({ serverId, tools }: { serverId: string; tools: ToolView[] }) {
  const [listHidden, setListHidden] = useState(() => {
    try {
      return localStorage.getItem('lsock-tools-list-hidden') === '1';
    } catch {
      return false;
    }
  });
  const toggleList = (hidden: boolean) => {
    setListHidden(hidden);
    try {
      localStorage.setItem('lsock-tools-list-hidden', hidden ? '1' : '0');
    } catch {
      /* ignore */
    }
  };
  const [q, setQ] = useState('');
  const [selected, setSelected] = useState<string | null>(tools[0]?.name ?? null);
  const filtered = useMemo(() => {
    const ql = q.toLowerCase();
    return tools.filter(
      t => !ql || t.name.toLowerCase().includes(ql) || t.displayName.toLowerCase().includes(ql) || (t.description ?? '').toLowerCase().includes(ql),
    );
  }, [tools, q]);
  const tool = tools.find(t => t.name === selected) ?? filtered[0];
  if (!tools.length) return <Empty icon={<Wrench className="size-8" />} title="This server exposes no tools" />;
  return (
    <Split direction="row" initial={300} min={220} storageKey="lsock-split-tools" hideFirst={listHidden}>
      <div className="flex h-full min-h-0 flex-col bg-subtle">
        <div className="flex shrink-0 items-center gap-1 p-2">
          <div className="relative flex-1">
            <Search className="pointer-events-none absolute top-2.5 left-3 size-3.5 text-muted" />
            <input
              value={q}
              onChange={e => setQ(e.target.value)}
              placeholder={`Search ${tools.length} tools`}
              aria-label="Search tools"
              className="h-8 w-full rounded-full border border-app bg-app pr-3 pl-8 text-[12.5px] outline-none focus:border-accent"
            />
          </div>
          <IconButton label="Hide tool list" onClick={() => toggleList(true)}>
            <PanelLeftClose className="size-4" />
          </IconButton>
        </div>
        <div className="min-h-0 flex-1 overflow-auto px-1.5 pb-3">
          {filtered.map(t => (
            <button
              key={t.name}
              onClick={() => setSelected(t.name)}
              className={cn(
                'mb-0.5 flex w-full flex-col gap-1 rounded-md px-2.5 py-2 text-left',
                tool?.name === t.name ? 'bg-accent-soft' : 'hover:bg-muted',
              )}
            >
              <div className="flex items-center gap-2">
                <span className="min-w-0 flex-1 truncate font-mono text-[12.5px] font-medium">{t.name}</span>
                <span className="text-[11px] text-muted">{t.params.filter(p => p.depth === 0).length} params</span>
              </div>
              {t.description && <span className="line-clamp-2 text-[12px] text-muted">{t.description.replace(/[*_`#]/g, '')}</span>}
              {t.action && (
                <span>
                  <ActionBadge action={t.action} />
                </span>
              )}
            </button>
          ))}
          {filtered.length === 0 && <div className="p-4 text-center text-[12.5px] text-muted">No tools match “{q}”</div>}
        </div>
      </div>
      {tool ? (
        <ToolDetail key={tool.name} serverId={serverId} tool={tool} onShowList={listHidden ? () => toggleList(false) : undefined} />
      ) : (
        <Empty title="Select a tool" />
      )}
    </Split>
  );
}
