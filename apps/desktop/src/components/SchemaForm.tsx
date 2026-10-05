// Form generated from a JSON Schema (MCP tool inputSchema). Unsupported
// shapes fall back to a JSON text field so every schema stays editable.
import { Toggle } from './ui';
import { cn } from '../lib/utils';

type Schema = Record<string, unknown>;

function resolve(root: Schema, s: Schema, depth = 0): Schema {
  const ref = s.$ref as string | undefined;
  if (!ref || depth > 8 || !ref.startsWith('#/')) return s;
  const target = ref
    .slice(2)
    .split('/')
    .reduce<unknown>((o, k) => (o && typeof o === 'object' ? (o as Schema)[k] : undefined), root) as Schema | undefined;
  if (!target) return s;
  const { $ref: _ignored, ...rest } = s;
  return resolve(root, { ...target, ...rest }, depth + 1);
}

function primaryType(s: Schema): string {
  const t = s.type;
  if (typeof t === 'string') return t;
  if (Array.isArray(t)) return (t.find(x => x !== 'null') as string) ?? 'string';
  const alt = (s.anyOf || s.oneOf) as Schema[] | undefined;
  if (alt) {
    const nonNull = alt.find(a => a.type !== 'null');
    if (nonNull) return primaryType(nonNull);
  }
  if (s.properties) return 'object';
  if (s.enum) return 'string';
  return 'any';
}

export function setPath(obj: unknown, key: string, value: unknown): Record<string, unknown> {
  const base = obj && typeof obj === 'object' && !Array.isArray(obj) ? { ...(obj as Record<string, unknown>) } : {};
  if (value === undefined) delete base[key];
  else base[key] = value;
  return base;
}

function Field({
  root,
  name,
  schema: raw,
  value,
  required,
  onChange,
  depth,
}: {
  root: Schema;
  name: string;
  schema: Schema;
  value: unknown;
  required: boolean;
  onChange: (v: unknown) => void;
  depth: number;
}) {
  const schema = resolve(root, raw);
  const type = primaryType(schema);
  const enumValues = (schema.enum as unknown[]) ?? ((schema.oneOf as Schema[] | undefined)?.every(o => 'const' in o) ? (schema.oneOf as Schema[]).map(o => o.const) : null);
  const desc = schema.description as string | undefined;
  const label = (
    <div className="flex items-baseline gap-1.5">
      <span className="font-mono text-[12.5px] font-medium">{name}</span>
      {required && <span className="text-[11px] text-amber-600 dark:text-amber-400">required</span>}
      <span className="font-mono text-[11px] text-muted">{type}</span>
    </div>
  );

  let control: React.ReactNode;
  const inputCls = 'h-8 w-full rounded-md border border-app bg-app px-2.5 font-mono text-[12.5px] outline-none focus:border-accent';
  if (enumValues) {
    control = (
      <select
        className={inputCls}
        value={value === undefined ? '' : JSON.stringify(value)}
        onChange={e => onChange(e.target.value === '' ? undefined : JSON.parse(e.target.value))}
      >
        <option value="">{required ? 'Select…' : '(not set)'}</option>
        {enumValues.map(v => (
          <option key={JSON.stringify(v)} value={JSON.stringify(v)}>
            {typeof v === 'string' ? v : JSON.stringify(v)}
          </option>
        ))}
      </select>
    );
  } else if (type === 'boolean') {
    control = <Toggle checked={value === true} onChange={v => onChange(v)} label={value === undefined ? '(not set)' : String(value)} />;
  } else if (type === 'integer' || type === 'number') {
    control = (
      <input
        type="number"
        className={inputCls}
        value={value === undefined || value === null ? '' : String(value)}
        step={type === 'integer' ? 1 : 'any'}
        placeholder={schema.default !== undefined ? `default: ${schema.default}` : ''}
        onChange={e => onChange(e.target.value === '' ? undefined : Number(e.target.value))}
      />
    );
  } else if (type === 'string') {
    const long = (schema.maxLength as number | undefined) === undefined && /body|text|content|description|query|code|markdown/i.test(name);
    control = long ? (
      <textarea
        className={cn(inputCls, 'h-20 py-1.5')}
        value={(value as string) ?? ''}
        placeholder={(schema.default as string) ?? ''}
        onChange={e => onChange(e.target.value === '' && !required ? undefined : e.target.value)}
      />
    ) : (
      <input
        className={inputCls}
        value={(value as string) ?? ''}
        placeholder={(schema.default as string) ?? (schema.format as string) ?? ''}
        onChange={e => onChange(e.target.value === '' && !required ? undefined : e.target.value)}
      />
    );
  } else if (type === 'object' && schema.properties && depth < 4) {
    return (
      <fieldset className="rounded-md border border-app p-2.5">
        <legend className="px-1">{label}</legend>
        {desc && <div className="mb-2 text-[12px] text-muted">{desc}</div>}
        <SchemaForm
          root={root}
          schema={schema}
          value={value}
          onChange={v => onChange(Object.keys(v as object).length ? v : undefined)}
          depth={depth + 1}
        />
      </fieldset>
    );
  } else if (type === 'array' && schema.items && ['string', 'number', 'integer'].includes(primaryType(resolve(root, schema.items as Schema)))) {
    const itemType = primaryType(resolve(root, schema.items as Schema));
    const arr = Array.isArray(value) ? value : [];
    control = (
      <input
        className={inputCls}
        placeholder="comma-separated values"
        value={arr.join(', ')}
        onChange={e => {
          const parts = e.target.value
            .split(',')
            .map(s => s.trim())
            .filter(Boolean);
          onChange(parts.length ? parts.map(p => (itemType === 'string' ? p : Number(p))) : undefined);
        }}
      />
    );
  } else {
    control = (
      <JsonField value={value} onChange={onChange} />
    );
  }
  return (
    <div className="flex flex-col gap-1">
      {label}
      {control}
      {desc && <div className="text-[12px] text-muted">{desc}</div>}
    </div>
  );
}

function JsonField({ value, onChange }: { value: unknown; onChange: (v: unknown) => void }) {
  const text = value === undefined ? '' : JSON.stringify(value);
  return (
    <input
      className="h-8 w-full rounded-md border border-app bg-app px-2.5 font-mono text-[12.5px] outline-none focus:border-accent"
      placeholder="JSON value"
      defaultValue={text}
      onBlur={e => {
        const t = e.target.value.trim();
        if (!t) return onChange(undefined);
        try {
          onChange(JSON.parse(t));
        } catch {
          onChange(t);
        }
      }}
    />
  );
}

export function SchemaForm({
  root,
  schema,
  value,
  onChange,
  depth = 0,
}: {
  root: Schema;
  schema: Schema;
  value: unknown;
  onChange: (v: unknown) => void;
  depth?: number;
}) {
  const s = resolve(root, schema);
  const props = (s.properties as Record<string, Schema>) ?? {};
  const required = new Set((s.required as string[]) ?? []);
  const keys = Object.keys(props).sort((a, b) => Number(required.has(b)) - Number(required.has(a)));
  if (keys.length === 0) return <div className="text-[12.5px] text-muted">This tool takes no arguments.</div>;
  const obj = (value && typeof value === 'object' ? value : {}) as Record<string, unknown>;
  return (
    <div className="flex flex-col gap-3">
      {keys.map(k => (
        <Field
          key={k}
          root={root}
          name={k}
          schema={props[k]}
          required={required.has(k)}
          value={obj[k]}
          depth={depth}
          onChange={v => onChange(setPath(obj, k, v))}
        />
      ))}
    </div>
  );
}
