import { autocompletion, type Completion, type CompletionContext, type CompletionResult } from '@codemirror/autocomplete';
import { type Extension, Facet, RangeSetBuilder } from '@codemirror/state';
import { Decoration, type DecorationSet, EditorView, ViewPlugin, type ViewUpdate } from '@codemirror/view';

import type { VarInfo } from './api';
import { variableDefined } from './utils';

const varsFacet = Facet.define<readonly VarInfo[], readonly VarInfo[]>({
  combine: lists => lists.flat(),
});

const definedMark = Decoration.mark({ class: 'cm-ls-var' });
const missingMark = Decoration.mark({ class: 'cm-ls-var-missing' });
const tagMark = Decoration.mark({ class: 'cm-ls-tag' });

function decorations(view: EditorView): DecorationSet {
  const names = new Set(view.state.facet(varsFacet).map(v => v.name));
  const builder = new RangeSetBuilder<Decoration>();
  const text = view.state.doc.toString();
  const re = /\$\{\s*([^}]*?)\s*\}|\{%\s*([^%]*?)\s*%\}/g;
  let match: RegExpExecArray | null;
  while ((match = re.exec(text))) {
    const from = match.index;
    const to = from + match[0].length;
    if (match[1] !== undefined) {
      const raw = match[1].trim().replace(/^_\./, '');
      const bracket = raw.match(/^_\[['"]([\s\S]*)['"]\]$/);
      const name = (bracket ? bracket[1] : raw).split('|')[0].trim();
      builder.add(from, to, variableDefined(names, name) ? definedMark : missingMark);
    } else {
      builder.add(from, to, tagMark);
    }
  }
  return builder.finish();
}

const variableHighlight = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;
    constructor(view: EditorView) {
      this.decorations = decorations(view);
    }
    update(update: ViewUpdate) {
      if (update.docChanged || update.startState.facet(varsFacet) !== update.state.facet(varsFacet)) {
        this.decorations = decorations(update.view);
      }
    }
  },
  { decorations: plugin => plugin.decorations },
);

function sourceRank(source: string): number {
  if (source.startsWith('Folder:')) return 2;
  if (source.startsWith('Global:')) return 1;
  if (source === 'Base Environment' || !source) return 3;
  return 0;
}

function preview(value: unknown): string {
  const text = typeof value === 'string' ? value : JSON.stringify(value);
  return text.length > 60 ? `${text.slice(0, 60)}…` : text;
}

function variableCompletions(context: CompletionContext): CompletionResult | null {
  const before = context.matchBefore(/\$\{[\w.-]*/);
  if (!before || (before.from === before.to && !context.explicit)) return null;
  const typed = before.text.slice(2).toLowerCase();
  const options: Completion[] = context.state
    .facet(varsFacet)
    .filter(v => v.name.toLowerCase().includes(typed))
    .sort((a, b) => {
      const as = a.name.toLowerCase().startsWith(typed) ? 0 : 1;
      const bs = b.name.toLowerCase().startsWith(typed) ? 0 : 1;
      if (as !== bs) return as - bs;
      const ar = sourceRank(a.source);
      const br = sourceRank(b.source);
      if (ar !== br) return ar - br;
      return a.name.localeCompare(b.name);
    })
    .slice(0, 30)
    .map(v => ({
      label: v.name,
      detail: v.source,
      info: preview(v.value),
      apply: (view, _completion, from, to) => {
        const insert = `\${${v.name}}`;
        const end = view.state.sliceDoc(to, to + 1) === '}' ? to + 1 : to;
        view.dispatch({ changes: { from, to: end, insert }, selection: { anchor: from + insert.length } });
      },
    }));
  if (!options.length) return null;
  return { from: before.from, options, validFor: /\$\{[\w.-]*/ };
}

/** Highlight `${variables}` and offer them after `${` in a code editor. */
export function variableExtensions(vars: readonly VarInfo[]): Extension {
  return [
    varsFacet.of(vars),
    variableHighlight,
    autocompletion({ override: [variableCompletions], activateOnTyping: true }),
  ];
}
