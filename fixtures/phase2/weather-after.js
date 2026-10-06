// SSE response: one "data:" line per JSON-RPC message
const frames = ls.response.text().split('\n').filter(l => l.startsWith('data:')).map(l => JSON.parse(l.slice(5)));
const result = frames.find(f => f.id === 3).result;
ls.test(`weather for ${ls.iterationData.get('city')}`, () => {
  ls.expect(result.isError).to.not.equal(true);
  ls.expect(result.content[0].text).to.include(ls.iterationData.get('city'));
});
