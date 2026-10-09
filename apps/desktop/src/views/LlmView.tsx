import { Brain, CheckCircle2, ChevronDown, ChevronRight, Loader2, Plus, Send, ShieldAlert, Sparkles, Square, Trash2, Wrench, XCircle } from 'lucide-react';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';

import { CodeEditor, JsonTree, Markdown } from '../components/editors';
import { Badge, Button, CopyButton, Empty, Input, Kbd, Select, Split, Tabs, useToast } from '../components/ui';
import { api, type AgentEvent, errorText, type LlmMessage, type LlmRequest, type LlmRun, onLlmEvent, type ToolCallInfo, type ToolResultInfo, type TreeNode } from '../lib/api';
import { cn, formatMs, modKey, timeAgo } from '../lib/utils';
import { useProviders } from './AiProviders';
import { ActionBadge, actionReason } from './McpTools';
import { useAutosave } from './RequestView';

type Item =
  | { kind: 'user'; text: string }
  | { kind: 'assistant'; turn: number; text: string; thinking: string; streaming: boolean }
  | { kind: 'tool'; call: ToolCallInfo; state: 'pending' | 'running' | 'done'; result?: ToolResultInfo }
  | { kind: 'error'; message: string };

interface Stats {
  model: string;
  inputTokens: number;
  outputTokens: number;
  turns: number;
  ttftMs?: number | null;
  totalMs: number;
}

/** Rebuild the conversation view from a stored run. */
function itemsFromRun(run: LlmRun): Item[] {
  const results = new Map(run.toolCalls.map(t => [t.call.id, t]));
  const items: Item[] = [];
  let turn = 0;
  for (const m of run.transcript as LlmMessage[]) {
    if (m.role === 'user') {
      const text = m.content.filter(b => b.type === 'text').map(b => (b as { text: string }).text).join('\n');
      if (text) items.push({ kind: 'user', text });
      continue;
    }
    turn += 1;
    const text = m.content.filter(b => b.type === 'text').map(b => (b as { text: string }).text).join('');
    const thinking = m.content.filter(b => b.type === 'thinking').map(b => (b as { thinking: string }).thinking).join('\n');
    if (text || thinking) items.push({ kind: 'assistant', turn, text, thinking, streaming: false });
    for (const b of m.content) {
      if (b.type === 'tool_use') {
        const t = results.get(b.id);
        items.push({ kind: 'tool', call: t?.call ?? { id: b.id, name: b.name, server: '', tool: b.name, input: b.input }, state: 'done', result: t?.result });
      }
    }
  }
  if (run.error) items.push({ kind: 'error', message: run.error });
  return items;
}

function ToolCard({ item, onDecide }: { item: Extract<Item, { kind: 'tool' }>; onDecide?: (allow: boolean, always?: boolean) => void }) {
  const [open, setOpen] = useState(item.state !== 'done' || !!item.result?.isError);
  const r = item.result;
  const destructive = item.call.action?.action === 'delete';
  return (
    <div className={cn('rounded-lg border bg-app', item.state === 'pending' ? (destructive ? 'border-rose-500/60' : 'border-amber-500/60') : 'border-app')}>
      <button className="flex w-full items-center gap-2 px-3 py-2 text-left" onClick={() => setOpen(!open)}>
        {open ? <ChevronDown className="size-3.5 text-muted" /> : <ChevronRight className="size-3.5 text-muted" />}
        <Wrench className="size-3.5 text-violet-500" />
        <span className="font-mono text-[12.5px]">
          {item.call.server && <span className="text-muted">{item.call.server}.</span>}
          <span className="font-semibold">{item.call.tool}</span>
        </span>
        <ActionBadge action={item.call.action} />
        <div className="flex-1" />
        {item.state === 'pending' && <Badge tone={destructive ? 'red' : 'amber'}>needs approval</Badge>}
        {item.state === 'running' && <Loader2 className="size-3.5 animate-spin text-muted" />}
        {r && (r.denied ? <Badge tone="amber">denied</Badge> : r.isError ? <XCircle className="size-4 text-rose-500" /> : <CheckCircle2 className="size-4 text-emerald-500" />)}
        {r && !r.denied && <span className="text-[11.5px] text-muted">{formatMs(r.latencyMs)}</span>}
      </button>
      {open && (
        <div className="border-t border-app px-3 py-2">
          <div className="mb-1 text-[13px] font-semibold tracking-tight text-app">Arguments</div>
          <JsonTree value={item.call.input} onPath={() => {}} defaultOpen={3} />
          {item.state === 'pending' && onDecide && (
            <div className="mt-3 flex items-center gap-2 rounded-md bg-subtle p-2.5">
              <ShieldAlert className={cn('size-4', destructive ? 'text-rose-500' : 'text-amber-500')} />
              <span className="flex-1 text-[12.5px]">
                The model wants to run <b className="font-mono">{item.call.tool}</b>. {actionReason(item.call.action)}
              </span>
              <Button size="sm" onClick={() => onDecide(false)}>
                Deny
              </Button>
              {item.call.allowKey && (
                <Button size="sm" variant="ghost" onClick={() => onDecide(true, true)} title="Run this tool without asking from now on, in this AI request">
                  Always allow
                </Button>
              )}
              <Button size="sm" variant={destructive ? 'danger' : 'primary'} onClick={() => onDecide(true)}>
                Allow
              </Button>
            </div>
          )}
          {r && (
            <>
              <div className="mt-2 mb-1 text-[13px] font-semibold tracking-tight text-app">Result</div>
              <pre className={cn('selectable max-h-64 overflow-auto rounded bg-subtle p-2 font-mono text-[12px] whitespace-pre-wrap', r.isError && 'text-rose-700 dark:text-rose-400')}>{r.text}</pre>
            </>
          )}
        </div>
      )}
    </div>
  );
}

export function LlmView({ id, tree, onOpenProviders }: { id: string; tree: TreeNode[]; onOpenProviders: () => void }) {
  const toast = useToast();
  const [req, setReq, flush] = useAutosave<LlmRequest>(id, api.llmRequestUpdate);
  const [providers] = useProviders();
  const [models, setModels] = useState<string[]>([]);
  const [items, setItems] = useState<Item[]>([]);
  const [stats, setStats] = useState<Stats | null>(null);
  const [runs, setRuns] = useState<LlmRun[]>([]);
  const [selectedRun, setSelectedRun] = useState<LlmRun | null>(null);
  const [running, setRunning] = useState(false);
  const [tab, setTab] = useState('chat');
  const runId = useRef<string | null>(null);
  const bottom = useRef<HTMLDivElement>(null);

  const servers = useMemo(() => {
    const out: TreeNode[] = [];
    const walk = (ns: TreeNode[]) => ns.forEach(n => (n.kind === 'mcp' ? out.push(n) : walk(n.children)));
    walk(tree);
    return out;
  }, [tree]);

  const loadRuns = useCallback(async () => {
    const r = await api.llmRuns(id).catch(() => []);
    setRuns(r);
    return r;
  }, [id]);

  useEffect(() => {
    loadRuns().then(r => {
      if (r[0]) {
        setSelectedRun(r[0]);
        setItems(itemsFromRun(r[0]));
        setStats({ model: r[0].model, inputTokens: r[0].inputTokens, outputTokens: r[0].outputTokens, turns: r[0].turns, ttftMs: r[0].ttftMs, totalMs: r[0].totalMs });
      }
    });
  }, [loadRuns]);

  useEffect(() => {
    const un = onLlmEvent((rid, ev: AgentEvent) => {
      if (rid !== runId.current) return;
      setItems(prev => {
        const next = [...prev];
        const lastAssistant = () => {
          for (let i = next.length - 1; i >= 0; i--) if (next[i].kind === 'assistant') return i;
          return -1;
        };
        switch (ev.type) {
          case 'turnStart':
            next.push({ kind: 'assistant', turn: ev.turn, text: '', thinking: '', streaming: true });
            break;
          case 'stream': {
            const i = lastAssistant();
            if (i < 0) break;
            const a = next[i] as Extract<Item, { kind: 'assistant' }>;
            if (ev.event.type === 'textDelta') next[i] = { ...a, text: a.text + ev.event.text };
            if (ev.event.type === 'thinkingDelta') next[i] = { ...a, thinking: a.thinking + ev.event.text };
            break;
          }
          case 'assistantMessage': {
            const i = lastAssistant();
            if (i >= 0) next[i] = { ...(next[i] as Extract<Item, { kind: 'assistant' }>), streaming: false };
            break;
          }
          case 'toolCall':
            next.push({ kind: 'tool', call: ev.call, state: ev.needsApproval ? 'pending' : 'running' });
            break;
          case 'toolResult': {
            const i = next.findIndex(x => x.kind === 'tool' && x.call.id === ev.result.id);
            if (i >= 0) next[i] = { ...(next[i] as Extract<Item, { kind: 'tool' }>), state: 'done', result: ev.result };
            break;
          }
          case 'error':
            next.push({ kind: 'error', message: ev.message });
            break;
        }
        return next.filter(x => !(x.kind === 'assistant' && !x.streaming && !x.text && !x.thinking));
      });
      if (ev.type === 'assistantMessage')
        setStats(s => ({
          model: s?.model ?? '',
          inputTokens: (s?.inputTokens ?? 0) + ev.usage.inputTokens,
          outputTokens: (s?.outputTokens ?? 0) + ev.usage.outputTokens,
          turns: ev.turn,
          ttftMs: s?.ttftMs ?? ev.ttftMs,
          totalMs: s?.totalMs ?? 0,
        }));
      if (ev.type === 'saved') {
        setRunning(false);
        setSelectedRun(ev.run);
        // the stored transcript has the rendered prompt ({{ vars }} resolved)
        setItems(itemsFromRun(ev.run));
        setStats({ model: ev.run.model, inputTokens: ev.run.inputTokens, outputTokens: ev.run.outputTokens, turns: ev.run.turns, ttftMs: ev.run.ttftMs, totalMs: ev.run.totalMs });
        loadRuns();
      }
      if (ev.type === 'error' && !runId.current) setRunning(false);
    });
    return () => {
      un.then(f => f());
    };
  }, [loadRuns]);

  useEffect(() => {
    bottom.current?.scrollIntoView({ block: 'end' });
  }, [items]);

  const provider = providers.find(p => p.id === req?.providerId) ?? providers[0];
  useEffect(() => {
    setModels([]);
  }, [provider?.id]);

  const send = useCallback(async () => {
    if (!req || running) return;
    if (!provider) {
      onOpenProviders();
      return;
    }
    await flush();
    const rid = `llmrun_${crypto.randomUUID().replace(/-/g, '')}`;
    runId.current = rid;
    setRunning(true);
    setTab('chat');
    setSelectedRun(null);
    setStats({ model: req.model || provider.defaultModel, inputTokens: 0, outputTokens: 0, turns: 0, totalMs: 0 });
    setItems(req.messages.filter(m => m.text.trim()).map(m => (m.role === 'user' ? { kind: 'user' as const, text: m.text } : { kind: 'assistant' as const, turn: 0, text: m.text, thinking: '', streaming: false })));
    try {
      await api.llmRunStart(rid, req.id);
    } catch (e) {
      setRunning(false);
      setItems(it => [...it, { kind: 'error', message: errorText(e) }]);
    }
  }, [req, running, provider, flush, onOpenProviders]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') {
        e.preventDefault();
        send();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [send]);

  if (!req) return <Empty title="Loading…" />;
  const update = (p: Partial<LlmRequest>) => setReq({ ...req, ...p });
  const decide = (call: ToolCallInfo, allow: boolean, always?: boolean) => {
    if (!runId.current) return;
    setItems(it => it.map(x => (x.kind === 'tool' && x.call.id === call.id ? { ...x, state: 'running' } : x)));
    const remember = allow && always && call.allowKey ? { requestId: id, allowKey: call.allowKey } : undefined;
    // Keep the open editor's copy in step, so its autosave does not drop the new entry.
    if (remember && !(req.alwaysAllow ?? []).includes(remember.allowKey)) update({ alwaysAllow: [...(req.alwaysAllow ?? []), remember.allowKey] });
    api.llmApprove(runId.current, call.id, allow, undefined, remember).catch(e => toast(errorText(e), 'error'));
  };

  return (
    <Split direction="row" initial={420} min={340} storageKey="lsock-split-llm">
      <div className="flex h-full min-h-0 flex-col">
        <div className="min-h-0 flex-1 overflow-auto p-4">
          <div className="mb-3 flex items-center gap-2">
            <Sparkles className="size-4 text-accent" />
            <Input className="font-medium" value={req.name} onChange={e => update({ name: e.target.value })} aria-label="Name" />
          </div>
          <div className="mb-3 grid grid-cols-2 gap-2">
            <label className="flex flex-col gap-1">
              <span className="text-[11.5px] font-medium text-muted">Provider</span>
              <Select
                value={provider?.id ?? ''}
                onChange={e => (e.target.value === '__manage' ? onOpenProviders() : update({ providerId: e.target.value }))}
              >
                {!providers.length && <option value="">No providers</option>}
                {providers.map(p => (
                  <option key={p.id} value={p.id}>
                    {p.name}
                    {p.hasKey ? '' : ' (no key)'}
                  </option>
                ))}
                <option value="__manage">Manage providers…</option>
              </Select>
            </label>
            <label className="flex flex-col gap-1">
              <span className="text-[11.5px] font-medium text-muted">Model</span>
              <Input
                list={`models-${id}`}
                value={req.model}
                placeholder={provider?.defaultModel || 'model id'}
                onFocus={() => provider && !models.length && api.llmModels(provider.id).then(setModels).catch(() => {})}
                onChange={e => update({ model: e.target.value })}
              />
              <datalist id={`models-${id}`}>
                {[...new Set([...models, ...(provider?.suggestedModels ?? [])])].map(m => (
                  <option key={m} value={m} />
                ))}
              </datalist>
            </label>
          </div>
          <label className="mb-3 flex flex-col gap-1">
            <span className="text-[11.5px] font-medium text-muted">System prompt</span>
            <textarea
              value={req.system}
              onChange={e => update({ system: e.target.value })}
              placeholder="You are a helpful API assistant. Variables like {{ _.base_url }} work here."
              className="h-20 resize-y rounded-md border border-app bg-app p-2 text-[13px] outline-none focus:border-accent"
            />
          </label>
          <div className="mb-1 text-[11.5px] font-medium text-muted">Messages</div>
          {req.messages.map((m, i) => (
            <div key={i} className="mb-2 rounded-md border border-app bg-app">
              <div className="flex items-center gap-2 border-b border-app px-2 py-1">
                <Select
                  className="h-6 border-0 px-1 text-[12px]"
                  value={m.role}
                  onChange={e => update({ messages: req.messages.map((x, j) => (j === i ? { ...x, role: e.target.value as 'user' | 'assistant' } : x)) })}
                >
                  <option value="user">User</option>
                  <option value="assistant">Assistant</option>
                </Select>
                <div className="flex-1" />
                {req.messages.length > 1 && (
                  <button aria-label="Remove message" className="text-muted hover:text-rose-500" onClick={() => update({ messages: req.messages.filter((_, j) => j !== i) })}>
                    <Trash2 className="size-3.5" />
                  </button>
                )}
              </div>
              <textarea
                value={m.text}
                onChange={e => update({ messages: req.messages.map((x, j) => (j === i ? { ...x, text: e.target.value } : x)) })}
                placeholder={i === 0 ? 'Ask something… e.g. “List the open issues and summarise them” — {{ variables }} work here' : ''}
                className="h-24 w-full resize-y bg-transparent p-2 text-[13px] outline-none"
              />
            </div>
          ))}
          <Button size="sm" variant="ghost" onClick={() => update({ messages: [...req.messages, { role: req.messages.at(-1)?.role === 'user' ? 'assistant' : 'user', text: '' }] })}>
            <Plus className="size-3.5" /> Add message
          </Button>

          <div className="mt-4 mb-1 flex items-center gap-1.5 text-[11.5px] font-medium text-muted">
            <Wrench className="size-3.5" /> Tools from MCP servers
          </div>
          <div className="rounded-md border border-app bg-app">
            {servers.map(s => (
              <label key={s.id} className="flex cursor-pointer items-center gap-2 border-b border-app px-2.5 py-1.5 last:border-b-0 hover:bg-muted">
                <input
                  type="checkbox"
                  className="accent-[var(--color-accent)]"
                  checked={req.mcpServerIds.includes(s.id)}
                  onChange={e => update({ mcpServerIds: e.target.checked ? [...req.mcpServerIds, s.id] : req.mcpServerIds.filter(x => x !== s.id) })}
                />
                <span className="truncate">{s.name}</span>
              </label>
            ))}
            {!servers.length && <div className="p-2.5 text-[12px] text-muted">Add an MCP server to this collection to let the model call its tools.</div>}
          </div>
          <div className="mt-3 grid grid-cols-3 gap-2">
            <label className="col-span-3 flex flex-col gap-1">
              <span className="text-[11.5px] font-medium text-muted">Run tools without asking</span>
              <Select value={req.autoApprove} onChange={e => update({ autoApprove: e.target.value as LlmRequest['autoApprove'] })}>
                <option value="read-only">Read tools (recommended): ask for Create, Update, Delete, and unknown tools</option>
                <option value="none">Never — ask for every call</option>
                <option value="all">Always — trusted servers only</option>
              </Select>
            </label>
            {(req.alwaysAllow ?? []).length > 0 && (
              <div className="col-span-3 flex flex-col gap-1">
                <span className="text-[11.5px] font-medium text-muted">Always allowed</span>
                <div className="flex flex-wrap gap-1">
                  {(req.alwaysAllow ?? []).map(k => (
                    <span key={k} className="inline-flex items-center gap-1 rounded border border-app bg-subtle px-1.5 py-0.5 font-mono text-[11.5px]">
                      {k.split('::').pop()}
                      <button
                        aria-label={`Stop always allowing ${k.split('::').pop()}`}
                        className="text-muted hover:text-rose-500"
                        onClick={() => update({ alwaysAllow: (req.alwaysAllow ?? []).filter(x => x !== k) })}
                      >
                        ×
                      </button>
                    </span>
                  ))}
                </div>
              </div>
            )}
            <label className="flex flex-col gap-1">
              <span className="text-[11.5px] font-medium text-muted">Max turns</span>
              <Input type="number" min={1} value={req.maxTurns} onChange={e => update({ maxTurns: Math.max(1, Number(e.target.value) || 1) })} />
            </label>
            <label className="flex flex-col gap-1">
              <span className="text-[11.5px] font-medium text-muted">Max tokens</span>
              <Input type="number" min={1} value={req.maxTokens} onChange={e => update({ maxTokens: Math.max(1, Number(e.target.value) || 1) })} />
            </label>
            <label className="flex flex-col gap-1">
              <span className="text-[11.5px] font-medium text-muted">Temperature</span>
              <Input
                type="number"
                step={0.1}
                min={0}
                max={2}
                placeholder="default"
                value={req.temperature ?? ''}
                onChange={e => update({ temperature: e.target.value === '' ? null : Number(e.target.value) })}
              />
            </label>
          </div>
        </div>
        <div className="flex shrink-0 gap-2 border-t border-app p-3">
          {running ? (
            <Button className="flex-1" onClick={() => runId.current && api.llmCancel(runId.current)}>
              <Square className="size-3.5" /> Stop
            </Button>
          ) : (
            <Button className="flex-1" variant="primary" onClick={send} title={`Send (${modKey()}+Enter)`}>
              <Send className="size-3.5" /> {provider ? 'Send' : 'Add an AI provider'}
            </Button>
          )}
        </div>
      </div>

      <div className="flex h-full min-h-0 flex-col">
        <Tabs
          value={tab}
          onChange={setTab}
          tabs={[
            { id: 'chat', label: 'Conversation' },
            { id: 'trace', label: 'Request trace', count: selectedRun?.requestBodies.length ?? 0 },
          ]}
          right={
            runs.length > 0 && !running ? (
              <Select
                aria-label="Run history"
                className="h-7 max-w-60 text-[12px]"
                value={selectedRun?.id ?? ''}
                onChange={e => {
                  const r = runs.find(x => x.id === e.target.value);
                  if (r) {
                    setSelectedRun(r);
                    setItems(itemsFromRun(r));
                    setStats({ model: r.model, inputTokens: r.inputTokens, outputTokens: r.outputTokens, turns: r.turns, ttftMs: r.ttftMs, totalMs: r.totalMs });
                  }
                }}
              >
                {runs.map(r => (
                  <option key={r.id} value={r.id}>
                    {r.error ? 'Error' : r.model} · {r.turns} turn{r.turns === 1 ? '' : 's'} · {timeAgo(r.created)}
                  </option>
                ))}
              </Select>
            ) : undefined
          }
        />
        {tab === 'chat' ? (
          <div className="min-h-0 flex-1 overflow-auto">
            {items.length === 0 ? (
              <Empty icon={<Sparkles className="size-8" />} title="Ask a model — with your MCP tools">
                Pick a provider, write a prompt, tick the MCP servers it may use, and press <Kbd>{modKey()}</Kbd> <Kbd>Enter</Kbd>. Tool calls appear here live; risky ones wait for your approval.
              </Empty>
            ) : (
              <div className="mx-auto flex max-w-3xl flex-col gap-3 p-4">
                {items.map((it, i) =>
                  it.kind === 'user' ? (
                    <div key={i} className="ml-auto max-w-[85%] rounded-2xl rounded-br-sm bg-accent px-3.5 py-2 whitespace-pre-wrap text-on-accent">
                      {it.text}
                    </div>
                  ) : it.kind === 'assistant' ? (
                    <div key={i} className="flex flex-col gap-1.5">
                      {it.thinking && (
                        <details className="rounded-md border border-app bg-subtle px-3 py-1.5 text-[12.5px] text-muted">
                          <summary className="flex cursor-pointer items-center gap-1.5">
                            <Brain className="size-3.5" /> Thinking
                          </summary>
                          <div className="mt-1 whitespace-pre-wrap">{it.thinking}</div>
                        </details>
                      )}
                      {(it.text || it.streaming) && (
                        <div className="max-w-[92%] rounded-2xl rounded-bl-sm border border-app bg-app px-3.5 py-2">
                          {it.text ? <Markdown>{it.text}</Markdown> : <Loader2 className="size-4 animate-spin text-muted" />}
                          {it.streaming && it.text && <span className="ml-0.5 inline-block h-3.5 w-1.5 animate-pulse bg-accent align-middle" />}
                        </div>
                      )}
                    </div>
                  ) : it.kind === 'tool' ? (
                    <ToolCard key={it.call.id} item={it} onDecide={running ? (allow, always) => decide(it.call, allow, always) : undefined} />
                  ) : (
                    <div key={i} className="rounded-lg border border-rose-500/30 bg-rose-500/5 px-3 py-2 font-mono text-[12.5px] text-rose-700 dark:text-rose-400">
                      {it.message}
                    </div>
                  ),
                )}
                <div ref={bottom} />
              </div>
            )}
          </div>
        ) : (
          <div className="flex min-h-0 flex-1 flex-col">
            {selectedRun?.requestBodies.length ? (
              <>
                <div className="flex shrink-0 justify-between border-b border-app px-3 py-1.5 text-[12px] text-muted">
                  <span>Exact JSON sent to {selectedRun.providerName} for each turn (API key not included)</span>
                  <CopyButton text={JSON.stringify(selectedRun.requestBodies, null, 2)} />
                </div>
                <CodeEditor value={JSON.stringify(selectedRun.requestBodies, null, 2)} readOnly />
              </>
            ) : (
              <Empty title="No trace yet">Run the request to see the exact provider payloads.</Empty>
            )}
          </div>
        )}
        {stats && (
          <div className="flex h-8 shrink-0 items-center gap-3 border-t border-app px-4 text-[11.5px] text-muted">
            {running && <Loader2 className="size-3.5 animate-spin" />}
            <span className="font-medium text-app">{stats.model}</span>
            <span>
              {stats.inputTokens} in · {stats.outputTokens} out tokens
            </span>
            <span>
              {stats.turns} turn{stats.turns === 1 ? '' : 's'}
            </span>
            {stats.ttftMs != null && <span>first token {formatMs(stats.ttftMs)}</span>}
            {stats.totalMs > 0 && <span>total {formatMs(stats.totalMs)}</span>}
          </div>
        )}
      </div>
    </Split>
  );
}
