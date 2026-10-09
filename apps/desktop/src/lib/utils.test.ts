import { describe, expect, it } from 'vitest';

import {
  formatBytes,
  formatMs,
  methodShort,
  looksLikeCurl,
  pathParamsInUrl,
  prettyJson,
  authFromText,
  authToText,
  curlToMcp,
  kvFromText,
  kvToText,
  splitQuery,
  statusTone,
  tokenizeTemplate,
} from './utils';

describe('tokenizeTemplate', () => {
  it('splits variables and tags', () => {
    expect(tokenizeTemplate("{{ _.base }}/users/{{id}}?x={% uuid 'v4' %}")).toEqual([
      { text: '{{ _.base }}', kind: 'var', name: 'base' },
      { text: '/users/', kind: 'text' },
      { text: '{{id}}', kind: 'var', name: 'id' },
      { text: '?x=', kind: 'text' },
      { text: "{% uuid 'v4' %}", kind: 'tag', name: 'uuid' },
    ]);
  });
  it('keeps plain text and strips filters', () => {
    expect(tokenizeTemplate('plain')).toEqual([{ text: 'plain', kind: 'text' }]);
    expect(tokenizeTemplate('{{ name | upper }}')[0].name).toBe('name');
  });
});

describe('looksLikeCurl', () => {
  it('recognises a pasted curl command', () => {
    expect(looksLikeCurl('curl https://ex.test')).toBe(true);
    expect(looksLikeCurl('  CURL -X POST https://ex.test')).toBe(true);
    expect(looksLikeCurl('$ curl https://ex.test')).toBe(true);
    expect(looksLikeCurl('> curl.exe -XPUT https://ex.test')).toBe(true);
    expect(looksLikeCurl("% curl'https://ex.test'")).toBe(true);
    expect(looksLikeCurl('# curl https://ex.test')).toBe(true);
    expect(looksLikeCurl('curl"https://ex.test"')).toBe(true);
    expect(looksLikeCurl('\uFEFFcurl https://ex.test')).toBe(true);
    expect(looksLikeCurl('```bash\ncurl https://ex.test\n```')).toBe(true);
    expect(looksLikeCurl('curl -X POST ^\n  https://ex.test')).toBe(true);
    expect(looksLikeCurl('CuRl.ExE -XPUT https://ex.test')).toBe(true);
    expect(looksLikeCurl('kapil@mac ~ % curl https://ex.test')).toBe(true);
    expect(looksLikeCurl('❯ curl https://ex.test')).toBe(true);
    expect(looksLikeCurl('(venv) $ curl https://ex.test')).toBe(true);
  });
  it('leaves urls and other commands alone', () => {
    expect(looksLikeCurl('')).toBe(false);
    expect(looksLikeCurl('https://curl.se')).toBe(false);
    expect(looksLikeCurl('curling https://ex.test')).toBe(false);
    expect(looksLikeCurl('wget https://ex.test')).toBe(false);
    expect(looksLikeCurl('# setup')).toBe(false);
    expect(looksLikeCurl('curl')).toBe(false);
    expect(looksLikeCurl('curl: https://ex.test')).toBe(false);
    expect(looksLikeCurl('https://example.com/curl https://ex.test')).toBe(false);
  });
});

describe('url helpers', () => {
  it('finds path params once', () => {
    expect(pathParamsInUrl('http://x/users/:id/posts/:pid/:id')).toEqual(['id', 'pid']);
  });
  it('splits query strings but not templated ones', () => {
    expect(splitQuery('http://x/a?q=a%20b&n=1')).toEqual({
      base: 'http://x/a',
      params: [
        { name: 'q', value: 'a b' },
        { name: 'n', value: '1' },
      ],
    });
    expect(splitQuery('http://x/a?q={{ v }}').params).toEqual([]);
  });
});

describe('formatting', () => {
  it('formats sizes and durations', () => {
    expect(formatBytes(0)).toBe('0 B');
    expect(formatBytes(512)).toBe('512 B');
    expect(formatBytes(2048)).toBe('2.0 KB');
    expect(formatMs(0.5)).toBe('0.50 ms');
    expect(formatMs(1500)).toBe('1.50 s');
    expect(prettyJson('{"a":1}')).toBe('{\n  "a": 1\n}');
    expect(prettyJson('nope')).toBeNull();
  });
  it('colors status classes and shortens method names', () => {
    expect(statusTone(0)).toContain('rose');
    expect(statusTone(201)).toContain('emerald');
    expect(statusTone(301)).toContain('sky');
    expect(statusTone(404)).toContain('amber');
    expect(statusTone(500)).toContain('rose');
    expect(methodShort('delete')).toBe('DEL');
    expect(methodShort('options')).toBe('OPT');
    expect(methodShort('patch')).toBe('PATCH');
  });
});

describe('url edge cases', () => {
  it('decodes plus as a space in query values', () => {
    expect(splitQuery('http://x/a?q=a+b').params).toEqual([{ name: 'q', value: 'a b' }]);
  });

  it('keeps the fragment on the URL and out of the query value on the URL and out of the query value', () => {
    expect(splitQuery('http://x/a?q=1#section')).toEqual({
      base: 'http://x/a#section',
      params: [{ name: 'q', value: '1' }],
    });
  });

  it('takes path params from the path only from the path only', () => {
    expect(pathParamsInUrl('http://x/users/:id?next=/:home#/:frag')).toEqual(['id']);
  });
});

describe('bulk edit', () => {
  it('round-trips rows, with // for disabled ones', () => {
    const rows = [
      { name: 'Accept', value: 'application/json' },
      { name: 'X-Off', value: '1', disabled: true },
      { name: 'Authorization', value: 'Bearer {{ _.token }}' },
    ];
    const text = kvToText(rows);
    expect(text).toBe('Accept: application/json\n// X-Off: 1\nAuthorization: Bearer {{ _.token }}');
    expect(kvFromText(text)).toEqual(rows);
  });

  it('parses a Postman-style paste, blank lines, = pairs, and colons in values', () => {
    const text = 'Content-Type: application/json\n\n  //Cache-Control:no-cache\nredirect=https://x.io/cb?a=1\nReferer: https://x.io:8443/a\nflag';
    expect(kvFromText(text)).toEqual([
      { name: 'Content-Type', value: 'application/json' },
      { name: 'Cache-Control', value: 'no-cache', disabled: true },
      { name: 'redirect', value: 'https://x.io/cb?a=1' },
      { name: 'Referer', value: 'https://x.io:8443/a' },
      { name: 'flag', value: '' },
    ]);
  });

  it('keeps ids and descriptions of rows that are still there, including duplicates', () => {
    const prev = [
      { id: 'a', name: 'X', value: '1', description: 'first' },
      { id: 'b', name: 'X', value: '2' },
      { id: 'c', name: 'Gone', value: '' },
    ];
    expect(kvFromText('X: 10\nX: 20\nNew: 3', prev)).toEqual([
      { id: 'a', description: 'first', name: 'X', value: '10' },
      { id: 'b', name: 'X', value: '20' },
      { name: 'New', value: '3' },
    ]);
  });

  const TYPES = ['inherit', 'none', 'basic', 'bearer', 'apikey'] as const;
  const defaults = (t: string) =>
    (t === 'basic'
      ? { type: 'basic', username: '', password: '' }
      : t === 'bearer'
        ? { type: 'bearer', token: '', prefix: null }
        : { type: t }) as never;

  it('round-trips auth settings', () => {
    const auth = { type: 'basic', username: 'ada', password: 'p:w', disabled: true } as const;
    const text = authToText(auth);
    expect(text).toBe('type: basic\nusername: ada\npassword: p:w\ndisabled: true');
    expect(authFromText(text, auth, defaults, TYPES)).toEqual({ auth, ignored: [] });
  });

  it('switches type from a pasted block and reports unknown fields', () => {
    const r = authFromText('type: bearer\ntoken: {{ _.t }}\nprefix:\nscope: x', { type: 'inherit' }, defaults, TYPES);
    expect(r.auth).toEqual({ type: 'bearer', token: '{{ _.t }}', prefix: null });
    expect(r.ignored).toEqual(['scope']);
  });

  it('edits fields in place when the type is unchanged, and ignores an unknown type', () => {
    const cur = { type: 'basic', username: 'a', password: 'b' } as const;
    const r = authFromText('type: kerberos\npassword: c', cur, defaults, TYPES);
    expect(r.auth).toEqual({ type: 'basic', username: 'a', password: 'c' });
    expect(r.ignored).toEqual(['type']);
  });
});

describe('curlToMcp', () => {
  const req = (over: object) =>
    ({
      name: '',
      description: '',
      method: 'POST',
      url: 'https://mcp.example.com/mcp',
      parameters: [],
      pathParameters: [],
      headers: [],
      body: {},
      authentication: { type: 'inherit' },
      ...over,
    }) as never;

  it('keeps URL, query, and custom headers; skips headers the MCP client sets', () => {
    const c = curlToMcp(
      req({
        parameters: [
          { name: 'tenant', value: '{{ _.tenant }}' },
          { name: 'q', value: 'a b&c' },
        ],
        headers: [
          { name: 'Content-Type', value: 'application/json' },
          { name: 'Accept', value: 'application/json, text/event-stream' },
          { name: 'X-Api-Version', value: '2' },
        ],
        authentication: { type: 'bearer', token: '$TOKEN', prefix: null },
        body: { mimeType: 'application/json', text: '{"jsonrpc":"2.0"}' },
      }),
    );
    expect(c.url).toBe('https://mcp.example.com/mcp?tenant={{ _.tenant }}&q=a%20b%26c');
    expect(c.headers).toEqual([{ name: 'X-Api-Version', value: '2' }]);
    expect(c.authentication).toEqual({ type: 'bearer', token: '$TOKEN', prefix: null });
    expect(c.dropped).toEqual(['Content-Type', 'Accept', 'request body']);
  });

  it('turns Basic auth into an Authorization header, since MCP auth is bearer only', () => {
    const c = curlToMcp(req({ authentication: { type: 'basic', username: 'ada', password: 'pä' } }));
    expect(c.authentication).toEqual({ type: 'none' });
    expect(c.headers).toEqual([{ name: 'Authorization', value: `Basic ${btoa('ada:p\u00c3\u00a4')}` }]);
  });
});
