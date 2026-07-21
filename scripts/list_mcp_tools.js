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
          console.log('Available tools:');
          msg.result.tools.forEach(t => {
            console.log(`- ${t.name}: ${t.description}`);
          });
          child.kill();
          process.exit(0);
        }
      } catch (e) {
        // Ignored
      }
    }
  }
});

// Request list
setTimeout(() => {
  const req = {
    jsonrpc: '2.0',
    id: 1,
    method: 'tools/list',
    params: {}
  };
  child.stdin.write(JSON.stringify(req) + '\n');
}, 1000);
