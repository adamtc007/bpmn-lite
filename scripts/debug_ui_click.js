const { spawn } = require('child_process');

const browserUrl = process.env.BROWSER_URL || 'http://127.0.0.1:9223';
const targetUrl = process.env.TARGET_URL || 'http://localhost:5173';

console.log(`Connecting to ${browserUrl}...`);
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
    if (line) {
      try {
        const msg = JSON.parse(line);
        if (msg.id && pending.has(msg.id)) {
          const { resolve, reject } = pending.get(msg.id);
          pending.delete(msg.id);
          if (msg.error) reject(msg.error);
          else resolve(msg.result);
        }
      } catch (e) {
        console.error('JSON parse error:', line, e);
      }
    }
  }
});

function callTool(name, args = {}) {
  return new Promise((resolve, reject) => {
    const id = nextId++;
    const req = {
      jsonrpc: '2.0',
      id,
      method: 'tools/call',
      params: { name, arguments: args }
    };
    pending.set(id, { resolve, reject });
    child.stdin.write(JSON.stringify(req) + '\n');
  });
}

const sleep = ms => new Promise(r => setTimeout(r, ms));

function parseMcpJson(text) {
  const match = text.match(/```json\n([\s\S]*?)\n```/);
  const jsonStr = match ? match[1] : text;
  return JSON.parse(jsonStr);
}

async function clickByText(text) {
  console.log(`Clicking element containing (case-insensitive): "${text}"`);
  const expr = `() => {
    const target = '${text}'.toLowerCase();
    const btn = Array.from(document.querySelectorAll('button')).find(b => {
      const txt = (b.textContent || '').toLowerCase();
      const title = (b.title || b.getAttribute('title') || '').toLowerCase();
      return txt.includes(target) || title.includes(target);
    });
    if (btn) {
      btn.click();
      return { success: true, text: (btn.textContent || btn.title).trim() };
    }
    return { success: false, error: 'Not found' };
  }`;
  try {
    const res = await callTool('evaluate_script', { "function": expr });
    const parsed = parseMcpJson(res.content[0].text);
    console.log(`Result for "${text}":`, parsed);
    return parsed;
  } catch (err) {
    console.error(`Evaluation failed for "${text}":`, err);
    return { success: false, error: err.toString() };
  }
}

async function main() {
  try {
    console.log('--- Connecting & Navigating ---');
    await callTool('list_pages');
    await callTool('select_page', { pageIdx: 0 });
    await callTool('navigate_page', { url: `${targetUrl}/chat` });
    await sleep(5000);

    console.log('--- Starting New Session ---');
    await clickByText('New session');
    await sleep(4000);

    console.log('--- Selecting BPMN Workspace ---');
    await clickByText('BPMN');
    await sleep(4000);

    console.log('--- Resetting Demo State ---');
    await clickByText('Reset demo');
    await sleep(2000);

    console.log('--- Clicking Fund CBU ---');
    await clickByText('Fund CBU');
    await sleep(3000);

    console.log('--- Clicking Next Button ---');
    const clickRes = await clickByText('Next:');
    console.log('Click result:', clickRes);
    await sleep(3000);

    console.log('--- Getting Console Messages ---');
    const logs = await callTool('list_console_messages');
    console.log(logs.content[0].text);

    console.log('--- Getting Network Requests ---');
    const net = await callTool('list_network_requests');
    const lines = net.content[0].text.split('\n');
    const postRequests = lines.filter(line => line.includes('POST') || line.includes('next-step') || line.includes('start'));
    console.log('POST/next-step/start network requests:', postRequests);

  } catch (err) {
    console.error('Error:', err);
  } finally {
    child.kill();
  }
}

main();
