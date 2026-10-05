import { Select, Toggle } from '../components/ui';
import { VarInput } from '../components/VarInput';
import type { Auth } from '../lib/api';

const LABELS: Record<Auth['type'], string> = {
  inherit: 'Inherit from parent folder',
  none: 'No auth',
  basic: 'Basic',
  bearer: 'Bearer token',
  apikey: 'API key',
};

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <label className="grid grid-cols-[140px_1fr] items-center gap-3">
      <span className="text-muted">{label}</span>
      {children}
    </label>
  );
}

export function AuthEditor({
  auth,
  onChange,
  contextId,
  allowInherit = true,
}: {
  auth: Auth;
  onChange: (a: Auth) => void;
  contextId: string;
  allowInherit?: boolean;
}) {
  const setType = (t: Auth['type']) => {
    switch (t) {
      case 'basic':
        return onChange({ type: 'basic', username: '', password: '' });
      case 'bearer':
        return onChange({ type: 'bearer', token: '', prefix: null });
      case 'apikey':
        return onChange({ type: 'apikey', key: '', value: '', addTo: 'header' });
      default:
        return onChange({ type: t } as Auth);
    }
  };
  const disabled = 'disabled' in auth && !!auth.disabled;
  return (
    <div className="flex max-w-2xl flex-col gap-3 p-4">
      <Field label="Type">
        <Select value={auth.type} onChange={e => setType(e.target.value as Auth['type'])} className="w-64">
          {(Object.keys(LABELS) as Auth['type'][])
            .filter(t => allowInherit || t !== 'inherit')
            .map(t => (
              <option key={t} value={t}>
                {LABELS[t]}
              </option>
            ))}
        </Select>
      </Field>
      {auth.type === 'inherit' && (
        <p className="text-[12.5px] text-muted">
          Uses the auth of the nearest parent folder that defines one. If none does, no auth is sent.
        </p>
      )}
      {auth.type === 'basic' && (
        <>
          <Field label="Username">
            <VarInput contextId={contextId} value={auth.username} onChange={v => onChange({ ...auth, username: v })} />
          </Field>
          <Field label="Password">
            <VarInput contextId={contextId} value={auth.password} onChange={v => onChange({ ...auth, password: v })} />
          </Field>
        </>
      )}
      {auth.type === 'bearer' && (
        <>
          <Field label="Token">
            <VarInput contextId={contextId} value={auth.token} onChange={v => onChange({ ...auth, token: v })} placeholder="{{ _.token }}" />
          </Field>
          <Field label="Prefix">
            <VarInput contextId={contextId} value={auth.prefix ?? ''} onChange={v => onChange({ ...auth, prefix: v || null })} placeholder="Bearer" />
          </Field>
        </>
      )}
      {auth.type === 'apikey' && (
        <>
          <Field label="Key">
            <VarInput contextId={contextId} value={auth.key} onChange={v => onChange({ ...auth, key: v })} placeholder="X-API-Key" />
          </Field>
          <Field label="Value">
            <VarInput contextId={contextId} value={auth.value} onChange={v => onChange({ ...auth, value: v })} />
          </Field>
          <Field label="Add to">
            <Select value={auth.addTo ?? 'header'} onChange={e => onChange({ ...auth, addTo: e.target.value })} className="w-64">
              <option value="header">Header</option>
              <option value="queryParams">Query parameter</option>
              <option value="cookie">Cookie</option>
            </Select>
          </Field>
        </>
      )}
      {auth.type !== 'inherit' && auth.type !== 'none' && (
        <Field label="Enabled">
          <Toggle checked={!disabled} onChange={v => onChange({ ...auth, disabled: !v } as Auth)} />
        </Field>
      )}
    </div>
  );
}
