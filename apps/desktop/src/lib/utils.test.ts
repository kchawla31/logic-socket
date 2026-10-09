import { describe, expect, it } from 'vitest';

import {
  formatBytes,
  formatMs,
  methodShort,
  looksLikeCurl,
  pathParamsInUrl,
  prettyJson,
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
