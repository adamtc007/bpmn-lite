// Browser round-trip acceptance test: template authoring -> publish -> spawn -> run to completion.
//
// Drives the ob-poc React page at /bpmn-templates (TemplateRoundTripPage) through
// chrome-devtools-mcp, same harness pattern as test_bpmn_ui_demo.js.
//
// Prerequisites:
//   1. Designer:  BPMN_LITE_DEMO_BIND=127.0.0.1:8080 cargo run -p bpmn-lite-server-designer --bin bpmn-lite-demo-designer
//   2. Frontend:  cd ob-poc-ui-react && npm run dev            (vite on :5173, proxies /api/dsl + /bpmn -> :8080)
//   3. Chrome:    launched with --remote-debugging-port=9223
//
// Asserts, in order: session created + graph ops admitted; template published;
// published list shows it; spawn returns Running with a waiting job on t1;
// "Run to completion" ends with the success banner and state Completed after
// exactly 4 advances (4 ServiceTasks) -- real VM execution via complete_job,
// not the runner's plan-walker simulation.

const { spawn } = require('child_process');

const browserUrl = process.env.BROWSER_URL || 'http://127.0.0.1:9223';
const targetUrl = process.env.TARGET_URL || 'http://localhost:5173';
const templateName = process.env.TEMPLATE_NAME || 'template1';

console.log(`Starting chrome-devtools-mcp connecting to ${browserUrl}...`);
const child = spawn('npx', ['-y', 'chrome-devtools-mcp', '--browserUrl', browserUrl]);

let buffer = '';
let nextId = 1;
const pending = new Map();

child.stdout.on('data', (data) => {
  buffer += data.toString();
  let lineEnd;
  while ((lineEnd = buffer.indexOf('\n')) !== -1) {
    const line = buffer.substring(0, lineEnd).trim();
    buffer = buffer.substring(lineEnd + 1);
    if (!line) continue;
    try {
      const msg = JSON.parse(line);
      if (msg.id && pending.has(msg.id)) {
        const { resolve, reject } = pending.get(msg.id);
        pending.delete(msg.id);
        if (msg.error) reject(msg.error);
        else resolve(msg.result);
      }
    } catch (e) {
      console.error('Failed to parse JSON RPC line:', line, e);
    }
  }
});
child.stderr.on('data', () => {});

function callTool(name, args = {}) {
  return new Promise((resolve, reject) => {
    const id = nextId++;
    pending.set(id, { resolve, reject });
    child.stdin.write(
      JSON.stringify({ jsonrpc: '2.0', id, method: 'tools/call', params: { name, arguments: args } }) + '\n'
    );
  });
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// Run a script in the page; the function body must return a JSON-serializable value.
async function evalInPage(fnBody) {
  const res = await callTool('evaluate_script', { function: fnBody });
  const text = res?.content?.map((c) => c.text).join('\n') ?? '';
  return text;
}

async function clickByText(label) {
  return evalInPage(`() => {
    const btn = [...document.querySelectorAll('button')].find(b => b.textContent.trim() === ${JSON.stringify(label)});
    if (!btn) return 'NOT_FOUND';
    btn.click();
    return 'CLICKED';
  }`);
}

async function pageText() {
  return evalInPage(`() => document.body.innerText`);
}

async function waitForText(needle, timeoutMs, label) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const text = await pageText();
    if (text.includes(needle)) return text;
    if (Date.now() > deadline) {
      throw new Error(`Timed out waiting for "${needle}" (${label}). Page text:\n${text}`);
    }
    await sleep(400);
  }
}

function assert(cond, msg) {
  if (!cond) throw new Error(`ASSERTION FAILED: ${msg}`);
}

async function main() {
  await callTool('new_page', { url: `${targetUrl}/bpmn-templates` });
  await sleep(1200);

  // Step 1: author the 4-step workflow (session + graph-edit).
  assert((await clickByText('Create 4-step workflow')).includes('CLICKED'), 'create button present');
  const afterCreate = await waitForText('ops result:', 15000, 'graph ops admitted');
  assert(afterCreate.includes('session:'), 'session id shown');
  console.log('PASS 1: session created, graph ops admitted');

  // Step 2: publish as template.
  assert((await clickByText('Save as template')).includes('CLICKED'), 'save button present');
  const afterSave = await waitForText('published', 15000, 'template published');
  assert(afterSave.includes(templateName), `published entry names ${templateName}`);
  console.log('PASS 2: template published');

  // Step 3: spawn from the published list (first row = newest version).
  assert((await clickByText('Spawn instance')).includes('CLICKED'), 'spawn button present');
  const afterSpawn = await waitForText('waiting jobs:', 15000, 'instance spawned');
  assert(afterSpawn.includes('Running'), 'instance is Running after spawn');
  assert(afterSpawn.includes('t1 ('), 'VM parked on t1 job wait');
  console.log('PASS 3: instance spawned, Running, waiting on t1');

  // Step 4: run to completion through the real engine.
  assert((await clickByText('Run to completion')).includes('CLICKED'), 'run button present');
  const finalText = await waitForText('completed after', 30000, 'run to completion');
  assert(finalText.includes('Completed'), 'state is Completed');
  assert(finalText.includes('completed after 4 advance(s)'), 'exactly 4 advances for 4 tasks');
  assert(/waiting jobs:\s*\n?\s*none/.test(finalText), 'no waiting jobs remain');
  console.log('PASS 4: instance ran to Completed in 4 advances');

  console.log('ROUND TRIP: ALL PASS');
}

main()
  .then(() => process.exit(0))
  .catch((e) => {
    console.error(e.message || e);
    process.exit(1);
  })
  .finally(() => child.kill());
