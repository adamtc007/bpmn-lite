//! Artifact hash computation using BLAKE3.
//!
//! Per `dmn-lite-semantics.md` §3.2.4:
//!
//! ```text
//! artifact_hash = blake3(normalised_source || resolved_entity_ids || compiled_ir)
//! ```
//!
//! The Sem OS snapshot ID is metadata, not a hash input.

use dmn_lite_types::{
    compiled::{ArtifactHash, RangeEntry},
    instr::Instr,
    ir::{EntityRef, TypedDecision, TypedValue},
};

use crate::emit::serialize_typed_value;

/// Compute the BLAKE3 artifact hash for a compiled decision.
pub fn compute_artifact_hash(
    source_text: &str,
    typed: &TypedDecision,
    instructions: &[Instr],
    const_pool: &[TypedValue],
    range_pool: &[RangeEntry],
) -> ArtifactHash {
    let mut hasher = blake3::Hasher::new();

    // 1. Normalised source
    let normalised = normalise_source(source_text);
    let ns_bytes = normalised.as_bytes();
    hasher.update(&(ns_bytes.len() as u64).to_le_bytes());
    hasher.update(ns_bytes);

    // 2. Resolved entity IDs (domain_id + value_id per entity, source order)
    let entity_bytes = serialise_entity_refs(&typed.resolved_entities);
    hasher.update(&(entity_bytes.len() as u64).to_le_bytes());
    hasher.update(&entity_bytes);

    // 3. Compiled IR (instructions + pools)
    let ir_bytes = serialise_ir(instructions, const_pool, range_pool);
    hasher.update(&(ir_bytes.len() as u64).to_le_bytes());
    hasher.update(&ir_bytes);

    let digest = hasher.finalize();
    ArtifactHash::from_bytes(*digest.as_bytes())
}

// ── Source normalisation ──────────────────────────────────────────────────────

/// Strip comments and collapse whitespace for deterministic hashing.
///
/// Rules:
/// 1. Remove `;` line comments (`;` to end of line).
/// 2. Replace every run of whitespace (spaces, tabs, newlines) with a single space.
/// 3. Trim leading and trailing whitespace.
pub(crate) fn normalise_source(source: &str) -> String {
    // Strip comments
    let mut stripped = String::with_capacity(source.len());
    for line in source.lines() {
        let without_comment = if let Some(pos) = line.find(';') {
            &line[..pos]
        } else {
            line
        };
        if !without_comment.trim().is_empty() {
            stripped.push_str(without_comment);
            stripped.push('\n');
        }
    }
    // Collapse whitespace
    let mut normalised = String::with_capacity(stripped.len());
    let mut in_ws = false;
    for ch in stripped.chars() {
        if ch.is_ascii_whitespace() {
            if !in_ws {
                normalised.push(' ');
                in_ws = true;
            }
        } else {
            normalised.push(ch);
            in_ws = false;
        }
    }
    normalised.trim().to_owned()
}

// ── Entity ID serialisation ───────────────────────────────────────────────────

fn serialise_entity_refs(entities: &[EntityRef]) -> Vec<u8> {
    let mut out = Vec::with_capacity(entities.len() * 32);
    for e in entities {
        out.extend(e.domain_id.0.as_bytes());
        out.extend(e.value_id.0.as_bytes());
        // source_span is NOT included — identity is about what the entity IS,
        // not where it appeared.
    }
    out
}

// ── IR serialisation ──────────────────────────────────────────────────────────

fn serialise_ir(
    instructions: &[Instr],
    const_pool: &[TypedValue],
    range_pool: &[RangeEntry],
) -> Vec<u8> {
    let mut out = Vec::new();
    // Instruction stream
    out.extend((instructions.len() as u32).to_le_bytes());
    for instr in instructions {
        serialise_instr(&mut out, instr);
    }
    // Const pool
    out.extend((const_pool.len() as u32).to_le_bytes());
    for v in const_pool {
        let bytes = serialize_typed_value(v);
        out.extend((bytes.len() as u32).to_le_bytes());
        out.extend(&bytes);
    }
    // Range pool
    out.extend((range_pool.len() as u32).to_le_bytes());
    for r in range_pool {
        out.push(r.lower_inclusive as u8);
        out.push(r.upper_inclusive as u8);
        match &r.lower {
            None => out.push(0),
            Some(v) => {
                out.push(1);
                let b = serialize_typed_value(v);
                out.extend((b.len() as u32).to_le_bytes());
                out.extend(&b);
            }
        }
        match &r.upper {
            None => out.push(0),
            Some(v) => {
                out.push(1);
                let b = serialize_typed_value(v);
                out.extend((b.len() as u32).to_le_bytes());
                out.extend(&b);
            }
        }
    }
    out
}

fn serialise_instr(out: &mut Vec<u8>, instr: &Instr) {
    match instr {
        Instr::LoadField(f) => {
            out.push(0x01);
            out.extend(f.0.to_le_bytes());
        }
        Instr::PushConst(c) => {
            out.push(0x02);
            out.extend(c.0.to_le_bytes());
        }
        Instr::PushConstSet(s) => {
            out.push(0x03);
            out.extend(s.0.to_le_bytes());
        }
        Instr::Pop => out.push(0x04),
        Instr::Dup => out.push(0x05),
        Instr::Eq => out.push(0x10),
        Instr::NotEq => out.push(0x11),
        Instr::Lt => out.push(0x12),
        Instr::Le => out.push(0x13),
        Instr::Gt => out.push(0x14),
        Instr::Ge => out.push(0x15),
        Instr::InSet => out.push(0x20),
        Instr::RangeCheck(r) => {
            out.push(0x21);
            out.extend(r.0.to_le_bytes());
        }
        Instr::IsNull => out.push(0x30),
        Instr::IsNotNull => out.push(0x31),
        Instr::And => out.push(0x40),
        Instr::Or => out.push(0x41),
        Instr::Not => out.push(0x42),
        Instr::Br(a) => {
            out.push(0x50);
            out.extend(a.to_le_bytes());
        }
        Instr::BrFalse(a) => {
            out.push(0x51);
            out.extend(a.to_le_bytes());
        }
        Instr::BrTrue(a) => {
            out.push(0x52);
            out.extend(a.to_le_bytes());
        }
        Instr::RuleMatched(r) => {
            out.push(0x60);
            out.extend((r.0 as u32).to_le_bytes());
        }
        Instr::StoreOutputTos(f) => {
            out.push(0x61);
            out.extend(f.0.to_le_bytes());
        }
        Instr::StoreOutput(f, c) => {
            out.push(0x62);
            out.extend(f.0.to_le_bytes());
            out.extend(c.0.to_le_bytes());
        }
        Instr::EndDecision => out.push(0x70),
        // Reserved — should not appear in v0.1 artifacts, but serialise for completeness.
        Instr::Call(b) => {
            out.push(0xE0);
            out.extend(b.0.to_le_bytes());
        }
        Instr::Return => out.push(0xE1),
        Instr::ForAllBegin {
            collection,
            bound_var,
            end,
        } => {
            out.push(0xE2);
            out.extend(collection.0.to_le_bytes());
            out.extend(bound_var.0.to_le_bytes());
            out.extend(end.to_le_bytes());
        }
        Instr::ForAllEnd => out.push(0xE3),
        Instr::AggregateBegin {
            collection,
            bound_var,
            op,
            end,
        } => {
            out.push(0xE4);
            out.extend(collection.0.to_le_bytes());
            out.extend(bound_var.0.to_le_bytes());
            out.push(*op as u8);
            out.extend(end.to_le_bytes());
        }
        Instr::AggregateEnd => out.push(0xE5),
        Instr::LoadPath(p) => {
            out.push(0xE6);
            out.extend(p.0.to_le_bytes());
        }
    }
}
