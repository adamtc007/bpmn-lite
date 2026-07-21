const { spawn } = require('child_process');
const fs = require('fs');
const path = require('path');

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
    console.log('   BPMN Interactive Browser Test (MCP Chrome)     ');
    console.log('==================================================\n');

    // 1. Connect and navigate
    console.log('[STEP 1] Connecting to browser pages...');
    const pages = await callTool('list_pages');
    console.log(`Found active browser tab(s).`);

    console.log(`[STEP 2] Navigating to target URL: ${targetUrl}/chat`);
    await callTool('navigate_page', { url: `${targetUrl}/chat` });
    await fnSleep(5000); // Allow frontend extra time to load

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
    await fnSleep(4000);

    // 3. Select BPMN Workspace
    console.log('[STEP 4] Selecting BPMN workspace directly from the Universe options...');
    let postSessionSnapshot = await callTool('take_snapshot');
    let bpmnWorkspaceOptionUid = null;
    for (const line of postSessionSnapshot.content[0].text.split('\n')) {
      if (line.includes('button "BPMN bpmn_workspace"')) {
        const match = line.match(/uid=([^\s]+)/);
        if (match) bpmnWorkspaceOptionUid = match[1];
      }
    }

    if (!bpmnWorkspaceOptionUid) {
      console.log('--- postSessionSnapshot Text ---');
      console.log(postSessionSnapshot.content && postSessionSnapshot.content[0] ? postSessionSnapshot.content[0].text : 'No text content');
      console.log('--------------------------------');
      throw new Error('Could not find BPMN workspace button in initial options');
    }

    await callTool('click', { uid: bpmnWorkspaceOptionUid });
    await fnSleep(5000);

    // 4. Submit macro request to force journey suggestions
    console.log('[STEP 5] Typing initial query: "Let us wrap create-cbu in a retry loop" to trigger journey selection...');
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

    await callTool('fill', { uid: bpmnTextboxUid, value: 'Let us wrap create-cbu in a retry loop' });
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
    console.log('Waiting 8 seconds for journey suggestions...');
    await fnSleep(8000);

    // 4.5. Select BPMN Operations Journey
    console.log('[STEP 5.5] Selecting BPMN Operations journey from suggestion card...');
    let postWorkspaceSnapshot = await callTool('take_snapshot');
    let bpmnOperationsUid = null;
    for (const line of postWorkspaceSnapshot.content[0].text.split('\n')) {
      if (line.includes('button "BPMN Operations')) {
        const match = line.match(/uid=([^\s]+)/);
        if (match) bpmnOperationsUid = match[1];
      }
    }

    if (!bpmnOperationsUid) {
      // Fallback: search for any clickables with "BPMN Operations"
      for (const line of postWorkspaceSnapshot.content[0].text.split('\n')) {
        if (line.toLowerCase().includes('bpmn operations') && line.includes('button')) {
          const match = line.match(/uid=([^\s]+)/);
          if (match) {
            bpmnOperationsUid = match[1];
            break;
          }
        }
      }
    }

    if (!bpmnOperationsUid) {
      fs.writeFileSync(path.join(__dirname, 'journey_suggestions_failed_snapshot.json'), JSON.stringify(postWorkspaceSnapshot, null, 2));
      throw new Error('Could not find BPMN Operations journey button in suggestions');
    }

    console.log(`Found BPMN Operations journey button (UID: ${bpmnOperationsUid}). Clicking to activate...`);
    await callTool('click', { uid: bpmnOperationsUid });
    await fnSleep(6000);

    // 5. Submit actual macro request inside active journey
    console.log('[STEP 5.6] Submitting macro request inside active journey: "Let us wrap create-cbu in a retry loop"...');
    let journeyActiveSnapshot = await callTool('take_snapshot');
    let activeTextboxUid = null;
    for (const line of journeyActiveSnapshot.content[0].text.split('\n')) {
      if (line.includes('textbox "Type a message..."')) {
        const match = line.match(/uid=([^\s]+)/);
        if (match) activeTextboxUid = match[1];
      }
    }

    if (!activeTextboxUid) {
      throw new Error('Could not find chat input textbox inside active BPMN Operations journey');
    }

    await callTool('fill', { uid: activeTextboxUid, value: 'Let us wrap create-cbu in a retry loop' });
    await fnSleep(1000);

    let activeSendBtnSnapshot = await callTool('take_snapshot');
    let activeSendBtnUid = null;
    for (const line of activeSendBtnSnapshot.content[0].text.split('\n')) {
      if (line.includes('button "Send message (Enter)"')) {
        const match = line.match(/uid=([^\s]+)/);
        if (match) activeSendBtnUid = match[1];
      }
    }

    if (!activeSendBtnUid) {
      throw new Error('Could not find Send button inside active BPMN Operations journey');
    }

    await callTool('click', { uid: activeSendBtnUid });
    console.log('Waiting 12 seconds for Sage to suggest the macro mutation...');
    await fnSleep(12000);

    // 6. Look for Suggested Action and click Apply
    console.log('[STEP 6] Identifying and applying suggested macro...');
    let macroSuggestedSnapshot = await callTool('take_snapshot');
    
    // Save snapshot to log directory for debugging
    const snapPath = path.join(__dirname, 'macro_suggested_snapshot.json');
    fs.writeFileSync(snapPath, JSON.stringify(macroSuggestedSnapshot, null, 2));
    
    let applyMacroBtnUid = null;
    for (const line of macroSuggestedSnapshot.content[0].text.split('\n')) {
      if (line.includes('Apply macro') || line.includes('button "Apply"') || line.includes('button "Yes"') || line.includes('apply_macro')) {
        // Look for buttons in the card
        const match = line.match(/uid=([^\s]+)/);
        if (match) applyMacroBtnUid = match[1];
      }
    }

    if (applyMacroBtnUid) {
      console.log(`Found Apply button (UID: ${applyMacroBtnUid}). Clicking to execute AST mutation...`);
      await callTool('click', { uid: applyMacroBtnUid });
      await fnSleep(6000);
      console.log('  -> SUCCESS: Suggested macro applied!');
    } else {
      console.log('  -> WARNING: Could not find explicit "Apply" button. Scanning alternate options...');
      // Fallback: search for any clickables with "Apply" or "Confirm"
      for (const line of macroSuggestedSnapshot.content[0].text.split('\n')) {
        if (line.toLowerCase().includes('apply') && line.includes('button')) {
          const match = line.match(/uid=([^\s]+)/);
          if (match) {
            applyMacroBtnUid = match[1];
            break;
          }
        }
      }
      if (applyMacroBtnUid) {
        console.log(`Found fallback button (UID: ${applyMacroBtnUid}). Clicking...`);
        await callTool('click', { uid: applyMacroBtnUid });
        await fnSleep(6000);
      } else {
        console.log('  -> FAIL: Suggested action buttons not found in the chat card.');
        passed = false;
      }
    }

    // 6. Verify visual workflow representation
    console.log('[STEP 7] Verifying visual workflow diagram...');
    let finalSnapshot = await callTool('take_snapshot');
    const contentText = finalSnapshot.content && finalSnapshot.content[0] ? finalSnapshot.content[0].text : '';

    if (contentText.includes('create-cbu-retry-loop') || contentText.includes('Loop (Max 3)')) {
      console.log('  -> PASS: Workflow map updated. Loop block exists in visual diagram.');
    } else {
      console.log('  -> FAIL: Loop block did not render in the visual diagram.');
      passed = false;
    }

    console.log('\n==================================================');
    if (passed) {
      console.log('  Interactive E2E Test Suite Completed successfully! ');
    } else {
      console.log('  Interactive E2E Test Suite FAILED assertions!      ');
    }
    console.log('==================================================\n');

  } catch (err) {
    console.error('\n[FAIL] E2E script encountered an error:', err);
    passed = false;
  } finally {
    child.kill();
    process.exit(passed ? 0 : 1);
  }
}

// Start execution
runTests();
