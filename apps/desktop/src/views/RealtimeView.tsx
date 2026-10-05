import { ArrowDownLeft, ArrowUpRight, Circle, Info, Plug, Power, Radio, Send, Trash2, TriangleAlert } from 'lucide-react';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';

import { CodeEditor, JsonTree, KeyValueEditor } from '../components/editors';
import { Badge, Button, CopyButton, Empty, Input, Kbd, Select, Split, Tabs, useToast } from '../components/ui';
import { VarInput } from '../components/VarInput';
import { api, errorText, onRtEvent, type RealtimeRequest, type RtEvent, type RtKind } from '../lib/api';
import { clockTime, cn, formatBytes, modKey, prettyJson } from '../lib/utils';
import { AuthEditor } from './AuthEditor';
import { useAutosave } from './RequestView';

const KINDS: { id: RtKind; label: string }[] = [
  { id: 'websocket', label: 'WebSocket' },
  { id: 'socketio', label: 'Socket.IO' },
  { id: 'sse', label: 'Event stream (SSE)' },
];

function parsed(data: string): unknown | undefined {
  const t = data.trim();
  if (!t.startsWith('{') && !t.startsWith('[')) return undefined;
  try {
    return JSON.parse(t);
  } catch {
    return undefined;
  }
}

export function RealtimeView({ id }: { id: string }) {
  const toast = useToast();
  const [req, setReq, flush] = useAutosave<RealtimeRequest>(id, api.rtUpdate);
  const [connected, setConnected] = useState(false);
  const [connecting, setConnecting] = useState(false);
  const [since, setSince] = useState<number | null>(null);
  const [now, setNow] = useState(Date.now());
  const [events, setEvents] = useState<RtEvent[]>([]);
  const [tab, setTab] = useState('message');
  const [filter, setFilter] = useState('all');
  const [q, setQ] = useState('');
  const [selected, setSelected] = useState<number | null>(null);
  const [ack, setAck] = useState(false);
  const list = useRef<HTMLDivElement>(null);

  useEffect(() => {
    api.rtStatus(id).then(s => setConnected(s.connected));
    api.rtLog(id).then(setEvents);
    const un = onRtEvent((rid, ev) => {
      if (rid !== id) return;
      // seq is unique per connection; handshake events are replayed once on connect
      setEvents(prev => (prev.some(x => x.seq === ev.seq) ? prev : [...prev, ev]));
      if (ev.kind === 'close' || ev.direction === 'error') api.rtStatus(id).then(s => setConnected(s.connected));
    });
    return () => {
      un.then(f => f());
    };
  }, [id]);

  useEffect(() => {
    if (!connected) return;
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, [connected]);

  useEffect(() => {
    if (list.current) list.current.scrollTop = list.current.scrollHeight;
  }, [events.length]);

  const connect = async () => {
    setConnecting(true);
    try {
      await flush();
      setEvents([]);
      setSelected(null);
      await api.rtConnect(id);
      setConnected(true);
      setSince(Date.now());
    } catch (e) {
      toast(errorText(e), 'error');
      api.rtLog(id).then(setEvents);
    } finally {
      setConnecting(false);
    }
  };
  const disconnect = async () => {
    await api.rtDisconnect(id);
    setConnected(false);
    setSince(null);
  };

  const send = useCallback(async () => {
    if (!req || !connected || req.kind === 'sse') return;
    try {
      await flush();
      await api.rtSend(id, req.payload, req.kind === 'socketio' ? req.event : null, ack);
    } catch (e) {
      toast(errorText(e), 'error');
    }
  }, [req, connected, id, ack, flush, toast]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') {
        e.preventDefault();
        if (connected) send();
        else connect();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  });

  const shown = useMemo(
    () =>
      events.filter(
        e =>
          (filter === 'all' || (filter === 'sent' && e.direction === 'out') || (filter === 'received' && e.direction === 'in') || (filter === 'system' && (e.direction === 'info' || e.direction === 'error'))) &&
          (!q || e.data.toLowerCase().includes(q.toLowerCase()) || (e.name ?? '').toLowerCase().includes(q.toLowerCase())),
      ),
    [events, filter, q],
  );

  if (!req) return <Empty title="Loading…" />;
  const update = (p: Partial<RealtimeRequest>) => setReq({ ...req, ...p });
  const sel = events.find(e => e.seq === selected);
  const sent = events.filter(e => e.direction === 'out').length;
  const received = events.filter(e => e.direction === 'in').length;
  const isJson = req.payloadFormat === 'json';
  const payloadError = isJson && req.payload.trim() && !req.payload.includes('{{') && prettyJson(req.payload) === null;

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex shrink-0 items-center gap-2 border-b border-app p-2">
        <Select aria-label="Protocol" value={req.kind} onChange={e => update({ kind: e.target.value as RtKind })} className="w-[150px] text-[12.5px]" disabled={connected}>
          {KINDS.map(k => (
            <option key={k.id} value={k.id}>
              {k.label}
            </option>
          ))}
        </Select>
        <VarInput className="flex-1" value={req.url} contextId={id} onChange={url => update({ url })} onEnter={connected ? undefined : connect} placeholder="wss://example.com/socket" />
        {connected ? (
          <Button onClick={disconnect}>
            <Power className="size-3.5" /> Disconnect
          </Button>
        ) : (
          <Button variant="primary" onClick={connect} loading={connecting}>
            {!connecting && <Plug className="size-3.5" />} Connect
          </Button>
        )}
      </div>
      <div className="flex shrink-0 items-center gap-3 border-b border-app bg-subtle px-4 py-1.5 text-[12px] text-muted">
        <span className="flex items-center gap-1.5">
          <Circle className={cn('size-2.5', connected ? 'fill-emerald-500 text-emerald-500' : 'fill-zinc-400 text-zinc-400')} />
          {connected ? `Connected${since ? ` · ${Math.max(0, Math.round((now - since) / 1000))}s` : ''}` : 'Not connected'}
        </span>
        <span>↑ {sent} sent</span>
        <span>↓ {received} received</span>
      </div>
      <Split direction="row" initial={460} min={320} storageKey="irs-split-rt">
        <div className="flex h-full min-h-0 flex-col">
          <Tabs
            value={tab}
            onChange={setTab}
            tabs={[
              { id: 'message', label: req.kind === 'sse' ? 'Request' : 'Message' },
              { id: 'headers', label: 'Headers', count: req.headers.filter(h => !h.disabled && h.name).length },
              { id: 'auth', label: 'Auth', dot: req.authentication.type !== 'inherit' && req.authentication.type !== 'none' },
              { id: 'settings', label: 'Settings' },
            ]}
          />
          <div className="min-h-0 flex-1 overflow-auto">
            {tab === 'message' &&
              (req.kind === 'sse' ? (
                <div className="flex flex-col gap-3 p-4">
                  <p className="text-[12.5px] text-muted">
                    Server-Sent Events are receive-only: connect and incoming events appear on the right, with their <code className="font-mono">event</code> name and{' '}
                    <code className="font-mono">id</code>.
                  </p>
                  <label className="flex items-center gap-3">
                    <span className="w-16 text-muted">Method</span>
                    <Select value={req.method || 'GET'} onChange={e => update({ method: e.target.value })}>
                      {['GET', 'POST'].map(m => (
                        <option key={m}>{m}</option>
                      ))}
                    </Select>
                  </label>
                  {req.method === 'POST' && (
                    <textarea
                      value={req.body}
                      onChange={e => update({ body: e.target.value })}
                      placeholder="Request body"
                      className="h-32 rounded-md border border-app bg-app p-2 font-mono text-[12.5px] outline-none focus:border-accent"
                    />
                  )}
                </div>
              ) : (
                <div className="flex h-full min-h-0 flex-col">
                  <div className="flex shrink-0 items-center gap-2 border-b border-app px-3 py-1.5">
                    {req.kind === 'socketio' && (
                      <>
                        <span className="text-[12px] text-muted">Event</span>
                        <Input className="h-7 w-40 font-mono text-[12.5px]" value={req.event} onChange={e => update({ event: e.target.value })} placeholder="message" />
                        <label className="flex items-center gap-1 text-[12px] text-muted">
                          <input type="checkbox" checked={ack} onChange={e => setAck(e.target.checked)} /> ack
                        </label>
                      </>
                    )}
                    <div className="flex-1" />
                    <div className="flex rounded-md border border-app p-0.5">
                      {(['json', 'text'] as const).map(f => (
                        <button
                          key={f}
                          onClick={() => update({ payloadFormat: f })}
                          className={cn('rounded px-2 py-0.5 text-[11.5px] uppercase', req.payloadFormat === f ? 'bg-accent text-white' : 'text-muted')}
                        >
                          {f}
                        </button>
                      ))}
                    </div>
                  </div>
                  <CodeEditor
                    value={req.payload}
                    onChange={payload => update({ payload })}
                    language={isJson ? 'json' : 'text'}
                    placeholder={req.kind === 'socketio' ? '["arg1", {"arg2": true}]  — an array becomes the event arguments' : '{"type": "subscribe", "channel": "{{ _.channel }}"}'}
                  />
                  <div className="flex shrink-0 items-center gap-2 border-t border-app px-3 py-2">
                    {payloadError && <span className="text-[12px] text-amber-600">Not valid JSON</span>}
                    <span className="text-[11.5px] text-muted">
                      <Kbd>{modKey()}</Kbd> <Kbd>↵</Kbd> {connected ? 'send' : 'connect'}
                    </span>
                    <div className="flex-1" />
                    <Button variant="primary" onClick={send} disabled={!connected}>
                      <Send className="size-3.5" /> {req.kind === 'socketio' ? 'Emit' : 'Send'}
                    </Button>
                  </div>
                </div>
              ))}
            {tab === 'headers' && <KeyValueEditor items={req.headers} onChange={headers => update({ headers })} contextId={id} namePlaceholder="Header" />}
            {tab === 'auth' && <AuthEditor auth={req.authentication} onChange={authentication => update({ authentication })} contextId={id} />}
            {tab === 'settings' && (
              <div className="flex max-w-xl flex-col gap-3 p-4">
                {req.kind === 'websocket' && (
                  <label className="flex flex-col gap-1">
                    <span className="text-[12px] text-muted">Subprotocols (comma-separated)</span>
                    <Input
                      value={req.subprotocols.join(', ')}
                      onChange={e => update({ subprotocols: e.target.value.split(',').map(s => s.trim()).filter(Boolean) })}
                      placeholder="graphql-transport-ws, mqtt"
                    />
                  </label>
                )}
                {req.kind === 'socketio' && (
                  <>
                    <label className="flex flex-col gap-1">
                      <span className="text-[12px] text-muted">Namespace</span>
                      <Input value={req.namespace} onChange={e => update({ namespace: e.target.value })} placeholder="/" />
                    </label>
                    <label className="flex flex-col gap-1">
                      <span className="text-[12px] text-muted">Handshake auth (JSON)</span>
                      <textarea
                        value={req.socketioAuth}
                        onChange={e => update({ socketioAuth: e.target.value })}
                        placeholder={'{"token": "{{ _.token }}"}'}
                        className="h-24 rounded-md border border-app bg-app p-2 font-mono text-[12.5px] outline-none focus:border-accent"
                      />
                    </label>
                    <p className="text-[12px] text-muted">Socket.IO v4 over the WebSocket transport. The URL is the server origin; /socket.io/ is added automatically.</p>
                  </>
                )}
                {req.kind === 'sse' && <p className="text-[12.5px] text-muted">The client sends Accept: text/event-stream and keeps the connection open until you disconnect or the server ends the stream.</p>}
              </div>
            )}
          </div>
        </div>

        <div className="flex h-full min-h-0 flex-col">
          <div className="flex shrink-0 flex-wrap items-center gap-1.5 border-b border-app p-2">
            {['all', 'received', 'sent', 'system'].map(f => (
              <button
                key={f}
                onClick={() => setFilter(f)}
                className={cn('rounded-full px-2.5 py-0.5 text-[12px] capitalize', filter === f ? 'bg-accent text-white' : 'bg-muted text-muted hover:text-app')}
              >
                {f}
              </button>
            ))}
            <Input className="h-7 w-40 text-[12px]" placeholder="Search messages" value={q} onChange={e => setQ(e.target.value)} />
            <div className="flex-1" />
            <Button size="sm" variant="ghost" onClick={() => setEvents([])}>
              <Trash2 className="size-3.5" /> Clear
            </Button>
          </div>
          <Split direction="col" initial={420} min={120} storageKey="irs-split-rt-log">
            <div ref={list} className="h-full overflow-auto font-mono text-[12px]">
              {shown.map(e => (
                <button
                  key={e.seq}
                  onClick={() => setSelected(e.seq)}
                  className={cn('grid w-full grid-cols-[18px_88px_auto_1fr_60px] items-center gap-2 border-b border-app px-2 py-1 text-left', selected === e.seq ? 'bg-accent-soft' : 'hover:bg-muted')}
                >
                  {e.direction === 'out' ? (
                    <ArrowUpRight className="size-3.5 text-sky-500" aria-label="sent" />
                  ) : e.direction === 'in' ? (
                    <ArrowDownLeft className="size-3.5 text-emerald-500" aria-label="received" />
                  ) : e.direction === 'error' ? (
                    <TriangleAlert className="size-3.5 text-rose-500" aria-label="error" />
                  ) : (
                    <Info className="size-3.5 text-muted" aria-label="info" />
                  )}
                  <span className="text-muted">{clockTime(e.timestampMs)}</span>
                  <span>{e.name ? <Badge tone="violet">{e.name}</Badge> : e.kind !== 'text' && <Badge>{e.kind}</Badge>}</span>
                  <span className={cn('truncate', e.direction === 'error' && 'text-rose-600', e.direction === 'info' && 'text-muted')}>{e.data}</span>
                  <span className="text-right text-[11px] text-muted">{e.direction === 'in' || e.direction === 'out' ? formatBytes(e.size) : ''}</span>
                </button>
              ))}
              {shown.length === 0 && (
                <Empty icon={<Radio className="size-8" />} title={connected ? 'Waiting for messages…' : 'No messages yet'}>
                  Connect, then {req.kind === 'sse' ? 'events stream in here' : 'send a message — both directions are logged here'}.
                </Empty>
              )}
            </div>
            <div className="flex h-full min-h-0 flex-col">
              {sel ? (
                <>
                  <div className="flex shrink-0 items-center gap-2 border-b border-app px-3 py-1.5 text-[12px]">
                    <Badge tone={sel.direction === 'in' ? 'green' : sel.direction === 'out' ? 'blue' : 'neutral'}>{sel.direction}</Badge>
                    {sel.name && <span className="font-mono">{sel.name}</span>}
                    <span className="text-muted">{clockTime(sel.timestampMs)}</span>
                    <div className="flex-1" />
                    <CopyButton text={sel.data} />
                  </div>
                  <div className="min-h-0 flex-1 overflow-auto p-3">
                    {parsed(sel.data) !== undefined ? (
                      <JsonTree value={parsed(sel.data)} onPath={p => (navigator.clipboard?.writeText(p), toast(`Copied ${p}`))} defaultOpen={4} />
                    ) : (
                      <pre className="selectable font-mono text-[12.5px] whitespace-pre-wrap">{sel.data}</pre>
                    )}
                  </div>
                </>
              ) : (
                <Empty title="Select a message">JSON messages are shown as a tree.</Empty>
              )}
            </div>
          </Split>
        </div>
      </Split>
    </div>
  );
}

