const http = require('http');

const BASE_URL = process.env.BASE_URL || 'http://localhost:8080';

// Helper to make HTTP requests using native Node.js http module
function request(method, path, body = null) {
  return new Promise((resolve, reject) => {
    const url = new URL(BASE_URL + path);
    const options = {
      hostname: url.hostname,
      port: url.port,
      path: url.pathname + url.search,
      method: method,
      headers: {
        'Content-Type': 'application/json',
      },
    };

    const req = http.request(options, (res) => {
      let data = '';
      res.on('data', (chunk) => {
        data += chunk;
      });
      res.on('end', () => {
        let json;
        try {
          json = data ? JSON.parse(data) : {};
        } catch (e) {
          json = { text: data };
        }
        if (res.statusCode >= 200 && res.statusCode < 300) {
          resolve({ status: res.statusCode, data: json });
        } else {
          reject(new Error(`HTTP ${res.statusCode}: ${JSON.stringify(json)}`));
        }
      });
    });

    req.on('error', (err) => {
      reject(err);
    });

    if (body) {
      req.write(JSON.stringify(body));
    }
    req.end();
  });
}

// 1. S-expression DSL workflow definitions
const COMPREHENSIVE_DSL = `(workflow comprehensive-test-workflow
  (start-event :id start :next create-cbu)
  
  (service-task :id create-cbu :verb ob-poc:cbu.create :next type-decision)
  
  (business-rule-task :id type-decision :decision dmn-lite:cbu_type_routing :next type-gateway)
  
  (exclusive-gateway :id type-gateway
    (flow :condition (= @cbu-type "fund")      :next run-loop)
    (flow :condition (= @cbu-type "corporate") :next run-parallel)
    (flow :condition (= @cbu-type "trust")     :next run-single))
  
  (loop :id run-loop :ceiling 2 :body (
     (service-task :id loop-step :verb ob-poc:cbu.add-product :args (:product "loop-item") :next run-loop)
  ) :next end)
  
  (split-and :id run-parallel :join parallel-join
    (flow :next task-p1)
    (flow :next task-p2))
  (service-task :id task-p1 :verb ob-poc:cbu.add-product :args (:product "part1") :next parallel-join)
  (service-task :id task-p2 :verb ob-poc:cbu.add-product :args (:product "part2") :next parallel-join)
  (join-and :id parallel-join :split run-parallel :next end)
  
  (service-task :id run-single :verb ob-poc:instrument-matrix.attach :next end)
  
  (end-event :id end :status "Operational"))`;

// 2. Main test suite execution
async function main() {
  console.log('==================================================');
  console.log('      BPMN Scenario Integration Test Harness      ');
  console.log('==================================================\n');

  try {
    // Step 1: Register workflow template
    console.log('[STEP 1] Defining and registering BPMN workflow template...');
    const templateName = 'comprehensive-test-workflow';
    const registerRes = await request('POST', '/bpmn/templates', {
      name: templateName,
      dsl_body: COMPREHENSIVE_DSL,
    });
    console.log(`  -> SUCCESS: Registered template. Hash: ${registerRes.data.plan_hash}, Version: ${registerRes.data.version}\n`);

    // Verify it is listed in the templates registry
    console.log('[STEP 2] Verifying template list...');
    const listTemplates = await request('GET', '/bpmn/templates');
    const template = listTemplates.data.find(t => t.name === templateName);
    if (!template) {
      throw new Error(`Template '${templateName}' was not found in the registry list.`);
    }
    console.log(`  -> SUCCESS: Template found in registry.\n`);

    // Step 3: Run scenarios for different client types
    const scenarios = [
      { type: 'fund', expectedPath: ['create-cbu', 'type-decision', 'run-loop', 'loop-step', 'run-loop', 'loop-step', 'end'] },
      { type: 'corporate', expectedPath: ['create-cbu', 'type-decision', 'run-parallel', 'task-p1', 'task-p2', 'parallel-join', 'end'] },
      { type: 'trust', expectedPath: ['create-cbu', 'type-decision', 'run-single', 'end'] }
    ];

    for (const scenario of scenarios) {
      console.log(`--------------------------------------------------`);
      console.log(`Executing Scenario: Client Type = ${scenario.type.toUpperCase()}`);
      console.log(`--------------------------------------------------`);

      // Start Instance
      console.log(`[START] Spawning instance with cbu_type: '${scenario.type}'...`);
      const startRes = await request('POST', '/bpmn/instances/start', {
        cbu_type: scenario.type,
        bpmn_dsl: COMPREHENSIVE_DSL,
        variables: {
          "@cbu-type": scenario.type,
        }
      });
      const instanceId = startRes.data.instance_id;
      console.log(`  -> SUCCESS: Spawned instance ID: ${instanceId}`);

      // Verify Visualization map
      console.log(`[VIZ] Retrieving UI visualization graph...`);
      const graphRes = await request('GET', `/bpmn/instances/${instanceId}/graph`);
      const nodesCount = graphRes.data.nodes.length;
      const edgesCount = graphRes.data.edges.length;
      console.log(`  -> SUCCESS: Visual map loaded. Nodes: ${nodesCount}, Edges: ${edgesCount}`);
      
      // Drive through the process execution steps
      const isActive = (s) => s === 'Running' || s.startsWith('WaitingOnSubmission') || s.startsWith('WaitingOnInvocation');
      let currentNode = null;
      let status = 'Running';
      let executionTrace = [];
      let maxSteps = 20;
      let stepsCount = 0;

      while (isActive(status) && stepsCount < maxSteps) {
        stepsCount++;
        
        // Get current instance details
        const instDetails = await request('GET', `/bpmn/instances/${instanceId}`);
        currentNode = instDetails.data.current_node;
        status = instDetails.data.status;
        
        if (!isActive(status)) {
          break;
        }

        console.log(`  [Step ${stepsCount}] Current active node: '${currentNode}'`);
        executionTrace.push(currentNode);

        // Transition outputs
        let outputs = {};
        if (currentNode === 'create-cbu') {
          outputs['@cbu'] = 'cbu-123';
        } else if (currentNode === 'type-decision') {
          outputs['@cbu-type'] = scenario.type;
        } else if (currentNode === 'loop-step') {
          outputs['@loop-count-run-loop'] = stepsCount;
        }

        await request('POST', `/bpmn/instances/${instanceId}/next-step`, { outputs });
      }

      console.log(`[FINISH] Instance reached terminal state.`);
      console.log(`  -> Final Node: ${currentNode}`);
      console.log(`  -> Final Status: ${status}`);
      console.log(`  -> Execution Trace: ${executionTrace.join(' -> ')} -> end`);

      // Assertions
      if (status !== 'Completed') {
        throw new Error(`Scenario failed: expected status 'Completed' but got '${status}'`);
      }
      console.log(`  -> PASS: Scenario completed successfully.\n`);
    }

    console.log('==================================================');
    console.log('      All BPMN Scenarios Passed Successfully!     ');
    console.log('==================================================\n');
    process.exit(0);

  } catch (error) {
    console.error('\n[FAIL] Test harness execution failed:', error.message);
    process.exit(1);
  }
}

main();
