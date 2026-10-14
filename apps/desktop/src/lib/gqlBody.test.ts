import { describe, expect, it } from 'vitest';

import { parseGqlBody, serializeGqlBody, variablesText } from './gqlBody';

describe('graphql body variables', () => {
  it('keeps a quoted variable in valid JSON', () => {
    const text = serializeGqlBody('query { me { id } }', '{\n  "id": "${user_id}"\n}');
    const body = parseGqlBody(text);
    expect(body.query).toBe('query { me { id } }');
    expect(body.variables).toEqual({ id: '${user_id}' });
  });

  it('keeps an unquoted variable so the body can render it', () => {
    const raw = '{ "id": ${user_id} }';
    const text = serializeGqlBody('query { me { id } }', raw, 'Me');
    expect(JSON.parse.bind(JSON, text)).toThrow();
    const body = parseGqlBody(text);
    expect(body.query).toBe('query { me { id } }');
    expect(body.operationName).toBe('Me');
    expect(variablesText(body.variables)).toBe(raw);
    expect(serializeGqlBody(body.query, variablesText(body.variables), body.operationName)).toBe(text);
  });

  it('does not treat the word variables inside the query as the variables field', () => {
    const query = 'query { user(q: "variables") { id } }';
    const text = serializeGqlBody(query, '{ "id": ${id} }');
    expect(parseGqlBody(text).query).toBe(query);
    expect(variablesText(parseGqlBody(text).variables)).toBe('{ "id": ${id} }');
  });
});
