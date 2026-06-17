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

function parseMcpJson(text) {
  const match = text.match(/```json\n([\s\S]*?)\n```/);
  const jsonStr = match ? match[1] : text;
  return JSON.parse(jsonStr);
}

async function clickByText(text) {
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
    return parseMcpJson(res.content[0].text);
  } catch (err) {
    return { success: false, error: err.toString() };
  }
}

async function isBpmnWorkspaceActive() {
  const expr = `() => {
    const hasReset = Array.from(document.querySelectorAll('button')).some(b => {
      const txt = (b.textContent || '').toLowerCase();
      const title = (b.title || b.getAttribute('title') || '').toLowerCase();
      return txt.includes('reset demo') || title.includes('reset demo');
    });
    const hasFund = Array.from(document.querySelectorAll('button')).some(b => {
      const txt = (b.textContent || '').toLowerCase();
      return txt.includes('fund cbu');
    });
    return hasReset && hasFund;
  }`;
  try {
    const res = await callTool('evaluate_script', { "function": expr });
    return parseMcpJson(res.content[0].text);
  } catch (err) {
    return false;
  }
}

async function runTests() {
  let passed = true;
  try {
    console.log('\n==================================================');
    console.log('    BPMN UI E2E Demo Test Pack (MCP Chrome)      ');
    console.log('==================================================\n');

    // 1. Connect and navigate
    console.log('[STEP 1] Connecting to browser pages...');
    await callTool('list_pages');
    await callTool('select_page', { pageIdx: 0 });
    console.log(`Found active browser tab(s).`);

    const bpmnActive = await isBpmnWorkspaceActive();
    if (bpmnActive) {
      console.log('Detected that BPMN workspace is already active. Skipping navigation, session creation, and workspace selection.');
    } else {
      console.log(`[STEP 2] Navigating to target URL: ${targetUrl}/chat`);
      await callTool('navigate_page', { url: `${targetUrl}/chat` });
      await fnSleep(5000);

      // 2. Click New Session
      console.log('[STEP 3] Starting a new conversational session...');
      const sessionClick = await clickByText('New session');
      if (!sessionClick.success) {
        throw new Error(`Failed to click New session: ${sessionClick.error}`);
      }
      await fnSleep(4000);

      // 3. Select BPMN Workspace
      console.log('[STEP 4] Selecting BPMN workspace directly from the Universe options...');
      const bpmnClick = await clickByText('BPMN');
      if (!bpmnClick.success) {
        throw new Error(`Failed to click BPMN Workspace option: ${bpmnClick.error}`);
      }
      await fnSleep(5000);
    }

    // 4. Click Reset Demo first to ensure clean state
    console.log('[STEP 5] Cleaning state: resetting demo...');
    const resetClick = await clickByText('Reset demo');
    if (resetClick.success) {
      await fnSleep(2000);
      console.log('  -> Demo reset successful.');
    } else {
      console.log('  -> WARNING: Could not find Reset button in workspace.');
    }

    // 5. Execute Scenarios sequentially
    const scenarios = [
      { name: 'fund', btnLabel: 'Fund CBU' },
      { name: 'corporate', btnLabel: 'Corporate CBU' },
      { name: 'trust', btnLabel: 'Trust CBU' }
    ];

    for (const scenario of scenarios) {
      console.log(`\n--------------------------------------------------`);
      console.log(`Running Scenario: ${scenario.name.toUpperCase()}`);
      console.log(`--------------------------------------------------`);

      console.log(`Clicking '${scenario.btnLabel}'...`);
      const startClick = await clickByText(scenario.btnLabel);
      if (!startClick.success) {
        throw new Error(`Could not click start button for scenario: ${scenario.btnLabel}. Error: ${startClick.error}`);
      }
      await fnSleep(3000);

      // Step execution loop
      let step = 0;
      let completed = false;
      const maxSteps = 15;

      while (step < maxSteps) {
        step++;
        let currentSnap = await callTool('take_snapshot');
        let pageText = currentSnap.content[0].text;

        // Save diagnostic snapshot for trace
        fs.writeFileSync(
          path.join(__dirname, `scenario_${scenario.name}_step_${step}.json`),
          JSON.stringify(currentSnap, null, 2)
        );

        if (pageText.includes('✓ CBU Operational') || pageText.includes('Completed')) {
          console.log(`  -> SUCCESS: Process completed! CBU is Operational.`);
          completed = true;
          break;
        }

        // Click the Next button
        const nextClick = await clickByText('Next:');
        if (nextClick.success) {
          console.log(`  [Step ${step}] Clicking: "${nextClick.text}"`);
          await fnSleep(3000);
        } else {
          // Check again if completed just in case of state lag
          if (pageText.includes('✓ CBU Operational') || pageText.includes('Completed')) {
            console.log(`  -> SUCCESS: Process completed! CBU is Operational.`);
            completed = true;
            break;
          }
          console.log(`  [Step ${step}] No 'Next' button found. Waiting for execution...`);
          await fnSleep(2000);
        }
      }

      if (!completed) {
        console.log(`  -> FAIL: Scenario '${scenario.name}' failed to complete.`);
        passed = false;
      }
    }

    console.log('\n==================================================');
    if (passed) {
      console.log('   All BPMN UI Scenarios Mimicked Successfully!   ');
    } else {
      console.log('   BPMN UI Scenarios FAILED assertions!           ');
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
