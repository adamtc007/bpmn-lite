const { spawn } = require('child_process');
const fs = require('fs');
const path = require('path');

// Determine browser url or default to standard headless debugging port
const browserUrl = process.env.BROWSER_URL || 'http://127.0.0.1:9223';
const targetUrl = process.env.TARGET_URL || 'http://localhost:5173';

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
        console.error('Failed to parse JSON RPC line:', line, e);
      }
    }
  }
});

child.stderr.on('data', (data) => {
  // Suppress verbose output
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

async function runTests() {
  let passed = true;
  try {
    console.log('\n==================================================');
    console.log('  Sage BPMN Utterance Designer — E2E Test Suite   ');
    console.log('==================================================\n');

    // 1. Connect and navigate
    console.log('[STEP 1] Connecting to browser pages...');
    const pages = await callTool('list_pages');
    console.log(`Found active browser tab(s).`);

    console.log(`[STEP 2] Navigating to target URL: ${targetUrl}/chat`);
    await callTool('navigate_page', { url: `${targetUrl}/chat` });
    await fnSleep(3000);

    // 2. Click New Session
    console.log('[STEP 3] Starting a new conversational session...');
    let initialSnapshot = await callTool('take_snapshot');
    let newSessionUid = null;
    for (const line of initialSnapshot.content[0].text.split('\n')) {
      if (line.includes('button "New session"')) {
        const match = line.match(/uid=([^\s]+)/);
        if (match) newSessionUid = match[1];
      }
    }
    
    if (!newSessionUid) {
      throw new Error('Could not find New session button in initial snapshot');
    }

    await callTool('click', { uid: newSessionUid });
    await fnSleep(3000);

    // 3. Select BPMN Workspace directly from the initial session page (bypassing client scope gates)
    console.log('[STEP 4] Selecting BPMN workspace directly from the Universe options to bypass scope gates...');
    let postSessionSnapshot = await callTool('take_snapshot');
    let bpmnWorkspaceOptionUid = null;
    for (const line of postSessionSnapshot.content[0].text.split('\n')) {
      if (line.includes('button "BPMN bpmn_workspace"')) {
        const match = line.match(/uid=([^\s]+)/);
        if (match) bpmnWorkspaceOptionUid = match[1];
      }
    }

    if (!bpmnWorkspaceOptionUid) {
      throw new Error('Could not find BPMN workspace button in initial options');
    }

    await callTool('click', { uid: bpmnWorkspaceOptionUid });
    await fnSleep(4000);

    // 5. Submit designer utterance
    console.log('[STEP 6] Submitting bespoke utterance to active BPMN workspace...');
    let bpmnWorkspaceSnapshot = await callTool('take_snapshot');
    let bpmnTextboxUid = null;
    for (const line of bpmnWorkspaceSnapshot.content[0].text.split('\n')) {
      if (line.includes('textbox "Type a message..."')) {
        const match = line.match(/uid=([^\s]+)/);
        if (match) bpmnTextboxUid = match[1];
      }
    }

    if (!bpmnTextboxUid) {
      throw new Error('Could not find chat input textbox inside active BPMN workspace');
    }

    await callTool('fill', { uid: bpmnTextboxUid, value: 'Let us build a simple custody process with a start and end.' });
    await fnSleep(1000);

    let bpmnSendBtnSnapshot = await callTool('take_snapshot');
    let bpmnSendBtnUid = null;
    for (const line of bpmnSendBtnSnapshot.content[0].text.split('\n')) {
      if (line.includes('button "Send message (Enter)"')) {
        const match = line.match(/uid=([^\s]+)/);
        if (match) bpmnSendBtnUid = match[1];
      }
    }

    if (!bpmnSendBtnUid) {
      throw new Error('Could not find Send button inside active BPMN workspace');
    }

    await callTool('click', { uid: bpmnSendBtnUid });
    console.log('Waiting 10 seconds for the Sage designer agent to process the utterance...');
    await fnSleep(10000);

    // 6. Verify outputs and options
    console.log('[STEP 7] Taking final visual snapshot and asserting designer capabilities...');
    let finalSnapshot = await callTool('take_snapshot');
    
    // Save snapshot for diagnostics
    const snapshotPath = path.join(__dirname, 'test_ui_snapshot.json');
    fs.writeFileSync(snapshotPath, JSON.stringify(finalSnapshot, null, 2));
    console.log(`Visual diagnostic snapshot written to scripts/test_ui_snapshot.json`);

    const contentText = finalSnapshot.content && finalSnapshot.content[0] ? finalSnapshot.content[0].text : '';

    console.log('\n[TEST 1] Asserting Active BPMN Workspace Context...');
    if (contentText.includes('bpmn.workspace') || contentText.includes('BPMN bpmn_workspace')) {
      console.log('  -> PASS: Active workspace correctly bounds to BPMN context.');
    } else {
      console.log('  -> FAIL: Active workspace does not reference BPMN.');
      passed = false;
    }

    console.log('\n[TEST 2] Verifying Sage UTTER Gating & Journey Activation...');
    if (contentText.includes('BPMN Operations') || contentText.includes('Pack activated')) {
      console.log('  -> PASS: Conversational input matched to BPMN authoring journeys.');
    } else {
      console.log('  -> FAIL: Sage failed to identify authoring journey from designer utterance.');
      passed = false;
    }

    console.log('\n==================================================');
    if (passed) {
      console.log('  E2E Test Execution Completed Successfully!      ');
    } else {
      console.log('  E2E Test Execution FAILED Core Assertions!      ');
    }
    console.log('==================================================\n');

  } catch (err) {
    console.error('\n[FAIL] Test suite encountered an error:', err);
    passed = false;
  } finally {
    child.kill();
    process.exit(passed ? 0 : 1);
  }
}

// Start execution
runTests();
