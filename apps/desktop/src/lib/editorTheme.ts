import { HighlightStyle, syntaxHighlighting } from '@codemirror/language';
import { EditorView } from '@codemirror/view';
import { tags as t } from '@lezer/highlight';

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
  '.cm-panels': { backgroundColor: 'var(--bg-subtle)', color: 'var(--fg)' },
  '.cm-searchMatch': { backgroundColor: 'color-mix(in srgb, var(--accent) 22%, transparent)' },
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

export const editorTheme = [base, syntaxHighlighting(highlight)];
