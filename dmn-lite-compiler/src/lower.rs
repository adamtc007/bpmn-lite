//! Compilation pipeline: `DecisionAst` → `TypedDecision`.
//!
//! Combines schema resolution, type-checking, and IR lowering in a single
//! traversal. Errors are accumulated rather than short-circuiting: a type
//! error in one rule does not halt analysis of subsequent rules. An
//! input/output declaration error does halt further analysis (without resolved
//! schemas, no predicate or assignment can be checked).

use std::collections::HashMap;

use dmn_lite_types::{
    Catalogue, CompileError, CompileWarning, Domain,
    ast::{
        AssignmentAst, DecisionAst, HitPolicyAst, InputDeclAst, LiteralAst, NumberLitAst,
        OutputDeclAst, PredicateAst, RangeBound, SymbolAst, TypeRefAst, WhenAst,
    },
    ids::{DecisionId, DomainId, FieldId, NumberKind, RuleId, SourceSpan},
    ir::{
        ComparisonOp, EntityRef, FieldSchema, HitPolicy, ResolvedType, TypedAssignment,
        TypedDecision, TypedPredicate, TypedRule, TypedValue, TypedWhen,
    },
};

// ── Common field-declaration trait ───────────────────────────────────────────
// InputDeclAst and OutputDeclAst are structurally identical; this trait lets
// resolve_field_schema work over either without cloning.

trait AnyFieldDecl {
    fn decl_name(&self) -> &SymbolAst;
    fn decl_type_ref(&self) -> &TypeRefAst;
    fn decl_domain_ref(&self) -> &SymbolAst;
    fn decl_span(&self) -> SourceSpan;
}

impl AnyFieldDecl for InputDeclAst {
    fn decl_name(&self) -> &SymbolAst {
        &self.name
    }
    fn decl_type_ref(&self) -> &TypeRefAst {
        &self.type_ref
    }
    fn decl_domain_ref(&self) -> &SymbolAst {
        &self.domain_ref
    }
    fn decl_span(&self) -> SourceSpan {
        self.span
    }
}

impl AnyFieldDecl for OutputDeclAst {
    fn decl_name(&self) -> &SymbolAst {
        &self.name
    }
    fn decl_type_ref(&self) -> &TypeRefAst {
        &self.type_ref
    }
    fn decl_domain_ref(&self) -> &SymbolAst {
        &self.domain_ref
    }
    fn decl_span(&self) -> SourceSpan {
        self.span
    }
}

// ── Public entry point ────────────────────────────────────────────────────────

pub(crate) struct LowerResult {
    pub decision: Option<TypedDecision>,
    pub errors: Vec<CompileError>,
    pub warnings: Vec<CompileWarning>,
}

pub(crate) fn lower(ast: &DecisionAst, catalogue: &Catalogue) -> LowerResult {
    let mut errors: Vec<CompileError> = Vec::new();
    let mut warnings: Vec<CompileWarning> = Vec::new();
    let mut resolved_entities: Vec<EntityRef> = Vec::new();

    // ── Pre-flight: resolve input and output schemas ───────────────────────
    let Some((input_schema, input_map)) =
        resolve_field_schema(&ast.inputs, true, catalogue, &mut errors, &mut warnings)
    else {
        return LowerResult {
            decision: None,
            errors,
            warnings,
        };
    };
    let Some((output_schema, output_map)) =
        resolve_field_schema(&ast.outputs, false, catalogue, &mut errors, &mut warnings)
    else {
        return LowerResult {
            decision: None,
            errors,
            warnings,
        };
    };

    if !errors.is_empty() {
        return LowerResult {
            decision: None,
            errors,
            warnings,
        };
    }

    // Empty inputs / outputs
    if ast.inputs.is_empty() {
        errors.push(CompileError::EmptyInputs { span: ast.span });
    }
    if ast.outputs.is_empty() {
        errors.push(CompileError::EmptyOutputs { span: ast.span });
    }
    if !errors.is_empty() {
        return LowerResult {
            decision: None,
            errors,
            warnings,
        };
    }

    // Empty rules → warning
    if ast.rules.is_empty() {
        warnings.push(CompileWarning::EmptyRules { span: ast.span });
    }

    // ── Hit policy ─────────────────────────────────────────────────────────
    let hit_policy = match ast.hit_policy {
        HitPolicyAst::Unique(_) => HitPolicy::Unique,
        HitPolicyAst::First(_) => HitPolicy::First,
    };

    // ── Duplicate rule IDs ─────────────────────────────────────────────────
    let mut rule_id_seen: HashMap<&str, SourceSpan> = HashMap::new();
    for rule in &ast.rules {
        if let Some(&prev) = rule_id_seen.get(rule.id.name.as_str()) {
            errors.push(CompileError::DuplicateRuleId {
                name: rule.id.name.clone(),
                span: rule.id.span,
                previous: prev,
            });
        } else {
            rule_id_seen.insert(&rule.id.name, rule.id.span);
        }
    }

    // ── Per-rule analysis ──────────────────────────────────────────────────
    let mut typed_rules: Vec<TypedRule> = Vec::with_capacity(ast.rules.len());
    let mut catch_all_span: Option<SourceSpan> = None;

    for (rule_idx, rule) in ast.rules.iter().enumerate() {
        let rule_id = RuleId(rule_idx);

        // Catch-all tracking
        if let WhenAst::CatchAll(s) = &rule.when {
            if let Some(prev) = catch_all_span {
                errors.push(CompileError::MultipleCatchAllRules {
                    span: *s,
                    previous: prev,
                });
            } else {
                catch_all_span = Some(*s);
            }
        } else if let Some(ca) = catch_all_span {
            // Rule after catch-all under FIRST
            if hit_policy == HitPolicy::First {
                errors.push(CompileError::UnreachableAfterCatchAll {
                    rule: rule.id.name.clone(),
                    span: rule.span,
                    catch_all: ca,
                });
            }
        }

        // Lower :when
        let typed_when = lower_when(
            &rule.when,
            &input_schema,
            &input_map,
            catalogue,
            &mut errors,
            &mut resolved_entities,
        );

        // Lower :then
        let typed_then = lower_then(
            &rule.then,
            &rule.id.name,
            &output_schema,
            &output_map,
            catalogue,
            &mut errors,
            &mut resolved_entities,
        );

        typed_rules.push(TypedRule {
            rule_id,
            rule_name: rule.id.name.clone(),
            when: typed_when,
            then: typed_then,
            source_span: rule.span,
        });
    }

    // ── Decision identity ──────────────────────────────────────────────────
    let decision_id = DecisionId(
        ast.decision_id
            .as_ref()
            .map(|s| s.value.clone())
            .unwrap_or_else(|| ast.name.name.clone()),
    );

    let typed_decision = TypedDecision {
        decision_id,
        name: ast.name.name.clone(),
        hit_policy,
        input_schema,
        output_schema,
        rules: typed_rules,
        resolved_entities,
        source_span: ast.span,
    };

    // Return partial_decision if there are errors (schemas resolved = useful partial)
    LowerResult {
        decision: Some(typed_decision),
        errors,
        warnings,
    }
}

// ── Schema resolution ─────────────────────────────────────────────────────────

/// Resolve input or output field declarations into a schema vector and a
/// name→FieldId lookup map. Returns `None` if any error is fatal (meaning
/// further analysis cannot proceed).
fn resolve_field_schema<D: AnyFieldDecl>(
    decls: &[D],
    is_input: bool,
    catalogue: &Catalogue,
    errors: &mut Vec<CompileError>,
    warnings: &mut Vec<CompileWarning>,
) -> Option<(Vec<FieldSchema>, HashMap<String, FieldId>)> {
    let mut schema: Vec<FieldSchema> = Vec::with_capacity(decls.len());
    let mut name_map: HashMap<String, FieldId> = HashMap::new();
    let mut name_seen: HashMap<String, SourceSpan> = HashMap::new();
    let mut had_fatal = false;

    for (idx, decl) in decls.iter().enumerate() {
        let field_id = FieldId(idx);
        let name = decl.decl_name();
        let name_str = name.name.clone();

        // Duplicate field name check
        if let Some(&prev) = name_seen.get(&name_str) {
            if is_input {
                errors.push(CompileError::DuplicateInputField {
                    name: name_str.clone(),
                    span: name.span,
                    previous: prev,
                });
            } else {
                errors.push(CompileError::DuplicateOutputField {
                    name: name_str.clone(),
                    span: name.span,
                    previous: prev,
                });
            }
            had_fatal = true;
            continue;
        }
        name_seen.insert(name_str.clone(), name.span);

        // Resolve type and domain
        let field_type = match resolve_field_type(
            decl.decl_name(),
            decl.decl_type_ref(),
            decl.decl_domain_ref(),
            catalogue,
            errors,
            warnings,
        ) {
            Some(t) => t,
            None => {
                had_fatal = true;
                continue;
            }
        };

        name_map.insert(name_str.clone(), field_id);
        schema.push(FieldSchema {
            field_id,
            name: name_str,
            field_type,
            source_span: decl.decl_span(),
        });
    }

    if had_fatal {
        None
    } else {
        Some((schema, name_map))
    }
}

fn resolve_field_type(
    name: &SymbolAst,
    type_ref: &TypeRefAst,
    domain_ref: &SymbolAst,
    catalogue: &Catalogue,
    errors: &mut Vec<CompileError>,
    warnings: &mut Vec<CompileWarning>,
) -> Option<ResolvedType> {
    let domain_name = &domain_ref.name;
    let domain_span = domain_ref.span;

    match type_ref {
        TypeRefAst::Enum(_) => {
            // Enum: domain required and must resolve
            let domain = match catalogue.resolve_domain(domain_name) {
                Some(d) => d,
                None => {
                    errors.push(CompileError::UnknownDomain {
                        name: domain_name.clone(),
                        span: domain_span,
                    });
                    return None;
                }
            };
            Some(ResolvedType::Enum {
                domain_id: domain.domain_id,
            })
        }
        TypeRefAst::Bool(_) => {
            // Non-enum: domain resolved (must exist) but advisory
            if catalogue.resolve_domain(domain_name).is_none() {
                errors.push(CompileError::UnknownDomain {
                    name: domain_name.clone(),
                    span: domain_span,
                });
                return None;
            }
            warnings.push(CompileWarning::DomainOnNonEnum {
                field: name.name.clone(),
                type_name: "bool".into(),
                domain: domain_name.clone(),
                span: domain_span,
            });
            Some(ResolvedType::Bool)
        }
        TypeRefAst::Integer(_) => {
            if catalogue.resolve_domain(domain_name).is_none() {
                errors.push(CompileError::UnknownDomain {
                    name: domain_name.clone(),
                    span: domain_span,
                });
                return None;
            }
            warnings.push(CompileWarning::DomainOnNonEnum {
                field: name.name.clone(),
                type_name: "integer".into(),
                domain: domain_name.clone(),
                span: domain_span,
            });
            Some(ResolvedType::Integer)
        }
        TypeRefAst::Decimal(_) => {
            if catalogue.resolve_domain(domain_name).is_none() {
                errors.push(CompileError::UnknownDomain {
                    name: domain_name.clone(),
                    span: domain_span,
                });
                return None;
            }
            warnings.push(CompileWarning::DomainOnNonEnum {
                field: name.name.clone(),
                type_name: "decimal".into(),
                domain: domain_name.clone(),
                span: domain_span,
            });
            Some(ResolvedType::Decimal)
        }
        TypeRefAst::String(_) => {
            if catalogue.resolve_domain(domain_name).is_none() {
                errors.push(CompileError::UnknownDomain {
                    name: domain_name.clone(),
                    span: domain_span,
                });
                return None;
            }
            warnings.push(CompileWarning::DomainOnNonEnum {
                field: name.name.clone(),
                type_name: "string".into(),
                domain: domain_name.clone(),
                span: domain_span,
            });
            Some(ResolvedType::Str)
        }
    }
}

// ── :when lowering ────────────────────────────────────────────────────────────

fn lower_when(
    when: &WhenAst,
    input_schema: &[FieldSchema],
    input_map: &HashMap<String, FieldId>,
    catalogue: &Catalogue,
    errors: &mut Vec<CompileError>,
    entities: &mut Vec<EntityRef>,
) -> TypedWhen {
    match when {
        WhenAst::CatchAll(span) => TypedWhen::CatchAll(*span),
        WhenAst::Predicates(preds, span) => {
            let typed: Vec<TypedPredicate> = preds
                .iter()
                .filter_map(|p| {
                    lower_predicate(p, input_schema, input_map, catalogue, errors, entities)
                })
                .collect();
            TypedWhen::Predicates(typed, *span)
        }
    }
}

fn lower_predicate(
    pred: &PredicateAst,
    input_schema: &[FieldSchema],
    input_map: &HashMap<String, FieldId>,
    catalogue: &Catalogue,
    errors: &mut Vec<CompileError>,
    entities: &mut Vec<EntityRef>,
) -> Option<TypedPredicate> {
    match pred {
        PredicateAst::Eq { field, value, span } => {
            let (fid, ftype) = resolve_input_field(field, input_map, input_schema, errors)?;
            let tv =
                lower_literal_for_field(value, &ftype, &field.name, catalogue, errors, entities)?;
            Some(TypedPredicate::Comparison {
                field: fid,
                op: ComparisonOp::Eq,
                rhs: tv,
                source_span: *span,
            })
        }
        PredicateAst::NotEq { field, value, span } => {
            let (fid, ftype) = resolve_input_field(field, input_map, input_schema, errors)?;
            let tv =
                lower_literal_for_field(value, &ftype, &field.name, catalogue, errors, entities)?;
            Some(TypedPredicate::Comparison {
                field: fid,
                op: ComparisonOp::NotEq,
                rhs: tv,
                source_span: *span,
            })
        }
        PredicateAst::Lt { field, value, span } => lower_comparison_pred(
            field,
            ComparisonOp::Lt,
            value,
            *span,
            input_map,
            input_schema,
            errors,
        ),
        PredicateAst::Le { field, value, span } => lower_comparison_pred(
            field,
            ComparisonOp::Le,
            value,
            *span,
            input_map,
            input_schema,
            errors,
        ),
        PredicateAst::Gt { field, value, span } => lower_comparison_pred(
            field,
            ComparisonOp::Gt,
            value,
            *span,
            input_map,
            input_schema,
            errors,
        ),
        PredicateAst::Ge { field, value, span } => lower_comparison_pred(
            field,
            ComparisonOp::Ge,
            value,
            *span,
            input_map,
            input_schema,
            errors,
        ),
        PredicateAst::InSet {
            field,
            values,
            span,
        } => {
            let (fid, ftype) = resolve_input_field(field, input_map, input_schema, errors)?;
            let mut typed_values = Vec::with_capacity(values.len());
            for (i, v) in values.iter().enumerate() {
                let tv =
                    lower_literal_for_field(v, &ftype, &field.name, catalogue, errors, entities);
                if let Some(tv) = tv {
                    // Collect enum entities
                    if let TypedValue::Enum {
                        domain_id,
                        value_id,
                    } = &tv
                    {
                        entities.push(EntityRef {
                            domain_id: *domain_id,
                            value_id: *value_id,
                            source_span: v.span(),
                        });
                    }
                    // Type consistency check vs first element
                    if let Some(first) = typed_values.first()
                        && type_name_of(first) != type_name_of(&tv)
                    {
                        errors.push(CompileError::SetMemberTypeMismatch {
                            field: field.name.clone(),
                            field_type: ftype.type_name().into(),
                            element_type: type_name_of(&tv).into(),
                            index: i,
                            span: v.span(),
                        });
                    }
                    typed_values.push(tv);
                }
            }
            Some(TypedPredicate::InSet {
                field: fid,
                values: typed_values,
                source_span: *span,
            })
        }
        PredicateAst::Range {
            field,
            lower,
            upper,
            lower_inclusive,
            upper_inclusive,
            span,
        } => {
            let (fid, ftype) = resolve_input_field(field, input_map, input_schema, errors)?;
            if !ftype.is_numeric() {
                errors.push(CompileError::RangeOnNonNumeric {
                    field: field.name.clone(),
                    field_type: ftype.type_name().into(),
                    span: *span,
                });
                return None;
            }
            let lower_tv = lower_range_bound(lower, &ftype, errors);
            let upper_tv = lower_range_bound(upper, &ftype, errors);
            Some(TypedPredicate::Range {
                field: fid,
                lower: lower_tv,
                upper: upper_tv,
                lower_inclusive: *lower_inclusive,
                upper_inclusive: *upper_inclusive,
                source_span: *span,
            })
        }
        PredicateAst::IsNull { field, span } => {
            let (fid, _) = resolve_input_field(field, input_map, input_schema, errors)?;
            Some(TypedPredicate::IsNull {
                field: fid,
                source_span: *span,
            })
        }
        PredicateAst::IsNotNull { field, span } => {
            let (fid, _) = resolve_input_field(field, input_map, input_schema, errors)?;
            Some(TypedPredicate::IsNotNull {
                field: fid,
                source_span: *span,
            })
        }
        PredicateAst::Not { inner, span } => {
            let inner_typed =
                lower_predicate(inner, input_schema, input_map, catalogue, errors, entities)?;
            Some(TypedPredicate::Not {
                inner: Box::new(inner_typed),
                source_span: *span,
            })
        }
        PredicateAst::And { items, span } => {
            let typed_items: Vec<TypedPredicate> = items
                .iter()
                .filter_map(|p| {
                    lower_predicate(p, input_schema, input_map, catalogue, errors, entities)
                })
                .collect();
            Some(TypedPredicate::And {
                items: typed_items,
                source_span: *span,
            })
        }
        PredicateAst::Or { items, span } => {
            let typed_items: Vec<TypedPredicate> = items
                .iter()
                .filter_map(|p| {
                    lower_predicate(p, input_schema, input_map, catalogue, errors, entities)
                })
                .collect();
            Some(TypedPredicate::Or {
                items: typed_items,
                source_span: *span,
            })
        }
    }
}

fn lower_comparison_pred(
    field: &dmn_lite_types::ast::SymbolAst,
    op: ComparisonOp,
    value: &NumberLitAst,
    span: SourceSpan,
    input_map: &HashMap<String, FieldId>,
    input_schema: &[FieldSchema],
    errors: &mut Vec<CompileError>,
) -> Option<TypedPredicate> {
    let (fid, ftype) = resolve_input_field(field, input_map, input_schema, errors)?;
    if !ftype.is_numeric() {
        errors.push(CompileError::OrderedComparisonOnNonNumeric {
            field: field.name.clone(),
            field_type: ftype.type_name().into(),
            op: op.as_str().into(),
            span,
        });
        return None;
    }
    let tv = lower_number_for_type(value, &ftype, errors)?;
    Some(TypedPredicate::Comparison {
        field: fid,
        op,
        rhs: tv,
        source_span: span,
    })
}

fn lower_range_bound(
    bound: &RangeBound,
    ftype: &ResolvedType,
    errors: &mut Vec<CompileError>,
) -> Option<TypedValue> {
    match bound {
        RangeBound::Unbounded(_) => None,
        RangeBound::Value(n) => lower_number_for_type(n, ftype, errors),
    }
}

// ── :then lowering ────────────────────────────────────────────────────────────

fn lower_then(
    assignments: &[AssignmentAst],
    rule_name: &str,
    output_schema: &[FieldSchema],
    output_map: &HashMap<String, FieldId>,
    catalogue: &Catalogue,
    errors: &mut Vec<CompileError>,
    entities: &mut Vec<EntityRef>,
) -> Vec<TypedAssignment> {
    let mut typed: Vec<TypedAssignment> = Vec::new();
    let mut assigned: HashMap<FieldId, SourceSpan> = HashMap::new();

    for a in assignments {
        let fid = match output_map.get(&a.output.name) {
            Some(&id) => id,
            None => {
                errors.push(CompileError::UnknownOutputField {
                    name: a.output.name.clone(),
                    span: a.output.span,
                });
                continue;
            }
        };
        let ftype = &output_schema[fid.0].field_type;

        // Duplicate output assignment
        if let Some(&prev) = assigned.get(&fid) {
            errors.push(CompileError::DuplicateOutputAssignment {
                rule: rule_name.to_owned(),
                output: a.output.name.clone(),
                span: a.span,
                previous: prev,
            });
            continue;
        }
        assigned.insert(fid, a.span);

        let tv = lower_literal_for_assignment(
            &a.value,
            ftype,
            &a.output.name,
            catalogue,
            errors,
            entities,
        );
        if let Some(tv) = tv {
            typed.push(TypedAssignment {
                output_field: fid,
                value: tv,
                source_span: a.span,
            });
        }
    }

    // Check completeness: every declared output must be assigned
    for field in output_schema {
        if !assigned.contains_key(&field.field_id) {
            // Use the span of the last parsed assignment block, or the rule span (approximate)
            let span = assignments
                .last()
                .map(|a| a.span)
                .unwrap_or(SourceSpan::new(0, 0));
            errors.push(CompileError::MissingOutputAssignment {
                rule: rule_name.to_owned(),
                output: field.name.clone(),
                span,
            });
        }
    }

    typed
}

// ── Type-level literal lowering ───────────────────────────────────────────────

fn lower_literal_for_field(
    lit: &LiteralAst,
    ftype: &ResolvedType,
    field_name: &str,
    catalogue: &Catalogue,
    errors: &mut Vec<CompileError>,
    entities: &mut Vec<EntityRef>,
) -> Option<TypedValue> {
    lower_literal_for_field_ctx(lit, ftype, field_name, catalogue, errors, entities, false)
}

fn lower_literal_for_assignment(
    lit: &LiteralAst,
    ftype: &ResolvedType,
    field_name: &str,
    catalogue: &Catalogue,
    errors: &mut Vec<CompileError>,
    entities: &mut Vec<EntityRef>,
) -> Option<TypedValue> {
    lower_literal_for_field_ctx(lit, ftype, field_name, catalogue, errors, entities, true)
}

fn lower_literal_for_field_ctx(
    lit: &LiteralAst,
    ftype: &ResolvedType,
    field_name: &str,
    catalogue: &Catalogue,
    errors: &mut Vec<CompileError>,
    entities: &mut Vec<EntityRef>,
    is_assignment: bool,
) -> Option<TypedValue> {
    match (ftype, lit) {
        // ── Enum ──────────────────────────────────────────────────────────
        (ResolvedType::Enum { domain_id }, LiteralAst::Symbol(sym)) => {
            let domain = find_domain_by_id(catalogue, *domain_id)?;
            match domain.resolve_value(&sym.name) {
                Some(value_id) => {
                    let entity = EntityRef {
                        domain_id: *domain_id,
                        value_id,
                        source_span: sym.span,
                    };
                    entities.push(entity);
                    Some(TypedValue::Enum {
                        domain_id: *domain_id,
                        value_id,
                    })
                }
                None => {
                    errors.push(CompileError::UnknownDomainValue {
                        domain: domain.name.clone(),
                        symbol: sym.name.clone(),
                        span: sym.span,
                    });
                    None
                }
            }
        }
        (ResolvedType::Enum { .. }, other) => {
            push_type_mismatch(
                is_assignment,
                field_name,
                "enum",
                literal_type_name(other),
                lit.span(),
                errors,
            );
            None
        }

        // ── Bool ──────────────────────────────────────────────────────────
        (ResolvedType::Bool, LiteralAst::Boolean { value, .. }) => Some(TypedValue::Bool(*value)),
        (ResolvedType::Bool, other) => {
            push_type_mismatch(
                is_assignment,
                field_name,
                "bool",
                literal_type_name(other),
                lit.span(),
                errors,
            );
            None
        }

        // ── Integer ───────────────────────────────────────────────────────
        (ResolvedType::Integer, LiteralAst::Number(n)) if n.kind == NumberKind::Integer => {
            parse_integer(&n.text, n.span, errors)
        }
        (ResolvedType::Integer, LiteralAst::Number(n)) if n.kind == NumberKind::Decimal => {
            // Decimal literal against integer field → error
            push_type_mismatch(
                is_assignment,
                field_name,
                "integer",
                "decimal",
                n.span,
                errors,
            );
            None
        }
        (ResolvedType::Integer, other) => {
            push_type_mismatch(
                is_assignment,
                field_name,
                "integer",
                literal_type_name(other),
                lit.span(),
                errors,
            );
            None
        }

        // ── Decimal ───────────────────────────────────────────────────────
        (ResolvedType::Decimal, LiteralAst::Number(n)) => {
            // Both integer and decimal literals accepted; integer is widened
            parse_decimal(&n.text, n.span, errors)
        }
        (ResolvedType::Decimal, other) => {
            push_type_mismatch(
                is_assignment,
                field_name,
                "decimal",
                literal_type_name(other),
                lit.span(),
                errors,
            );
            None
        }

        // ── String ────────────────────────────────────────────────────────
        (ResolvedType::Str, LiteralAst::String(s)) => Some(TypedValue::Str(s.value.clone())),
        (ResolvedType::Str, other) => {
            push_type_mismatch(
                is_assignment,
                field_name,
                "string",
                literal_type_name(other),
                lit.span(),
                errors,
            );
            None
        }
    }
}

fn push_type_mismatch(
    is_assignment: bool,
    field: &str,
    field_type: &str,
    literal_type: &str,
    span: SourceSpan,
    errors: &mut Vec<CompileError>,
) {
    if is_assignment {
        errors.push(CompileError::AssignmentTypeMismatch {
            output: field.to_owned(),
            output_type: field_type.into(),
            literal_type: literal_type.into(),
            span,
        });
    } else {
        errors.push(CompileError::PredicateTypeMismatch {
            field: field.to_owned(),
            field_type: field_type.into(),
            literal_type: literal_type.into(),
            span,
        });
    }
}

fn lower_number_for_type(
    n: &NumberLitAst,
    ftype: &ResolvedType,
    errors: &mut Vec<CompileError>,
) -> Option<TypedValue> {
    match ftype {
        ResolvedType::Integer => {
            if n.kind == NumberKind::Decimal {
                errors.push(CompileError::PredicateTypeMismatch {
                    field: String::new(),
                    field_type: "integer".into(),
                    literal_type: "decimal".into(),
                    span: n.span,
                });
                None
            } else {
                parse_integer(&n.text, n.span, errors)
            }
        }
        ResolvedType::Decimal => parse_decimal(&n.text, n.span, errors),
        _ => unreachable!("lower_number_for_type called on non-numeric type"),
    }
}

fn parse_integer(
    text: &str,
    span: SourceSpan,
    errors: &mut Vec<CompileError>,
) -> Option<TypedValue> {
    match text.parse::<i64>() {
        Ok(v) => Some(TypedValue::Integer(v)),
        Err(_) => {
            errors.push(CompileError::PredicateTypeMismatch {
                field: String::new(),
                field_type: "integer".into(),
                literal_type: "overflow".into(),
                span,
            });
            None
        }
    }
}

fn parse_decimal(
    text: &str,
    span: SourceSpan,
    errors: &mut Vec<CompileError>,
) -> Option<TypedValue> {
    match text.parse::<f64>() {
        Ok(v) => Some(TypedValue::Decimal(v)),
        Err(_) => {
            errors.push(CompileError::PredicateTypeMismatch {
                field: String::new(),
                field_type: "decimal".into(),
                literal_type: "overflow".into(),
                span,
            });
            None
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn resolve_input_field(
    sym: &dmn_lite_types::ast::SymbolAst,
    input_map: &HashMap<String, FieldId>,
    input_schema: &[FieldSchema],
    errors: &mut Vec<CompileError>,
) -> Option<(FieldId, ResolvedType)> {
    match input_map.get(&sym.name) {
        Some(&fid) => Some((fid, input_schema[fid.0].field_type.clone())),
        None => {
            errors.push(CompileError::UnknownInputField {
                name: sym.name.clone(),
                span: sym.span,
            });
            None
        }
    }
}

fn find_domain_by_id(catalogue: &Catalogue, domain_id: DomainId) -> Option<&Domain> {
    catalogue.domains().find(|d| d.domain_id == domain_id)
}

fn literal_type_name(lit: &LiteralAst) -> &'static str {
    match lit {
        LiteralAst::Symbol(_) => "symbol",
        LiteralAst::String(_) => "string",
        LiteralAst::Number(n) => {
            if n.kind == NumberKind::Integer {
                "integer"
            } else {
                "decimal"
            }
        }
        LiteralAst::Boolean { .. } => "bool",
    }
}

fn type_name_of(tv: &TypedValue) -> &'static str {
    match tv {
        TypedValue::Enum { .. } => "enum",
        TypedValue::Bool(_) => "bool",
        TypedValue::Integer(_) => "integer",
        TypedValue::Decimal(_) => "decimal",
        TypedValue::Str(_) => "string",
        TypedValue::Null => "null",
    }
}
