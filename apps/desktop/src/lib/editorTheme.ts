import { HighlightStyle, syntaxHighlighting } from '@codemirror/language';
import { search } from '@codemirror/search';
import { EditorView } from '@codemirror/view';
import { tags as t } from '@lezer/highlight';

import { GraphiteSearchPanel } from './searchPanel';

/** One editor theme for light and dark: colours come from the CSS theme tokens. */
const base = EditorView.theme({
  '&': { backgroundColor: 'var(--bg)', color: 'var(--fg)' },
  '.cm-content': { caretColor: 'var(--accent)' },
  '.cm-cursor, .cm-dropCursor': { borderLeftColor: 'var(--accent)', borderLeftWidth: '2px' },
  '&.cm-focused .cm-selectionBackground, .cm-selectionBackground, .cm-content ::selection': { backgroundColor: 'var(--accent-soft) !important' },
  '.cm-gutters': { backgroundColor: 'var(--bg)', color: 'var(--fg-muted)', border: 'none' },
  '.cm-lineNumbers .cm-gutterElement': { padding: '0 10px 0 8px', opacity: '0.7' },
  '.cm-activeLine': { backgroundColor: 'color-mix(in srgb, var(--bg-muted) 55%, transparent)' },
  '.cm-activeLineGutter': { backgroundColor: 'transparent', color: 'var(--fg)' },
  '.cm-foldPlaceholder': { backgroundColor: 'var(--bg-muted)', border: 'none', color: 'var(--fg-muted)' },
  '.cm-matchingBracket': { backgroundColor: 'var(--accent-soft)', outline: 'none' },
  '.cm-placeholder': { color: 'var(--fg-muted)' },
  '.cm-tooltip': { backgroundColor: 'var(--bg)', border: '1px solid var(--border)', borderRadius: '10px', boxShadow: 'var(--shadow)', overflow: 'hidden' },
  '.cm-tooltip-autocomplete > ul > li': { padding: '3px 8px !important' },
  '.cm-tooltip-autocomplete > ul > li[aria-selected]': { backgroundColor: 'var(--accent-soft)', color: 'var(--fg)' },
  '.cm-completionDetail': { color: 'var(--fg-muted)', fontStyle: 'normal', marginLeft: '8px' },
  '.cm-completionInfo': { padding: '6px 10px', maxWidth: '320px' },
  '.cm-panels': { backgroundColor: 'var(--bg-subtle) !important', color: 'var(--fg) !important' },
  '.cm-panels-top': { borderBottom: '1px solid var(--border) !important' },
  '.cm-panels-bottom': { borderTop: '1px solid var(--border) !important' },
  '.cm-searchMatch': {
    backgroundColor: 'color-mix(in srgb, var(--accent) 38%, transparent) !important',
    outline: '1px solid color-mix(in srgb, var(--accent) 80%, transparent)',
    borderRadius: '2px',
  },
  '.cm-searchMatch-selected': {
    backgroundColor: 'color-mix(in srgb, var(--accent) 62%, transparent) !important',
    outline: '1px solid var(--accent)',
  },
  '.cm-ls-search': {
    display: 'flex',
    flexWrap: 'wrap',
    alignItems: 'center',
    gap: '6px',
    padding: '8px 10px',
    backgroundColor: 'var(--bg-subtle)',
    color: 'var(--fg)',
    fontFamily: 'var(--font-sans)',
    fontSize: '12.5px',
  },
  '.cm-ls-search-label': {
    display: 'flex',
    alignItems: 'center',
    gap: '6px',
    color: 'var(--fg-muted)',
    fontWeight: '400',
    fontSize: '12.5px',
  },
  '.cm-ls-search-input': {
    height: '28px',
    width: '180px',
    borderRadius: '999px',
    border: '1px solid var(--border)',
    backgroundColor: 'var(--bg)',
    color: 'var(--fg)',
    fontFamily: 'var(--font-mono)',
    fontSize: '12.5px',
    fontWeight: '400',
    padding: '0 12px',
    outline: 'none',
  },
  '.cm-ls-search-input:focus': {
    borderColor: 'var(--accent)',
    boxShadow: '0 0 0 3px var(--accent-soft)',
  },
  '.cm-ls-search-input::placeholder': { color: 'var(--fg-muted)' },
  '.cm-ls-search-btn': {
    height: '28px',
    borderRadius: '999px',
    border: '1px solid var(--border)',
    backgroundColor: 'var(--bg)',
    color: 'var(--fg)',
    fontFamily: 'var(--font-sans)',
    fontSize: '12px',
    fontWeight: '500',
    padding: '0 10px',
    cursor: 'pointer',
  },
  '.cm-ls-search-btn:hover': { backgroundColor: 'var(--bg-muted)' },
  '.cm-ls-search-toggle[aria-pressed=true]': {
    backgroundColor: 'var(--accent-soft)',
    borderColor: 'transparent',
    color: 'var(--accent)',
  },
  '.cm-ls-search-close': { marginLeft: 'auto' },
  '.cm-ls-search-meta': {
    flexBasis: '100%',
    color: 'var(--fg-muted)',
    fontSize: '11.5px',
    lineHeight: '1.4',
  },
  '.cm-ls-search-meta.is-error': { color: '#ef4444' },
  '.cm-ls-var': {
    backgroundColor: 'rgb(16 185 129 / 0.2)',
    color: 'light-dark(#047857, #6ee7b7)',
    borderRadius: '3px',
  },
  '.cm-ls-var-missing': {
    backgroundColor: 'rgb(244 63 94 / 0.18)',
    color: 'light-dark(#be123c, #fda4af)',
    borderRadius: '3px',
  },
  '.cm-ls-tag': {
    backgroundColor: 'rgb(47 107 255 / 0.16)',
    color: 'light-dark(#1d4ed8, #c5d4ff)',
    borderRadius: '3px',
  },
  '.cm-lintRange-error': { backgroundImage: 'none', textDecoration: 'underline wavy #ef4444' },
});

const highlight = HighlightStyle.define([
  { tag: [t.propertyName, t.attributeName], color: 'var(--syn-key)' },
  { tag: [t.string, t.special(t.string), t.regexp], color: 'var(--syn-string)' },
  { tag: [t.number, t.bool, t.null, t.atom], color: 'var(--syn-number)' },
  { tag: [t.keyword, t.operatorKeyword, t.controlKeyword, t.definitionKeyword, t.modifier], color: 'var(--syn-keyword)' },
  { tag: [t.function(t.variableName), t.function(t.propertyName), t.typeName, t.className], color: 'var(--syn-fn)' },
  { tag: [t.comment, t.lineComment, t.blockComment], color: 'var(--syn-comment)', fontStyle: 'italic' },
  { tag: [t.punctuation, t.bracket, t.separator], color: 'var(--fg-muted)' },
  { tag: t.invalid, color: '#ef4444' },
]);

const graphiteSearch = search({ top: true, createPanel: view => new GraphiteSearchPanel(view) });

export const editorTheme = [base, syntaxHighlighting(highlight), graphiteSearch];
