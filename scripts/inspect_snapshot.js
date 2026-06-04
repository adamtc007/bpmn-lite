const { spawn } = require('child_process');
const fs = require('fs');

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

async function fnSleep(ms) {
  return new Promise(r => setTimeout(r, ms));
}

async function run() {
  try {
    console.log('Listing pages...');
    await callTool('list_pages');

    console.log('Navigating to http://localhost:5173/chat...');
    await callTool('navigate_page', { url: 'http://localhost:5173/chat' });
    await fnSleep(2000);

    console.log('Taking initial snapshot...');
    let initialSnapshot = await callTool('take_snapshot');
    
    // Find the UID of the "New session" button
    let newSessionUid = null;
    for (const line of initialSnapshot.content[0].text.split('\n')) {
      if (line.includes('button "New session"')) {
        const match = line.match(/uid=([^\s]+)/);
        if (match) newSessionUid = match[1];
      }
    }

    if (!newSessionUid) {
      console.log('Could not find New session button');
      return;
    }

    console.log(`Clicking New session (uid: ${newSessionUid})...`);
    await callTool('click', { uid: newSessionUid });
    await fnSleep(3000);

    console.log('Taking snapshot after session creation...');
    let postSessionSnapshot = await callTool('take_snapshot');
    
    // Find the chat textbox
    let textboxUid = null;
    for (const line of postSessionSnapshot.content[0].text.split('\n')) {
      if (line.includes('textbox "Select or create a session first"') || line.includes('textbox "Type a message..."')) {
        const match = line.match(/uid=([^\s]+)/);
        if (match) textboxUid = match[1];
      }
    }

    if (!textboxUid) {
      console.log('Could not find chat textbox');
      return;
    }

    console.log(`Typing "bpmn" to trigger infrastructure bypass (uid: ${textboxUid})...`);
    await callTool('fill', { uid: textboxUid, value: 'bpmn' });
    await fnSleep(1000);

    console.log('Taking snapshot to find send button...');
    let sendBtnSnapshot = await callTool('take_snapshot');
    let sendBtnUid = null;
    for (const line of sendBtnSnapshot.content[0].text.split('\n')) {
      if (line.includes('button "Send message (Enter)"')) {
        const match = line.match(/uid=([^\s]+)/);
        if (match) sendBtnUid = match[1];
      }
    }

    if (!sendBtnUid) {
      console.log('Could not find Send button after filling text');
      return;
    }

    console.log(`Clicking Send button (uid: ${sendBtnUid})...`);
    await callTool('click', { uid: sendBtnUid });
    await fnSleep(4000);

    console.log('Taking snapshot to find BPMN workspace selection option...');
    let workspaceOptionsSnapshot = await callTool('take_snapshot');
    let bpmnWorkspaceOptionUid = null;
    for (const line of workspaceOptionsSnapshot.content[0].text.split('\n')) {
      if (line.includes('button "BPMN bpmn_workspace"')) {
        const match = line.match(/uid=([^\s]+)/);
        if (match) bpmnWorkspaceOptionUid = match[1];
      }
    }

    if (!bpmnWorkspaceOptionUid) {
      console.log('Could not find BPMN workspace button in options:', workspaceOptionsSnapshot.content[0].text);
      return;
    }

    console.log(`Clicking BPMN workspace option (uid: ${bpmnWorkspaceOptionUid})...`);
    await callTool('click', { uid: bpmnWorkspaceOptionUid });
    await fnSleep(4000);

    console.log('Taking snapshot after selecting BPMN workspace...');
    let bpmnWorkspaceSnapshot = await callTool('take_snapshot');

    // Find the chat textbox inside the active BPMN workspace
    let bpmnTextboxUid = null;
    for (const line of bpmnWorkspaceSnapshot.content[0].text.split('\n')) {
      if (line.includes('textbox "Type a message..."')) {
        const match = line.match(/uid=([^\s]+)/);
        if (match) bpmnTextboxUid = match[1];
      }
    }

    if (!bpmnTextboxUid) {
      console.log('Could not find chat textbox inside BPMN workspace');
      return;
    }

    console.log(`Typing bespoke designer utterance in active BPMN session (uid: ${bpmnTextboxUid})...`);
    await callTool('fill', { uid: bpmnTextboxUid, value: 'Let us build a simple custody process with a start and end.' });
    await fnSleep(1000);

    console.log('Taking snapshot to find send button in BPMN session...');
    let bpmnSendBtnSnapshot = await callTool('take_snapshot');
    let bpmnSendBtnUid = null;
    for (const line of bpmnSendBtnSnapshot.content[0].text.split('\n')) {
      if (line.includes('button "Send message (Enter)"')) {
        const match = line.match(/uid=([^\s]+)/);
        if (match) bpmnSendBtnUid = match[1];
      }
    }

    if (!bpmnSendBtnUid) {
      console.log('Could not find Send button in BPMN session');
      return;
    }

    console.log(`Clicking Send button in BPMN session (uid: ${bpmnSendBtnUid})...`);
    await callTool('click', { uid: bpmnSendBtnUid });
    await fnSleep(10000);

    console.log('Taking final snapshot of BPMN designer workspace...');
    let finalSnapshot = await callTool('take_snapshot');
    fs.writeFileSync('/Users/adamtc007/dev/bpmn-lite/scripts/snapshot.json', JSON.stringify(finalSnapshot, null, 2));
    console.log('Snapshot written to scripts/snapshot.json');
    console.log('Snapshot text content:\n', finalSnapshot.content[0].text);

  } catch (err) {
    console.error('Error running:', err);
  } finally {
    child.kill();
    process.exit(0);
  }
}

run();
