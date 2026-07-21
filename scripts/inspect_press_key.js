const { spawn } = require('child_process');

const browserUrl = process.env.BROWSER_URL || 'http://127.0.0.1:9223';
const child = spawn('npx', ['-y', 'chrome-devtools-mcp', '--browserUrl', browserUrl]);

let buffer = '';
child.stdout.on('data', (data) => {
  buffer += data.toString();
  let lineEnd;
  while ((lineEnd = buffer.indexOf('\n')) !== -1) {
    const line = buffer.substring(0, lineEnd).trim();
    buffer = buffer.substring(lineEnd + 1);
    if (line) {
      try {
        const msg = JSON.parse(line);
        if (msg.id === 1) {
          const tool = msg.result.tools.find(t => t.name === 'press_key');
          console.log(JSON.stringify(tool, null, 2));
          child.kill();
          process.exit(0);
        }
      } catch (e) {
        // Ignored
      }
    }
  }
});

setTimeout(() => {
  child.stdin.write(JSON.stringify({
    jsonrpc: '2.0',
    id: 1,
    method: 'tools/list',
    params: {}
  }) + '\n');
}, 1000);
