export interface GqlBody {
  query: string;
  variables?: unknown;
  operationName?: string | null;
}

/** What the variables editor shows. A string is template text that is not JSON yet. */
export function variablesText(variables: unknown): string {
  if (variables == null) return '';
  if (typeof variables === 'string') return variables;
  return JSON.stringify(variables, null, 2);
}

/**
 * GraphQL request body. `variablesText` is kept as written when it contains
 * a `${variable}` and is not valid JSON, so the body renderer can fill it in.
 */
export function serializeGqlBody(query: string, variablesText: string, operationName?: string | null): string {
  const fields = [`"query": ${JSON.stringify(query)}`];
  const trimmed = variablesText.trim();
  if (trimmed) {
    try {
      const parsed = JSON.parse(trimmed) as unknown;
      const emptyObject = parsed !== null && typeof parsed === 'object' && !Array.isArray(parsed) && Object.keys(parsed as object).length === 0;
      if (!emptyObject) fields.push(`"variables": ${JSON.stringify(parsed)}`);
    } catch {
      fields.push(`"variables": ${trimmed}`);
    }
  }
  if (operationName) fields.push(`"operationName": ${JSON.stringify(operationName)}`);
  return `{\n  ${fields.join(',\n  ')}\n}`;
}

export function parseGqlBody(text: string | null | undefined): GqlBody {
  if (!text?.trim()) return { query: '' };
  try {
    const parsed = JSON.parse(text) as unknown;
    if (parsed && typeof parsed === 'object' && !Array.isArray(parsed) && typeof (parsed as GqlBody).query === 'string') {
      return parsed as GqlBody;
    }
  } catch {
    // A variable such as ${id} is not valid JSON until it is rendered.
  }
  return parseLoose(text) ?? { query: text };
}

function readString(src: string, start: number): { value: string; end: number } | null {
  if (src[start] !== '"') return null;
  let value = '';
  let i = start + 1;
  while (i < src.length) {
    const ch = src[i];
    if (ch === '\\') {
      const next = src[i + 1];
      if (next === undefined) return null;
      if (next === 'u') {
        const hex = src.slice(i + 2, i + 6);
        if (!/^[0-9a-fA-F]{4}$/.test(hex)) return null;
        value += String.fromCharCode(parseInt(hex, 16));
        i += 6;
        continue;
      }
      const simple: Record<string, string> = { n: '\n', r: '\r', t: '\t', '"': '"', '\\': '\\', '/': '/' };
      value += simple[next] ?? next;
      i += 2;
      continue;
    }
    if (ch === '"') return { value, end: i + 1 };
    value += ch;
    i += 1;
  }
  return null;
}

function fieldValueStart(src: string, key: string): number | null {
  let i = 0;
  let depth = 0;
  while (i < src.length) {
    const ch = src[i];
    if (ch === '"') {
      const text = readString(src, i);
      if (!text) return null;
      const after = /^\s*:/.exec(src.slice(text.end));
      if (depth === 1 && text.value === key && after) return text.end + after[0].length;
      i = text.end;
      continue;
    }
    if (ch === '{' || ch === '[') depth += 1;
    else if (ch === '}' || ch === ']') depth -= 1;
    i += 1;
  }
  return null;
}

function rawValue(src: string, start: number): string | null {
  let i = start;
  while (i < src.length && /\s/.test(src[i])) i += 1;
  const from = i;
  let depth = 0;
  let inString = false;
  while (i < src.length) {
    const ch = src[i];
    if (inString) {
      if (ch === '\\') {
        i += 2;
        continue;
      }
      if (ch === '"') inString = false;
      i += 1;
      continue;
    }
    if (ch === '"') {
      inString = true;
      i += 1;
      continue;
    }
    if (ch === '{' || ch === '[') depth += 1;
    else if (ch === '}' || ch === ']') {
      if (depth === 0) return src.slice(from, i).trim();
      depth -= 1;
    } else if (ch === ',' && depth === 0) return src.slice(from, i).trim();
    i += 1;
  }
  return null;
}

function parseLoose(text: string): GqlBody | null {
  const queryAt = fieldValueStart(text, 'query');
  if (queryAt == null) return null;
  const queryRaw = rawValue(text, queryAt);
  if (!queryRaw) return null;
  let query: string;
  try {
    const parsed = JSON.parse(queryRaw) as unknown;
    if (typeof parsed !== 'string') return null;
    query = parsed;
  } catch {
    return null;
  }
  let variables: unknown;
  const variablesAt = fieldValueStart(text, 'variables');
  if (variablesAt != null) {
    const raw = rawValue(text, variablesAt);
    if (raw) {
      try {
        variables = JSON.parse(raw) as unknown;
      } catch {
        variables = raw;
      }
    }
  }
  let operationName: string | null | undefined;
  const nameAt = fieldValueStart(text, 'operationName');
  if (nameAt != null) {
    const raw = rawValue(text, nameAt);
    if (raw && raw !== 'null') {
      try {
        const parsed = JSON.parse(raw) as unknown;
        operationName = typeof parsed === 'string' ? parsed : null;
      } catch {
        operationName = null;
      }
    }
  }
  return { query, variables, operationName };
}
