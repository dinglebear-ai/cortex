const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
class Element {
  constructor() { this.children = []; this.dataset = {}; this.handlers = {}; this.value = ''; this.text = ''; }
  set textContent(value) { this.text = value; this.children = []; }
  get textContent() { return this.text + this.children.map(child => child.textContent).join(' '); }
  get firstChild() { return this.children[0]; }
  append(...children) { this.children.push(...children); }
  replaceChildren(...children) { this.children = children; }
  removeChild(child) { this.children.splice(this.children.indexOf(child), 1); }
  addEventListener(name, handler) { this.handlers[name] = handler; }
  reset() { this.value = ''; }
  fire(name) { this.handlers[name]({ preventDefault() {} }); }
}
const tick = () => new Promise(resolve => setImmediate(resolve));
function setup() {
  const nodes = new Map(); const requests = [];
  const node = selector => { if (!nodes.has(selector)) nodes.set(selector, new Element()); return nodes.get(selector); };
  vm.runInNewContext(fs.readFileSync(require('node:path').resolve(__dirname, '../../web/app/app.js'), 'utf8'), {
    window: {}, document: { querySelector: node, createElement: () => new Element() },
    FormData: class { constructor(form) { this.form = form; } get() { return this.form.value; } },
    fetch: (url, options) => new Promise((resolve, reject) => requests.push({ url, options, reject, resolve: payload => resolve({ ok: true, json: async () => payload }) })),
  });
  return { node, requests };
}
test('clearing connection rejects delayed success and error UI writes', async () => {
  for (const fail of [false, true]) {
    const { node, requests } = setup();
    node('[data-token-form]').value = 'token'; node('[data-token-form]').fire('submit');
    node('[data-clear-token]').fire('click');
    if (fail) requests[0].reject(new Error('late failure')); else requests[0].resolve({});
    await tick();
    assert.equal(requests.length, 1, 'stale refresh initiated more requests');
    assert.match(node('[data-answer-stack]').textContent, /Bearer token cleared/);
    assert.match(node('[data-v1-state]').textContent, /Disconnected/);
  }
});
test('latest Ask wins even when older request completes last', async () => {
  const { node, requests } = setup();
  node('[data-token-form]').value = 'token'; node('[data-token-form]').fire('submit');
  requests[0].resolve({}); await tick();
  for (const request of requests.slice(1)) request.resolve({}); await tick();
  const ask = query => { node('[data-ask-form]').value = query; node('[data-ask-form]').fire('submit'); return requests.at(-1); };
  const old = ask('old question'); const recent = ask('new question');
  recent.resolve({ result: { claims: [{ title: 'NEW' }] } }); await tick();
  old.resolve({ result: { claims: [{ title: 'OLD' }] } }); await tick();
  assert.match(node('[data-answer-stack]').textContent, /NEW/);
  assert.doesNotMatch(node('[data-answer-stack]').textContent, /OLD/);
  assert.equal(node('[data-answer-stack]').dataset.askState, 'complete');
});
