use bpmn_lite_authoring::importer::import_zeebe_bpmn;
use bpmn_lite_compiler::dsl::ExecutionNode;

// Helper to construct a basic definitions envelope around a process body.
fn wrap_process(id: &str, body: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
        <bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL"
                          xmlns:zeebe="http://camunda.org/schema/zeebe/1.0"
                          id="definitions">
          <bpmn:process id="{}" isExecutable="true">
            {}
          </bpmn:process>
        </bpmn:definitions>"#,
        id, body
    )
}

// 1. Linear Path (Start -> ServiceTask -> End)
const BODY_1_LINEAR: &str = r#"
    <bpmn:startEvent id="start" />
    <bpmn:serviceTask id="task1" name="Task 1">
      <bpmn:extensionElements>
        <zeebe:taskDefinition type="cbu.create" />
      </bpmn:extensionElements>
    </bpmn:serviceTask>
    <bpmn:endEvent id="end" />
    <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="task1" />
    <bpmn:sequenceFlow id="f2" sourceRef="task1" targetRef="end" />
"#;

// 2. Two Sequential Tasks
const BODY_2_SEQ_TASKS: &str = r#"
    <bpmn:startEvent id="start" />
    <bpmn:serviceTask id="task1" name="Task 1">
      <bpmn:extensionElements>
        <zeebe:taskDefinition type="cbu.create" />
      </bpmn:extensionElements>
    </bpmn:serviceTask>
    <bpmn:serviceTask id="task2" name="Task 2">
      <bpmn:extensionElements>
        <zeebe:taskDefinition type="cbu.approve" />
      </bpmn:extensionElements>
    </bpmn:serviceTask>
    <bpmn:endEvent id="end" />
    <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="task1" />
    <bpmn:sequenceFlow id="f2" sourceRef="task1" targetRef="task2" />
    <bpmn:sequenceFlow id="f3" sourceRef="task2" targetRef="end" />
"#;

// 3. Exclusive Gateway Split & Join (Valid SESE)
const BODY_3_XOR_SESE: &str = r#"
    <bpmn:startEvent id="start" />
    <bpmn:exclusiveGateway id="split" name="Split" />
    <bpmn:serviceTask id="task1" name="Task 1">
      <bpmn:extensionElements>
        <zeebe:taskDefinition type="cbu.create" />
      </bpmn:extensionElements>
    </bpmn:serviceTask>
    <bpmn:serviceTask id="task2" name="Task 2">
      <bpmn:extensionElements>
        <zeebe:taskDefinition type="cbu.approve" />
      </bpmn:extensionElements>
    </bpmn:serviceTask>
    <bpmn:exclusiveGateway id="join" name="Join" />
    <bpmn:endEvent id="end" />
    <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="split" />
    <bpmn:sequenceFlow id="f2" sourceRef="split" targetRef="task1">
      <bpmn:conditionExpression>= approved == true</bpmn:conditionExpression>
    </bpmn:sequenceFlow>
    <bpmn:sequenceFlow id="f3" sourceRef="split" targetRef="task2">
      <bpmn:conditionExpression>= approved == false</bpmn:conditionExpression>
    </bpmn:sequenceFlow>
    <bpmn:sequenceFlow id="f4" sourceRef="task1" targetRef="join" />
    <bpmn:sequenceFlow id="f5" sourceRef="task2" targetRef="join" />
    <bpmn:sequenceFlow id="f6" sourceRef="join" targetRef="end" />
"#;

// 4. Parallel Gateway Split & Join (Valid SESE)
const BODY_4_AND_SESE: &str = r#"
    <bpmn:startEvent id="start" />
    <bpmn:parallelGateway id="split" name="Split" gatewayDirection="Diverging" />
    <bpmn:serviceTask id="task1" name="Task 1">
      <bpmn:extensionElements>
        <zeebe:taskDefinition type="cbu.create" />
      </bpmn:extensionElements>
    </bpmn:serviceTask>
    <bpmn:serviceTask id="task2" name="Task 2">
      <bpmn:extensionElements>
        <zeebe:taskDefinition type="cbu.approve" />
      </bpmn:extensionElements>
    </bpmn:serviceTask>
    <bpmn:parallelGateway id="join" name="Join" gatewayDirection="Converging" />
    <bpmn:endEvent id="end" />
    <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="split" />
    <bpmn:sequenceFlow id="f2" sourceRef="split" targetRef="task1" />
    <bpmn:sequenceFlow id="f3" sourceRef="split" targetRef="task2" />
    <bpmn:sequenceFlow id="f4" sourceRef="task1" targetRef="join" />
    <bpmn:sequenceFlow id="f5" sourceRef="task2" targetRef="join" />
    <bpmn:sequenceFlow id="f6" sourceRef="join" targetRef="end" />
"#;

// 5. Inclusive Gateway Split & Join (Valid SESE)
const BODY_5_OR_SESE: &str = r#"
    <bpmn:startEvent id="start" />
    <bpmn:inclusiveGateway id="split" name="Split" gatewayDirection="Diverging" />
    <bpmn:serviceTask id="task1" name="Task 1">
      <bpmn:extensionElements>
        <zeebe:taskDefinition type="cbu.create" />
      </bpmn:extensionElements>
    </bpmn:serviceTask>
    <bpmn:serviceTask id="task2" name="Task 2">
      <bpmn:extensionElements>
        <zeebe:taskDefinition type="cbu.approve" />
      </bpmn:extensionElements>
    </bpmn:serviceTask>
    <bpmn:inclusiveGateway id="join" name="Join" gatewayDirection="Converging" />
    <bpmn:endEvent id="end" />
    <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="split" />
    <bpmn:sequenceFlow id="f2" sourceRef="split" targetRef="task1" />
    <bpmn:sequenceFlow id="f3" sourceRef="split" targetRef="task2" />
    <bpmn:sequenceFlow id="f4" sourceRef="task1" targetRef="join" />
    <bpmn:sequenceFlow id="f5" sourceRef="task2" targetRef="join" />
    <bpmn:sequenceFlow id="f6" sourceRef="join" targetRef="end" />
"#;

// 6. Unpaired Exclusive Gateway (Split without matching Join)
const BODY_6_XOR_UNPAIRED: &str = r#"
    <bpmn:startEvent id="start" />
    <bpmn:exclusiveGateway id="split" name="Split" />
    <bpmn:serviceTask id="task1" name="Task 1">
      <bpmn:extensionElements>
        <zeebe:taskDefinition type="cbu.create" />
      </bpmn:extensionElements>
    </bpmn:serviceTask>
    <bpmn:serviceTask id="task2" name="Task 2">
      <bpmn:extensionElements>
        <zeebe:taskDefinition type="cbu.approve" />
      </bpmn:extensionElements>
    </bpmn:serviceTask>
    <bpmn:endEvent id="end" />
    <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="split" />
    <bpmn:sequenceFlow id="f2" sourceRef="split" targetRef="task1">
      <bpmn:conditionExpression>= approved == true</bpmn:conditionExpression>
    </bpmn:sequenceFlow>
    <bpmn:sequenceFlow id="f3" sourceRef="split" targetRef="task2">
      <bpmn:conditionExpression>= approved == false</bpmn:conditionExpression>
    </bpmn:sequenceFlow>
    <bpmn:sequenceFlow id="f4" sourceRef="task1" targetRef="end" />
    <bpmn:sequenceFlow id="f5" sourceRef="task2" targetRef="end" />
"#;

// 7. Unpaired Parallel Gateway (Split without matching Join)
const BODY_7_AND_UNPAIRED: &str = r#"
    <bpmn:startEvent id="start" />
    <bpmn:parallelGateway id="split" name="Split" gatewayDirection="Diverging" />
    <bpmn:serviceTask id="task1" name="Task 1">
      <bpmn:extensionElements>
        <zeebe:taskDefinition type="cbu.create" />
      </bpmn:extensionElements>
    </bpmn:serviceTask>
    <bpmn:serviceTask id="task2" name="Task 2">
      <bpmn:extensionElements>
        <zeebe:taskDefinition type="cbu.approve" />
      </bpmn:extensionElements>
    </bpmn:serviceTask>
    <bpmn:endEvent id="end" />
    <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="split" />
    <bpmn:sequenceFlow id="f2" sourceRef="split" targetRef="task1" />
    <bpmn:sequenceFlow id="f3" sourceRef="split" targetRef="task2" />
    <bpmn:sequenceFlow id="f4" sourceRef="task1" targetRef="end" />
    <bpmn:sequenceFlow id="f5" sourceRef="task2" targetRef="end" />
"#;

// 8. Unpaired Inclusive Gateway (Split without matching Join)
const BODY_8_OR_UNPAIRED: &str = r#"
    <bpmn:startEvent id="start" />
    <bpmn:inclusiveGateway id="split" name="Split" gatewayDirection="Diverging" />
    <bpmn:serviceTask id="task1" name="Task 1">
      <bpmn:extensionElements>
        <zeebe:taskDefinition type="cbu.create" />
      </bpmn:extensionElements>
    </bpmn:serviceTask>
    <bpmn:serviceTask id="task2" name="Task 2">
      <bpmn:extensionElements>
        <zeebe:taskDefinition type="cbu.approve" />
      </bpmn:extensionElements>
    </bpmn:serviceTask>
    <bpmn:endEvent id="end" />
    <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="split" />
    <bpmn:sequenceFlow id="f2" sourceRef="split" targetRef="task1" />
    <bpmn:sequenceFlow id="f3" sourceRef="split" targetRef="task2" />
    <bpmn:sequenceFlow id="f4" sourceRef="task1" targetRef="end" />
    <bpmn:sequenceFlow id="f5" sourceRef="task2" targetRef="end" />
"#;

// 9. Mismatched Gateways (AND Split paired with XOR Join)
const BODY_9_MISMATCHED_GATES: &str = r#"
    <bpmn:startEvent id="start" />
    <bpmn:parallelGateway id="split" name="Split" gatewayDirection="Diverging" />
    <bpmn:serviceTask id="task1" name="Task 1">
      <bpmn:extensionElements>
        <zeebe:taskDefinition type="cbu.create" />
      </bpmn:extensionElements>
    </bpmn:serviceTask>
    <bpmn:serviceTask id="task2" name="Task 2">
      <bpmn:extensionElements>
        <zeebe:taskDefinition type="cbu.approve" />
      </bpmn:extensionElements>
    </bpmn:serviceTask>
    <bpmn:exclusiveGateway id="join" name="Join" />
    <bpmn:endEvent id="end" />
    <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="split" />
    <bpmn:sequenceFlow id="f2" sourceRef="split" targetRef="task1" />
    <bpmn:sequenceFlow id="f3" sourceRef="split" targetRef="task2" />
    <bpmn:sequenceFlow id="f4" sourceRef="task1" targetRef="join" />
    <bpmn:sequenceFlow id="f5" sourceRef="task2" targetRef="join" />
    <bpmn:sequenceFlow id="f6" sourceRef="join" targetRef="end" />
"#;

// 10. Nested Exclusive Gateways (XOR inside XOR, Valid SESE)
const BODY_10_NESTED_XOR: &str = r#"
    <bpmn:startEvent id="start" />
    <bpmn:exclusiveGateway id="split1" name="Split 1" />
    <bpmn:exclusiveGateway id="split2" name="Split 2" />
    <bpmn:serviceTask id="task1" name="Task 1">
      <bpmn:extensionElements>
        <zeebe:taskDefinition type="cbu.create" />
      </bpmn:extensionElements>
    </bpmn:serviceTask>
    <bpmn:serviceTask id="task2" name="Task 2">
      <bpmn:extensionElements>
        <zeebe:taskDefinition type="cbu.approve" />
      </bpmn:extensionElements>
    </bpmn:serviceTask>
    <bpmn:exclusiveGateway id="join2" name="Join 2" />
    <bpmn:serviceTask id="task3" name="Task 3">
      <bpmn:extensionElements>
        <zeebe:taskDefinition type="instrument-matrix.attach" />
      </bpmn:extensionElements>
    </bpmn:serviceTask>
    <bpmn:exclusiveGateway id="join1" name="Join 1" />
    <bpmn:endEvent id="end" />
    <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="split1" />
    <bpmn:sequenceFlow id="f2" sourceRef="split1" targetRef="split2">
      <bpmn:conditionExpression>= approved == true</bpmn:conditionExpression>
    </bpmn:sequenceFlow>
    <bpmn:sequenceFlow id="f3" sourceRef="split2" targetRef="task1">
      <bpmn:conditionExpression>= is_fund == true</bpmn:conditionExpression>
    </bpmn:sequenceFlow>
    <bpmn:sequenceFlow id="f4" sourceRef="split2" targetRef="task2">
      <bpmn:conditionExpression>= is_fund == false</bpmn:conditionExpression>
    </bpmn:sequenceFlow>
    <bpmn:sequenceFlow id="f5" sourceRef="task1" targetRef="join2" />
    <bpmn:sequenceFlow id="f6" sourceRef="task2" targetRef="join2" />
    <bpmn:sequenceFlow id="f7" sourceRef="join2" targetRef="join1" />
    <bpmn:sequenceFlow id="f8" sourceRef="split1" targetRef="task3">
      <bpmn:conditionExpression>= approved == false</bpmn:conditionExpression>
    </bpmn:sequenceFlow>
    <bpmn:sequenceFlow id="f9" sourceRef="task3" targetRef="join1" />
    <bpmn:sequenceFlow id="f10" sourceRef="join1" targetRef="end" />
"#;

// 11. Nested Parallel Gateways (AND inside AND, Valid SESE)
const BODY_11_NESTED_AND: &str = r#"
    <bpmn:startEvent id="start" />
    <bpmn:parallelGateway id="split1" name="Split 1" gatewayDirection="Diverging" />
    <bpmn:parallelGateway id="split2" name="Split 2" gatewayDirection="Diverging" />
    <bpmn:serviceTask id="task1" name="Task 1">
      <bpmn:extensionElements>
        <zeebe:taskDefinition type="cbu.create" />
      </bpmn:extensionElements>
    </bpmn:serviceTask>
    <bpmn:serviceTask id="task2" name="Task 2">
      <bpmn:extensionElements>
        <zeebe:taskDefinition type="cbu.approve" />
      </bpmn:extensionElements>
    </bpmn:serviceTask>
    <bpmn:parallelGateway id="join2" name="Join 2" gatewayDirection="Converging" />
    <bpmn:serviceTask id="task3" name="Task 3">
      <bpmn:extensionElements>
        <zeebe:taskDefinition type="instrument-matrix.attach" />
      </bpmn:extensionElements>
    </bpmn:serviceTask>
    <bpmn:parallelGateway id="join1" name="Join 1" gatewayDirection="Converging" />
    <bpmn:endEvent id="end" />
    <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="split1" />
    <bpmn:sequenceFlow id="f2" sourceRef="split1" targetRef="split2" />
    <bpmn:sequenceFlow id="f3" sourceRef="split2" targetRef="task1" />
    <bpmn:sequenceFlow id="f4" sourceRef="split2" targetRef="task2" />
    <bpmn:sequenceFlow id="f5" sourceRef="task1" targetRef="join2" />
    <bpmn:sequenceFlow id="f6" sourceRef="task2" targetRef="join2" />
    <bpmn:sequenceFlow id="f7" sourceRef="join2" targetRef="join1" />
    <bpmn:sequenceFlow id="f8" sourceRef="split1" targetRef="task3" />
    <bpmn:sequenceFlow id="f9" sourceRef="task3" targetRef="join1" />
    <bpmn:sequenceFlow id="f10" sourceRef="join1" targetRef="end" />
"#;

// 12. Nested Mismatched Gateways (AND inside XOR, Valid SESE)
const BODY_12_AND_INSIDE_XOR: &str = r#"
    <bpmn:startEvent id="start" />
    <bpmn:exclusiveGateway id="split1" name="Split 1" />
    <bpmn:parallelGateway id="split2" name="Split 2" gatewayDirection="Diverging" />
    <bpmn:serviceTask id="task1" name="Task 1">
      <bpmn:extensionElements>
        <zeebe:taskDefinition type="cbu.create" />
      </bpmn:extensionElements>
    </bpmn:serviceTask>
    <bpmn:serviceTask id="task2" name="Task 2">
      <bpmn:extensionElements>
        <zeebe:taskDefinition type="cbu.approve" />
      </bpmn:extensionElements>
    </bpmn:serviceTask>
    <bpmn:parallelGateway id="join2" name="Join 2" gatewayDirection="Converging" />
    <bpmn:serviceTask id="task3" name="Task 3">
      <bpmn:extensionElements>
        <zeebe:taskDefinition type="instrument-matrix.attach" />
      </bpmn:extensionElements>
    </bpmn:serviceTask>
    <bpmn:exclusiveGateway id="join1" name="Join 1" />
    <bpmn:endEvent id="end" />
    <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="split1" />
    <bpmn:sequenceFlow id="f2" sourceRef="split1" targetRef="split2">
      <bpmn:conditionExpression>= approved == true</bpmn:conditionExpression>
    </bpmn:sequenceFlow>
    <bpmn:sequenceFlow id="f3" sourceRef="split2" targetRef="task1" />
    <bpmn:sequenceFlow id="f4" sourceRef="split2" targetRef="task2" />
    <bpmn:sequenceFlow id="f5" sourceRef="task1" targetRef="join2" />
    <bpmn:sequenceFlow id="f6" sourceRef="task2" targetRef="join2" />
    <bpmn:sequenceFlow id="f7" sourceRef="join2" targetRef="join1" />
    <bpmn:sequenceFlow id="f8" sourceRef="split1" targetRef="task3">
      <bpmn:conditionExpression>= approved == false</bpmn:conditionExpression>
    </bpmn:sequenceFlow>
    <bpmn:sequenceFlow id="f9" sourceRef="task3" targetRef="join1" />
    <bpmn:sequenceFlow id="f10" sourceRef="join1" targetRef="end" />
"#;

// 13. Self-Loop (Back edge to previous task, SESE Loop)
const BODY_13_LOOP_XOR: &str = r#"
    <bpmn:startEvent id="start" />
    <bpmn:exclusiveGateway id="loop_join" name="Loop Join" />
    <bpmn:serviceTask id="task1" name="Task 1">
      <bpmn:extensionElements>
        <zeebe:taskDefinition type="cbu.create" />
      </bpmn:extensionElements>
    </bpmn:serviceTask>
    <bpmn:exclusiveGateway id="loop_split" name="Loop Split" />
    <bpmn:endEvent id="end" />
    <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="loop_join" />
    <bpmn:sequenceFlow id="f2" sourceRef="loop_join" targetRef="task1" />
    <bpmn:sequenceFlow id="f3" sourceRef="task1" targetRef="loop_split" />
    <bpmn:sequenceFlow id="f4" sourceRef="loop_split" targetRef="loop_join">
      <bpmn:conditionExpression>= retry == true</bpmn:conditionExpression>
    </bpmn:sequenceFlow>
    <bpmn:sequenceFlow id="f5" sourceRef="loop_split" targetRef="end">
      <bpmn:conditionExpression>= retry == false</bpmn:conditionExpression>
    </bpmn:sequenceFlow>
"#;

// 14. Loop with sequential task inside
const BODY_14_LOOP_SEQ: &str = r#"
    <bpmn:startEvent id="start" />
    <bpmn:exclusiveGateway id="loop_join" name="Loop Join" />
    <bpmn:serviceTask id="task1" name="Task 1">
      <bpmn:extensionElements>
        <zeebe:taskDefinition type="cbu.create" />
      </bpmn:extensionElements>
    </bpmn:serviceTask>
    <bpmn:serviceTask id="task2" name="Task 2">
      <bpmn:extensionElements>
        <zeebe:taskDefinition type="cbu.approve" />
      </bpmn:extensionElements>
    </bpmn:serviceTask>
    <bpmn:exclusiveGateway id="loop_split" name="Loop Split" />
    <bpmn:endEvent id="end" />
    <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="loop_join" />
    <bpmn:sequenceFlow id="f2" sourceRef="loop_join" targetRef="task1" />
    <bpmn:sequenceFlow id="f3" sourceRef="task1" targetRef="task2" />
    <bpmn:sequenceFlow id="f4" sourceRef="task2" targetRef="loop_split" />
    <bpmn:sequenceFlow id="f5" sourceRef="loop_split" targetRef="loop_join">
      <bpmn:conditionExpression>= retry == true</bpmn:conditionExpression>
    </bpmn:sequenceFlow>
    <bpmn:sequenceFlow id="f6" sourceRef="loop_split" targetRef="end">
      <bpmn:conditionExpression>= retry == false</bpmn:conditionExpression>
    </bpmn:sequenceFlow>
"#;

// 15. Boundary Timer Event (Strictly rejected under interim stopgap)
const BODY_15_BOUNDARY_TIMER: &str = r#"
    <bpmn:startEvent id="start" />
    <bpmn:serviceTask id="task1" name="Task 1">
      <bpmn:extensionElements>
        <zeebe:taskDefinition type="cbu.create" />
      </bpmn:extensionElements>
    </bpmn:serviceTask>
    <bpmn:boundaryEvent id="timer" attachedToRef="task1">
      <bpmn:timerEventDefinition id="timer_def" />
    </bpmn:boundaryEvent>
    <bpmn:endEvent id="end" />
    <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="task1" />
    <bpmn:sequenceFlow id="f2" sourceRef="task1" targetRef="end" />
"#;

// 16. Boundary Error Event (Strictly rejected under interim stopgap)
const BODY_16_BOUNDARY_ERROR: &str = r#"
    <bpmn:startEvent id="start" />
    <bpmn:serviceTask id="task1" name="Task 1">
      <bpmn:extensionElements>
        <zeebe:taskDefinition type="cbu.create" />
      </bpmn:extensionElements>
    </bpmn:serviceTask>
    <bpmn:boundaryEvent id="error" attachedToRef="task1">
      <bpmn:errorEventDefinition id="error_def" />
    </bpmn:boundaryEvent>
    <bpmn:endEvent id="end" />
    <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="task1" />
    <bpmn:sequenceFlow id="f2" sourceRef="task1" targetRef="end" />
"#;

// 17. FEEL Condition Warning / Missing Condition Expression
const BODY_17_MISSING_EXPR: &str = r#"
    <bpmn:startEvent id="start" />
    <bpmn:exclusiveGateway id="split" name="Split" />
    <bpmn:serviceTask id="task1" name="Task 1">
      <bpmn:extensionElements>
        <zeebe:taskDefinition type="cbu.create" />
      </bpmn:extensionElements>
    </bpmn:serviceTask>
    <bpmn:serviceTask id="task2" name="Task 2">
      <bpmn:extensionElements>
        <zeebe:taskDefinition type="cbu.approve" />
      </bpmn:extensionElements>
    </bpmn:serviceTask>
    <bpmn:serviceTask id="task3" name="Task 3">
      <bpmn:extensionElements>
        <zeebe:taskDefinition type="cbu.archive" />
      </bpmn:extensionElements>
    </bpmn:serviceTask>
    <bpmn:exclusiveGateway id="join" name="Join" />
    <bpmn:endEvent id="end" />
    <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="split" />
    <bpmn:sequenceFlow id="f2" sourceRef="split" targetRef="task1" />
    <bpmn:sequenceFlow id="f3" sourceRef="split" targetRef="task2">
      <bpmn:conditionExpression>= approved == false</bpmn:conditionExpression>
    </bpmn:sequenceFlow>
    <bpmn:sequenceFlow id="f3b" sourceRef="split" targetRef="task3" />
    <bpmn:sequenceFlow id="f4" sourceRef="task1" targetRef="join" />
    <bpmn:sequenceFlow id="f5" sourceRef="task2" targetRef="join" />
    <bpmn:sequenceFlow id="f5b" sourceRef="task3" targetRef="join" />
    <bpmn:sequenceFlow id="f6" sourceRef="join" targetRef="end" />
"#;

// 18. Multiple Missing Condition Expressions (Triggering FEEL warning in permissive, error in strict)
const BODY_18_BAD_FEEL: &str = r#"
    <bpmn:startEvent id="start" />
    <bpmn:exclusiveGateway id="split" name="Split" />
    <bpmn:serviceTask id="task1" name="Task 1">
      <bpmn:extensionElements>
        <zeebe:taskDefinition type="cbu.create" />
      </bpmn:extensionElements>
    </bpmn:serviceTask>
    <bpmn:serviceTask id="task2" name="Task 2">
      <bpmn:extensionElements>
        <zeebe:taskDefinition type="cbu.approve" />
      </bpmn:extensionElements>
    </bpmn:serviceTask>
    <bpmn:serviceTask id="task3" name="Task 3">
      <bpmn:extensionElements>
        <zeebe:taskDefinition type="cbu.archive" />
      </bpmn:extensionElements>
    </bpmn:serviceTask>
    <bpmn:serviceTask id="task4" name="Task 4">
      <bpmn:extensionElements>
        <zeebe:taskDefinition type="cbu.delete" />
      </bpmn:extensionElements>
    </bpmn:serviceTask>
    <bpmn:exclusiveGateway id="join" name="Join" />
    <bpmn:endEvent id="end" />
    <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="split" />
    <bpmn:sequenceFlow id="f2" sourceRef="split" targetRef="task1">
      <bpmn:conditionExpression>= approved == true</bpmn:conditionExpression>
    </bpmn:sequenceFlow>
    <bpmn:sequenceFlow id="f3" sourceRef="split" targetRef="task2" />
    <bpmn:sequenceFlow id="f3b" sourceRef="split" targetRef="task3" />
    <bpmn:sequenceFlow id="f3c" sourceRef="split" targetRef="task4" />
    <bpmn:sequenceFlow id="f4" sourceRef="task1" targetRef="join" />
    <bpmn:sequenceFlow id="f5" sourceRef="task2" targetRef="join" />
    <bpmn:sequenceFlow id="f5b" sourceRef="task3" targetRef="join" />
    <bpmn:sequenceFlow id="f5c" sourceRef="task4" targetRef="join" />
    <bpmn:sequenceFlow id="f6" sourceRef="join" targetRef="end" />
"#;

// 19. Duplicate Node IDs
const BODY_19_DUPLICATE_IDS: &str = r#"
    <bpmn:startEvent id="start" />
    <bpmn:serviceTask id="task1" name="Task 1">
      <bpmn:extensionElements>
        <zeebe:taskDefinition type="cbu.create" />
      </bpmn:extensionElements>
    </bpmn:serviceTask>
    <bpmn:serviceTask id="task1" name="Task 1 Duplicate">
      <bpmn:extensionElements>
        <zeebe:taskDefinition type="cbu.approve" />
      </bpmn:extensionElements>
    </bpmn:serviceTask>
    <bpmn:endEvent id="end" />
    <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="task1" />
    <bpmn:sequenceFlow id="f2" sourceRef="task1" targetRef="end" />
"#;

// 20. Missing Start Event
const BODY_20_MISSING_START: &str = r#"
    <bpmn:serviceTask id="task1" name="Task 1">
      <bpmn:extensionElements>
        <zeebe:taskDefinition type="cbu.create" />
      </bpmn:extensionElements>
    </bpmn:serviceTask>
    <bpmn:endEvent id="end" />
    <bpmn:sequenceFlow id="f1" sourceRef="task1" targetRef="end" />
"#;

#[test]
fn test_20_bpmn_compatibility_scenarios() {
    // Test 1: Linear Path (Valid SESE)
    let xml1 = wrap_process("p1", BODY_1_LINEAR);
    let plan1 = import_zeebe_bpmn(&xml1, "p1", false).unwrap();
    assert!(plan1.mathematically_proved);
    assert_eq!(plan1.nodes.len(), 3);

    // Test 2: Two Sequential Tasks (Valid SESE)
    let xml2 = wrap_process("p2", BODY_2_SEQ_TASKS);
    let plan2 = import_zeebe_bpmn(&xml2, "p2", false).unwrap();
    assert!(plan2.mathematically_proved);
    assert_eq!(plan2.nodes.len(), 4);

    // Test 3: XOR Gateway (Valid SESE)
    let xml3 = wrap_process("p3", BODY_3_XOR_SESE);
    let plan3 = import_zeebe_bpmn(&xml3, "p3", false).unwrap();
    assert!(plan3.mathematically_proved);
    assert!(plan3.nodes.contains_key("split"));
    assert!(plan3.nodes.contains_key("join"));

    // Test 4: Parallel Gateway (Valid SESE)
    let xml4 = wrap_process("p4", BODY_4_AND_SESE);
    let plan4 = import_zeebe_bpmn(&xml4, "p4", false).unwrap();
    assert!(plan4.mathematically_proved);
    assert!(matches!(
        plan4.nodes.get("split").unwrap(),
        ExecutionNode::Split(_)
    ));
    assert!(matches!(
        plan4.nodes.get("join").unwrap(),
        ExecutionNode::Join(_)
    ));

    // Test 5: Inclusive Gateway (Valid SESE)
    let xml5 = wrap_process("p5", BODY_5_OR_SESE);
    let plan5 = import_zeebe_bpmn(&xml5, "p5", false).unwrap();
    assert!(plan5.mathematically_proved);

    // Test 6: Unpaired XOR Gateway (Rejected in strict)
    let xml6 = wrap_process("p6", BODY_6_XOR_UNPAIRED);
    let res6 = import_zeebe_bpmn(&xml6, "p6", false);
    assert!(res6.is_err(), "Expected unpaired XOR to fail SESE");
    // Allowed in permissive but unproved
    let plan6_permissive = import_zeebe_bpmn(&xml6, "p6", true).unwrap();
    assert!(!plan6_permissive.mathematically_proved);
    assert!(plan6_permissive
        .unsafe_breeches
        .contains(&"BPMN_NON_SESE_TOPOLOGY".to_string()));

    // Test 7: Unpaired Parallel Gateway (Rejected in strict & permissive)
    let xml7 = wrap_process("p7", BODY_7_AND_UNPAIRED);
    assert!(import_zeebe_bpmn(&xml7, "p7", false).is_err());
    assert!(import_zeebe_bpmn(&xml7, "p7", true).is_err());

    // Test 8: Unpaired Inclusive Gateway (Rejected in strict & permissive)
    let xml8 = wrap_process("p8", BODY_8_OR_UNPAIRED);
    assert!(import_zeebe_bpmn(&xml8, "p8", false).is_err());
    assert!(import_zeebe_bpmn(&xml8, "p8", true).is_err());

    // Test 9: Mismatched Split/Join (Allowed in strict as SESE structure is valid)
    let xml9 = wrap_process("p9", BODY_9_MISMATCHED_GATES);
    let plan9 = import_zeebe_bpmn(&xml9, "p9", false).unwrap();
    assert!(plan9.mathematically_proved);

    // Test 10: Nested Exclusive Gateways (Valid SESE)
    let xml10 = wrap_process("p10", BODY_10_NESTED_XOR);
    let plan10 = import_zeebe_bpmn(&xml10, "p10", false).unwrap();
    assert!(plan10.mathematically_proved);

    // Test 11: Nested Parallel Gateways (Valid SESE)
    let xml11 = wrap_process("p11", BODY_11_NESTED_AND);
    let plan11 = import_zeebe_bpmn(&xml11, "p11", false).unwrap();
    assert!(plan11.mathematically_proved);

    // Test 12: Nested Mismatched Gateways (Valid SESE)
    let xml12 = wrap_process("p12", BODY_12_AND_INSIDE_XOR);
    let plan12 = import_zeebe_bpmn(&xml12, "p12", false).unwrap();
    assert!(plan12.mathematically_proved);

    // Test 13: Loop XOR Gateway (Rejected in strict, permissive compiles with SESE warning)
    let xml13 = wrap_process("p13", BODY_13_LOOP_XOR);
    let res13 = import_zeebe_bpmn(&xml13, "p13", false);
    assert!(res13.is_err());
    let plan13 = import_zeebe_bpmn(&xml13, "p13", true).unwrap();
    assert!(!plan13.mathematically_proved);
    assert!(plan13
        .unsafe_breeches
        .contains(&"BPMN_NON_SESE_TOPOLOGY".to_string()));

    // Test 14: Loop with Sequential inside (Rejected in strict, permissive compiles with SESE warning)
    let xml14 = wrap_process("p14", BODY_14_LOOP_SEQ);
    let res14 = import_zeebe_bpmn(&xml14, "p14", false);
    assert!(res14.is_err());
    let plan14 = import_zeebe_bpmn(&xml14, "p14", true).unwrap();
    assert!(!plan14.mathematically_proved);
    assert!(plan14
        .unsafe_breeches
        .contains(&"BPMN_NON_SESE_TOPOLOGY".to_string()));

    // Test 15: Boundary Timer Event (Hard-rejected in BOTH strict & permissive)
    let xml15 = wrap_process("p15", BODY_15_BOUNDARY_TIMER);
    assert!(import_zeebe_bpmn(&xml15, "p15", false).is_err());
    assert!(import_zeebe_bpmn(&xml15, "p15", true).is_err());

    // Test 16: Boundary Error Event (Hard-rejected in BOTH)
    let xml16 = wrap_process("p16", BODY_16_BOUNDARY_ERROR);
    assert!(import_zeebe_bpmn(&xml16, "p16", false).is_err());
    assert!(import_zeebe_bpmn(&xml16, "p16", true).is_err());

    // Test 17: FEEL Condition Warning / Missing Condition (Permissive compile warning)
    let xml17 = wrap_process("p17", BODY_17_MISSING_EXPR);
    assert!(import_zeebe_bpmn(&xml17, "p17", false).is_err());
    let plan17 = import_zeebe_bpmn(&xml17, "p17", true).unwrap();
    assert!(!plan17.mathematically_proved);
    assert!(plan17
        .unsafe_breeches
        .contains(&"FEEL_EVALUATION_WARNING".to_string()));

    // Test 18: Unparsed FEEL Expression (Missing =)
    let xml18 = wrap_process("p18", BODY_18_BAD_FEEL);
    assert!(import_zeebe_bpmn(&xml18, "p18", false).is_err());
    let plan18 = import_zeebe_bpmn(&xml18, "p18", true).unwrap();
    assert!(!plan18.mathematically_proved);
    assert!(plan18
        .unsafe_breeches
        .contains(&"FEEL_EVALUATION_WARNING".to_string()));

    // Test 19: Duplicate Node IDs (Rejected)
    let xml19 = wrap_process("p19", BODY_19_DUPLICATE_IDS);
    let res19 = import_zeebe_bpmn(&xml19, "p19", false);
    assert!(res19.is_err());

    // Test 20: Missing Start Event (Rejected)
    let xml20 = wrap_process("p20", BODY_20_MISSING_START);
    let res20 = import_zeebe_bpmn(&xml20, "p20", false);
    assert!(res20.is_err());
}
