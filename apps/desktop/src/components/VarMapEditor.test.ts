import { describe, expect, it } from 'vitest';

import { applyJsonText, buildVars, commitValue, formatVar, plainObject } from './VarMapEditor';

describe('formatVar', () => {
  it('shows strings raw and other json compact', () => {
    expect(formatVar('https://httpbin.org')).toBe('https://httpbin.org');
    expect(formatVar('${base_url}/v1')).toBe('${base_url}/v1');
    expect(formatVar({ id: 'u1' })).toBe('{"id":"u1"}');
    expect(formatVar(1)).toBe('1');
    expect(formatVar(null)).toBe('null');
  });
  it('does not show a sealed secret', () => {
    expect(formatVar('vault:v1:abc')).toBe('');
  });
});

describe('commitValue', () => {
  it('keeps a typed string as a string', () => {
    expect(commitValue('1', 'old', false)).toBe('1');
    expect(commitValue('{"a":1}', 's', false)).toBe('{"a":1}');
  });
  it('round-trips json values and keeps the previous value while the text is invalid', () => {
    expect(commitValue('{"id":"u1"}', { id: 'old' }, false)).toEqual({ id: 'u1' });
    expect(commitValue('2', 1, false)).toBe(2);
    expect(commitValue('{', { a: 1 }, false)).toEqual({ a: 1 });
  });
  it('leaves a masked secret stored as it is', () => {
    expect(commitValue('', 'vault:v1:abc', true)).toBe('vault:v1:abc');
  });
});

describe('applyJsonText', () => {
  it('replaces plain variables and keeps secrets', () => {
    const data = { base_url: 'https://x', token: 'vault:v1:abc' };
    expect(applyJsonText(data, ['token'], '{\n  "base_url": "https://y",\n  "n": 1\n}')).toEqual({
      token: 'vault:v1:abc',
      base_url: 'https://y',
      n: 1,
    });
    expect(plainObject(data, ['token'])).toEqual({ base_url: 'https://x' });
  });
  it('rejects a list', () => {
    expect(() => applyJsonText({}, [], '[1]')).toThrow(/JSON object/);
  });
});

describe('buildVars', () => {
  it('skips the blank row and keeps a masked secret', () => {
    expect(
      buildVars([
        { name: 'base_url', text: 'https://y', secret: false, raw: 'https://x', masked: false },
        { name: 'token', text: '', secret: true, raw: 'vault:v1:abc', masked: true },
        { name: '', text: '', secret: false, raw: undefined, masked: false },
      ]),
    ).toEqual({
      data: { base_url: 'https://y', token: 'vault:v1:abc' },
      secretKeys: ['token'],
    });
  });
});
