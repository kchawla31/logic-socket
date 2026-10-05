import { describe, expect, it } from 'vitest';

import { formatBytes, formatMs, pathParamsInUrl, prettyJson, splitQuery, tokenizeTemplate } from './utils';

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
    expect(formatBytes(512)).toBe('512 B');
    expect(formatBytes(2048)).toBe('2.0 KB');
    expect(formatMs(0.5)).toBe('0.50 ms');
    expect(formatMs(1500)).toBe('1.50 s');
    expect(prettyJson('{"a":1}')).toBe('{\n  "a": 1\n}');
    expect(prettyJson('nope')).toBeNull();
  });
});
