const { spawn } = require('child_process');

const browserUrl = process.env.BROWSER_URL || 'http://127.0.0.1:9223';

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
        // ignore
      }
    }
  }
});

function callListTools() {
  return new Promise((resolve, reject) => {
    const id = nextId++;
    const req = {
      jsonrpc: '2.0',
      id,
      method: 'tools/list',
      params: {}
    };
    pending.set(id, { resolve, reject });
    child.stdin.write(JSON.stringify(req) + '\n');
  });
}

async function main() {
  try {
    const result = await callListTools();
    const evaluateTool = result.tools.find(t => t.name === 'evaluate_script');
    console.log(JSON.stringify(evaluateTool, null, 2));
  } catch (err) {
    console.error('Error:', err);
  } finally {
    child.kill();
  }
}

main();
