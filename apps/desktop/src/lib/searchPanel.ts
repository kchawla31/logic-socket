import {
  closeSearchPanel,
  findNext,
  findPrevious,
  getSearchQuery,
  replaceAll,
  replaceNext,
  SearchQuery,
  setSearchQuery,
} from '@codemirror/search';
import { EditorState, Facet } from '@codemirror/state';
import { EditorView, type Panel, type ViewUpdate } from '@codemirror/view';

/** JSON editors turn this on so a regex `\n` also matches the two characters in `\n`. */
export const jsonSearchEscapes = Facet.define<boolean, boolean>({
  combine: values => values.some(Boolean),
});

/**
 * In a JSON document the text `\n` is a backslash and an n, while a regexp
 * `\n` matches a line break. Outside a character class, `\n`, `\r`, and `\t`
 * match either one. `\\n` still matches only the two characters.
 */
export function jsonEscapePattern(pattern: string): string {
  let out = '';
  let i = 0;
  let inClass = false;
  while (i < pattern.length) {
    const ch = pattern[i];
    if (ch === '\\' && i + 1 < pattern.length) {
      const next = pattern[i + 1];
      if (next === '\\') {
        out += '\\\\';
        i += 2;
        continue;
      }
      if (!inClass && (next === 'n' || next === 'r' || next === 't')) {
        out += `(?:\\\\${next}|\\${next})`;
        i += 2;
        continue;
      }
      out += `\\${next}`;
      i += 2;
      continue;
    }
    if (ch === '[' && !inClass) inClass = true;
    else if (ch === ']' && inClass) inClass = false;
    out += ch;
    i += 1;
  }
  return out;
}

export function searchHint(find: string, regexp: boolean, jsonEscapes: boolean, valid: boolean): string | null {
  if (regexp && find && !valid) return 'That regular expression is not valid.';
  if (!find.includes('\\')) return null;
  if (regexp && jsonEscapes) return '\\n matches a line break or the \\n characters in JSON. \\\\n matches only those characters.';
  return '\\n matches a line break. \\\\n matches the \\n characters in JSON.';
}

function matchCount(state: EditorState, query: SearchQuery): number {
  if (!query.valid || !query.search) return 0;
  let count = 0;
  const cursor = query.getCursor(state);
  for (let item = cursor.next(); !item.done; item = cursor.next()) {
    count += 1;
    if (count >= 999) return 999;
  }
  return count;
}

function button(label: string, title: string, onClick: () => void): HTMLButtonElement {
  const node = document.createElement('button');
  node.type = 'button';
  node.className = 'cm-ls-search-btn';
  node.textContent = label;
  node.title = title;
  node.addEventListener('click', onClick);
  return node;
}

function toggle(label: string, title: string, pressed: boolean, onClick: () => void): HTMLButtonElement {
  const node = document.createElement('button');
  node.type = 'button';
  node.className = 'cm-ls-search-btn cm-ls-search-toggle';
  node.textContent = label;
  node.title = title;
  node.setAttribute('aria-pressed', pressed ? 'true' : 'false');
  node.addEventListener('click', () => {
    node.setAttribute('aria-pressed', node.getAttribute('aria-pressed') === 'true' ? 'false' : 'true');
    onClick();
  });
  return node;
}

/** Find and replace bar styled with the Graphite tokens. */
export class GraphiteSearchPanel implements Panel {
  dom: HTMLElement;
  private view: EditorView;
  private query: SearchQuery;
  private searchField: HTMLInputElement;
  private replaceField: HTMLInputElement;
  private caseBtn: HTMLButtonElement;
  private reBtn: HTMLButtonElement;
  private wordBtn: HTMLButtonElement;
  private meta: HTMLElement;
  private commit = () => this.apply();

  constructor(view: EditorView) {
    this.view = view;
    this.query = getSearchQuery(view.state);
    const readOnly = view.state.readOnly;

    this.searchField = document.createElement('input');
    this.searchField.type = 'text';
    this.searchField.className = 'cm-ls-search-input';
    this.searchField.placeholder = '\\n';
    this.searchField.setAttribute('aria-label', 'Find');
    this.searchField.setAttribute('main-field', 'true');
    this.searchField.spellcheck = false;
    this.searchField.value = this.query.search;
    this.searchField.addEventListener('input', this.commit);
    this.searchField.addEventListener('keyup', this.commit);

    this.replaceField = document.createElement('input');
    this.replaceField.type = 'text';
    this.replaceField.className = 'cm-ls-search-input';
    this.replaceField.placeholder = '\\n';
    this.replaceField.setAttribute('aria-label', 'Replace');
    this.replaceField.spellcheck = false;
    this.replaceField.value = this.query.replace;
    this.replaceField.addEventListener('input', this.commit);
    this.replaceField.addEventListener('keyup', this.commit);

    this.caseBtn = toggle('Case', 'Match case', this.query.caseSensitive, this.commit);
    this.reBtn = toggle('Regex', 'Regular expression', this.query.regexp, this.commit);
    this.wordBtn = toggle('Word', 'Whole word', this.query.wholeWord, this.commit);

    const findLabel = document.createElement('label');
    findLabel.className = 'cm-ls-search-label';
    findLabel.append('Find', this.searchField);
    const replaceLabel = document.createElement('label');
    replaceLabel.className = 'cm-ls-search-label';
    replaceLabel.append('Replace', this.replaceField);

    this.meta = document.createElement('div');
    this.meta.className = 'cm-ls-search-meta';

    const bar = document.createElement('div');
    bar.className = 'cm-ls-search';
    bar.addEventListener('keydown', event => this.onKeyDown(event));
    bar.append(
      findLabel,
      this.caseBtn,
      this.reBtn,
      this.wordBtn,
      button('Next', 'Next match', () => findNext(view)),
      button('Previous', 'Previous match', () => findPrevious(view)),
    );
    if (!readOnly) {
      bar.append(
        replaceLabel,
        button('Replace', 'Replace this match', () => replaceNext(view)),
        button('All', 'Replace every match', () => replaceAll(view)),
      );
    }
    const close = button('Close', 'Close find and replace', () => closeSearchPanel(view));
    close.classList.add('cm-ls-search-close');
    close.setAttribute('aria-label', 'Close find and replace');
    bar.append(close, this.meta);
    this.dom = bar;
    this.refreshMeta();
  }

  private jsonEscapes(): boolean {
    return this.view.state.facet(jsonSearchEscapes);
  }

  private pattern(): string {
    const typed = this.searchField.value;
    return this.reBtn.getAttribute('aria-pressed') === 'true' && this.jsonEscapes() ? jsonEscapePattern(typed) : typed;
  }

  private apply() {
    const regexp = this.reBtn.getAttribute('aria-pressed') === 'true';
    const query = new SearchQuery({
      search: this.pattern(),
      caseSensitive: this.caseBtn.getAttribute('aria-pressed') === 'true',
      regexp,
      wholeWord: this.wordBtn.getAttribute('aria-pressed') === 'true',
      replace: this.replaceField.value,
    });
    if (!query.eq(this.query)) {
      this.query = query;
      this.view.dispatch({ effects: setSearchQuery.of(query) });
    }
    this.refreshMeta();
  }

  private refreshMeta() {
    const regexp = this.reBtn.getAttribute('aria-pressed') === 'true';
    const query = getSearchQuery(this.view.state);
    const current = query.search ? query : this.query;
    const valid = !regexp || !this.searchField.value || current.valid;
    const hint = searchHint(this.searchField.value, regexp, this.jsonEscapes(), valid);
    const count = matchCount(this.view.state, current);
    const found = !this.searchField.value ? '' : count >= 999 ? '999+ found' : count === 1 ? '1 found' : `${count} found`;
    this.meta.classList.toggle('is-error', !valid);
    this.meta.textContent = [found, hint].filter(Boolean).join(' · ');
  }

  private onKeyDown(event: KeyboardEvent) {
    if (event.key === 'Enter' && event.target === this.searchField) {
      event.preventDefault();
      (event.shiftKey ? findPrevious : findNext)(this.view);
    } else if (event.key === 'Enter' && event.target === this.replaceField) {
      event.preventDefault();
      replaceNext(this.view);
    } else if (event.key === 'Escape') {
      event.preventDefault();
      closeSearchPanel(this.view);
    }
  }

  update(update: ViewUpdate) {
    for (const tr of update.transactions) {
      for (const effect of tr.effects) {
        if (effect.is(setSearchQuery) && !effect.value.eq(this.query)) this.setQuery(effect.value);
      }
    }
    if (update.docChanged) this.refreshMeta();
  }

  setQuery(query: SearchQuery) {
    this.query = query;
    const rewritten = this.reBtn.getAttribute('aria-pressed') === 'true' && this.jsonEscapes() ? jsonEscapePattern(this.searchField.value) : this.searchField.value;
    if (query.search !== rewritten) this.searchField.value = query.search;
    this.replaceField.value = query.replace;
    this.caseBtn.setAttribute('aria-pressed', query.caseSensitive ? 'true' : 'false');
    this.reBtn.setAttribute('aria-pressed', query.regexp ? 'true' : 'false');
    this.wordBtn.setAttribute('aria-pressed', query.wholeWord ? 'true' : 'false');
    this.refreshMeta();
  }

  mount() {
    this.searchField.select();
  }

  get top() {
    return true;
  }
}
