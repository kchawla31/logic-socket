import { CheckCircle2, KeyRound, Plus, Sparkles, Trash2, Zap } from 'lucide-react';
import { useEffect, useState } from 'react';

import { Badge, Button, Empty, Input, Modal, Select, useDialog, useToast } from '../components/ui';
import { api, errorText, type KeySource, type LlmProvider } from '../lib/api';
import { cn } from '../lib/utils';

const KINDS: { id: LlmProvider['kind']; label: string; hint: string; base: string }[] = [
  { id: 'anthropic', label: 'Anthropic (Claude)', hint: 'Messages API', base: 'https://api.anthropic.com' },
  { id: 'openai', label: 'OpenAI', hint: 'Chat Completions', base: 'https://api.openai.com/v1' },
  { id: 'ollama', label: 'Ollama (local)', hint: 'No key needed', base: 'http://localhost:11434/v1' },
  { id: 'openai-compatible', label: 'OpenAI-compatible', hint: 'OpenRouter, Gemini, LM Studio, vLLM…', base: 'https://openrouter.ai/api/v1' },
];

function Row({ label, children, hint }: { label: string; children: React.ReactNode; hint?: React.ReactNode }) {
  return (
    <label className="flex flex-col gap-1">
      <span className="text-[12px] font-medium text-muted">{label}</span>
      {children}
      {hint && <span className="text-[11.5px] text-muted">{hint}</span>}
    </label>
  );
}

export function useProviders() {
  const [providers, setProviders] = useState<LlmProvider[]>([]);
  const reload = () => api.llmProviderList().then(setProviders).catch(() => setProviders([]));
  useEffect(() => {
    reload();
  }, []);
  return [providers, reload] as const;
}

export function AiProvidersModal({ open, onClose }: { open: boolean; onClose: () => void }) {
  const toast = useToast();
  const dialog = useDialog();
  const [providers, reload] = useProviders();
  const [sel, setSel] = useState<string | null>(null);
  const [draft, setDraft] = useState<LlmProvider | null>(null);
  const [key, setKey] = useState('');
  const [models, setModels] = useState<string[]>([]);
  const [testing, setTesting] = useState(false);

  useEffect(() => {
    if (open) reload();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);
  const current = providers.find(p => p.id === sel) ?? providers[0];
  useEffect(() => {
    setDraft(current ? { ...current } : null);
    setKey('');
    setModels([]);
  }, [current?.id]); // eslint-disable-line react-hooks/exhaustive-deps

  const save = async (next: LlmProvider) => {
    setDraft(next);
    try {
      await api.llmProviderUpdate(next);
      reload();
    } catch (e) {
      toast(errorText(e), 'error');
    }
  };

  const add = async (kind: LlmProvider['kind']) => {
    const label = KINDS.find(k => k.id === kind)!.label.split(' ')[0];
    const p = await api.llmProviderCreate(label, kind);
    await reload();
    setSel(p.id);
  };

  const test = async () => {
    if (!draft) return;
    setTesting(true);
    try {
      const m = await api.llmModels(draft.id);
      setModels(m);
      toast(`Connected — ${m.length} models available`, 'success');
    } catch (e) {
      toast(errorText(e), 'error');
    } finally {
      setTesting(false);
    }
  };

  const kindInfo = KINDS.find(k => k.id === draft?.kind);
  const ks = draft?.keySource;
  return (
    <Modal open={open} onClose={onClose} title={<span className="flex items-center gap-2"><Sparkles className="size-4 text-accent" /> AI providers</span>} width="max-w-4xl">
      <div className="flex h-[62vh]">
        <div className="flex w-60 shrink-0 flex-col border-r border-app bg-subtle p-2">
          {providers.map(p => (
            <button
              key={p.id}
              onClick={() => setSel(p.id)}
              className={cn('mb-0.5 flex flex-col rounded-md px-2.5 py-1.5 text-left', current?.id === p.id ? 'bg-accent-soft' : 'hover:bg-muted')}
            >
              <span className="flex items-center gap-1.5 font-medium">
                {p.name}
                {p.hasKey ? <CheckCircle2 className="size-3 text-emerald-500" /> : <KeyRound className="size-3 text-amber-500" />}
              </span>
              <span className="truncate text-[11.5px] text-muted">{p.defaultModel || p.kind}</span>
            </button>
          ))}
          <div className="mt-2 border-t border-app pt-2 text-[11px] font-semibold tracking-wide text-muted uppercase">Add</div>
          {KINDS.map(k => (
            <button key={k.id} onClick={() => add(k.id)} className="flex items-center gap-1.5 rounded-md px-2.5 py-1.5 text-left text-[12.5px] hover:bg-muted">
              <Plus className="size-3.5 text-muted" /> {k.label}
            </button>
          ))}
        </div>
        <div className="min-w-0 flex-1 overflow-auto p-5">
          {!draft ? (
            <Empty icon={<Sparkles className="size-8" />} title="No AI providers yet">
              Add Anthropic, OpenAI, a local Ollama, or any OpenAI-compatible endpoint. Keys are stored in your OS keychain, never in the database.
            </Empty>
          ) : (
            <div className="flex max-w-xl flex-col gap-4">
              <div className="flex items-center gap-2">
                <Badge tone="violet">{kindInfo?.label}</Badge>
                <span className="text-[12px] text-muted">{kindInfo?.hint}</span>
                <div className="flex-1" />
                <Button
                  size="sm"
                  variant="ghost"
                  onClick={async () => {
                    if (!(await dialog.confirm(`Delete provider "${draft.name}"?`, { danger: true, confirmLabel: 'Delete', message: 'Its stored key is removed from the keychain too.' }))) return;
                    await api.llmProviderDelete(draft.id);
                    setSel(null);
                    reload();
                  }}
                >
                  <Trash2 className="size-3.5" /> Delete
                </Button>
              </div>
              <Row label="Name">
                <Input value={draft.name} onChange={e => setDraft({ ...draft, name: e.target.value })} onBlur={() => save(draft)} />
              </Row>
              <Row label="Kind">
                <Select value={draft.kind} onChange={e => save({ ...draft, kind: e.target.value as LlmProvider['kind'] })}>
                  {KINDS.map(k => (
                    <option key={k.id} value={k.id}>
                      {k.label}
                    </option>
                  ))}
                </Select>
              </Row>
              <Row label="Base URL" hint={draft.kind === 'openai-compatible' ? 'e.g. https://openrouter.ai/api/v1 or https://generativelanguage.googleapis.com/v1beta/openai' : 'Leave empty for the default.'}>
                <Input value={draft.baseUrl} placeholder={kindInfo?.base} onChange={e => setDraft({ ...draft, baseUrl: e.target.value })} onBlur={() => save(draft)} />
              </Row>
              <Row label="Default model">
                <Input
                  list="lsock-models"
                  value={draft.defaultModel}
                  onChange={e => setDraft({ ...draft, defaultModel: e.target.value })}
                  onBlur={() => save(draft)}
                  placeholder={draft.suggestedModels[0]}
                />
                <datalist id="lsock-models">
                  {[...new Set([...models, ...draft.suggestedModels])].map(m => (
                    <option key={m} value={m} />
                  ))}
                </datalist>
              </Row>
              <Row label="API key">
                <Select
                  value={ks?.type}
                  onChange={e => {
                    const t = e.target.value;
                    const next: KeySource = t === 'env' ? { type: 'env', var: 'ANTHROPIC_API_KEY' } : t === 'template' ? { type: 'template', template: '{{ _.api_key }}' } : ({ type: t } as KeySource);
                    save({ ...draft, keySource: next });
                  }}
                >
                  <option value="keychain">Stored in OS keychain</option>
                  <option value="env">Environment variable</option>
                  <option value="template">From environment (template)</option>
                  <option value="none">No key</option>
                </Select>
              </Row>
              {ks?.type === 'keychain' && (
                <div className="flex gap-2">
                  <Input type="password" value={key} onChange={e => setKey(e.target.value)} placeholder={draft.hasKey ? '•••••••• (stored — enter a new key to replace)' : 'Paste API key'} />
                  <Button
                    variant="primary"
                    disabled={!key.trim()}
                    onClick={async () => {
                      await api.llmProviderSetKey(draft.id, key);
                      setKey('');
                      toast('Key saved to keychain', 'success');
                      reload();
                    }}
                  >
                    Save key
                  </Button>
                </div>
              )}
              {ks?.type === 'env' && (
                <Input value={ks.var} onChange={e => setDraft({ ...draft, keySource: { type: 'env', var: e.target.value } })} onBlur={() => save(draft)} className="font-mono" />
              )}
              {ks?.type === 'template' && (
                <Row label="Template" hint="Rendered against the active environment of the request — mark the variable as secret there.">
                  <Input value={ks.template} onChange={e => setDraft({ ...draft, keySource: { type: 'template', template: e.target.value } })} onBlur={() => save(draft)} className="font-mono" />
                </Row>
              )}
              <div className="flex items-center gap-2">
                <Button onClick={test} loading={testing}>
                  {!testing && <Zap className="size-3.5" />} Test connection
                </Button>
                {models.length > 0 && <span className="text-[12px] text-emerald-600">{models.length} models available</span>}
              </div>
            </div>
          )}
        </div>
      </div>
    </Modal>
  );
}
