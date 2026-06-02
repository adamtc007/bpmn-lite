const { spawn } = require('child_process');

const child = spawn('npx', ['-y', 'chrome-devtools-mcp', '--browserUrl', 'http://127.0.0.1:9223']);

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
        console.error('Failed to parse line:', line, e);
      }
    }
  }
});

child.stderr.on('data', (data) => {
  // console.error('STDERR:', data.toString());
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

async function run() {
  try {
    console.log('Listing pages...');
    let pagesResult = await callTool('list_pages');
    console.log('Pages:', JSON.stringify(pagesResult, null, 2));

    console.log('Navigating to http://localhost:5173 ...');
    let navResult = await callTool('navigate_page', { url: 'http://localhost:5173' });
    console.log('Navigation complete. Waiting 3s...');
    await new Promise(r => setTimeout(r, 3000));

    console.log('Taking snapshot...');
    let snapshotResult = await callTool('take_snapshot');
    // Save to a scratch file so we can view it
    const fs = require('fs');
    fs.writeFileSync('/Users/adamtc007/dev/bpmn-lite/scripts/snapshot.json', JSON.stringify(snapshotResult, null, 2));
    console.log('Snapshot written to scripts/snapshot.json');

    // Summarize the top level keys
    console.log('Snapshot Keys:', Object.keys(snapshotResult));
    if (snapshotResult.content && snapshotResult.content[0]) {
      const contentText = snapshotResult.content[0].text;
      try {
        const parsed = JSON.parse(contentText);
        console.log('Parsed content keys:', Object.keys(parsed));
        if (parsed.tree) {
          console.log('Root element nodeName:', parsed.tree.nodeName);
          console.log('Root element role:', parsed.tree.role);
          console.log('Root element keys:', Object.keys(parsed.tree));
        }
      } catch(e) {
        console.log('Content is not JSON. Length:', contentText.length);
        console.log('Content preview:', contentText.substring(0, 500));
      }
    }
  } catch (err) {
    console.error('Error running tools:', err);
  } finally {
    child.kill();
    process.exit(0);
  }
}

run();
