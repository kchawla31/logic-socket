// SSE response: one "data:" line per JSON-RPC message
const frames = insomnia.response.text().split('\n').filter(l => l.startsWith('data:')).map(l => JSON.parse(l.slice(5)));
const result = frames.find(f => f.id === 3).result;
insomnia.test(`weather for ${insomnia.iterationData.get('city')}`, () => {
  insomnia.expect(result.isError).to.not.equal(true);
  insomnia.expect(result.content[0].text).to.include(insomnia.iterationData.get('city'));
});
