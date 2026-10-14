import { SearchQuery } from '@codemirror/search';
import { EditorState } from '@codemirror/state';
import { describe, expect, it } from 'vitest';

import { jsonEscapePattern, searchHint } from './searchPanel';

describe('jsonEscapePattern', () => {
  it('matches a real newline and the JSON escape \\n', () => {
    const pattern = jsonEscapePattern('\\n');
    expect(pattern).toBe('(?:\\\\n|\\n)');
    const doc = '{\n  "a": "hello\\nworld"\n}';
    const found = doc.match(new RegExp(pattern, 'g'));
    expect(found).toEqual(['\n', '\\n', '\n']);
  });

  it('leaves a doubled backslash as the JSON escape only', () => {
    const pattern = jsonEscapePattern('\\\\n');
    expect(pattern).toBe('\\\\n');
    const doc = '{\n  "a": "hello\\nworld"\n}';
    expect(doc.match(new RegExp(pattern, 'g'))).toEqual(['\\n']);
  });

  it('does not rewrite escapes inside a character class', () => {
    expect(jsonEscapePattern('[\\n]')).toBe('[\\n]');
    expect(jsonEscapePattern('a\\tb')).toBe('a(?:\\\\t|\\t)b');
  });

  it('replaces a line break and the JSON escape together', () => {
    const doc = '{\n  "a": "hello\\nworld"\n}';
    const replaced = doc.replace(new RegExp(jsonEscapePattern('\\n'), 'g'), ' ');
    expect(replaced).toBe('{   "a": "hello world" }');
    const onlyEscape = doc.replace(new RegExp(jsonEscapePattern('\\\\n'), 'g'), ' ');
    expect(onlyEscape).toBe('{\n  "a": "hello world"\n}');
  });

  it('finds both forms through the editor search', () => {
    const doc = '{\n  "a": "hello\\nworld"\n}';
    const state = EditorState.create({ doc });
    const query = new SearchQuery({ search: jsonEscapePattern('\\n'), regexp: true, replace: ' ' });
    const hits: string[] = [];
    const cursor = query.getCursor(state);
    for (let item = cursor.next(); !item.done; item = cursor.next()) {
      hits.push(state.sliceDoc(item.value.from, item.value.to));
    }
    expect(hits.filter(hit => hit === '\n')).toHaveLength(2);
    expect(hits.filter(hit => hit === '\\n')).toHaveLength(1);
    expect(query.valid).toBe(true);
  });
});

describe('searchHint', () => {
  it('explains JSON escapes only when the pattern uses a backslash', () => {
    expect(searchHint('name', true, true, true)).toBeNull();
    expect(searchHint('\\n', true, true, true)).toContain('JSON');
    expect(searchHint('(\\n', true, true, false)).toBe('That regular expression is not valid.');
  });
});
