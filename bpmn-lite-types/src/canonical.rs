//! D3 — canonical binary encoding primitives (EOP-VS-BPMN-ISA-002 §6, V2.1).
//!
//! Generalizes `ffi-types/src/canonical.rs`'s tagged, length-prefixed
//! encoding into a reusable, **decode-capable** module (the original is
//! write-only: a one-way BLAKE3 hash with no decoder, which cannot satisfy
//! the round-trip fixed-point law `canonicalize(decode(b)) == b`).
//!
//! Scope (V0.9/V2.1 disposition, 2026-07-21; reversed same-day to full
//! supersession — see `docs/todo/EOP-PLAN-BPMN-ISA-002.md` V2 tranche
//! record): this module is the **single** canonical encoder for the entire
//! D3 Ring 2 hash domain — `ProcessInstance`, `Fiber`, `WaitState`,
//! `ProcessState`, `Value`, the session stack, and the D1 concurrency-table
//! / pending-effects surface all encode through `CanonicalEncode` here.
//! `ProcessInstance::try_canonical_hash_bytes` is the one fallible
//! exception (opaque externally-supplied JSON in `domain_payload`/
//! `placeholder_values` can fail to parse — see `CanonicalJsonError` and
//! `encode_canonical_json` below); every other impl in this module is
//! infallible by construction. `bpmn_lite_types::persistence`'s
//! `SnapshotEnvelope` still uses `serde_json` for its on-disk **storage**
//! format (`canonical_bytes`/`decode`) — that is the envelope's wire
//! format, a distinct concern from the Ring 2 **hash domain**, which no
//! longer touches `serde_json` at all (`state_hash()`).
//!
//! Wire format (encoder side is append-only to a `Vec<u8>`; decoder side is
//! a cursor over a `&[u8]` slice):
//!
//! - `u8`  = 1 byte, verbatim.
//! - `u16`/`u32`/`u64` = little-endian, fixed width.
//! - `bool` = `0x01` true / `0x00` false.
//! - `bytes` (fixed-size, e.g. `[u8; 32]`) = verbatim, no length prefix.
//! - `str`/`Vec<u8>` (variable-size) = `u32le` length prefix, then bytes.
//! - `Option<T>` = presence tag (`0x00` None, `0x01` Some) then `T` if Some.
//! - `Vec<T>` = `u32le` count, then each `T` in order (caller sorts first
//!   for anything that must be canonical over an unordered collection —
//!   `BTreeMap`/`BTreeSet` iteration is already sorted).
//! - `enum` = `u8` variant tag (assigned explicitly per type, never derived
//!   from declaration order implicitly — each `CanonicalEncode` impl below
//!   documents its tag table) then the variant's fields.

use std::collections::BTreeSet;

/// Growable canonical-byte sink. A thin `Vec<u8>` wrapper so encoders read
/// as a sequence of primitive writes, matching `ffi-types/src/canonical.rs`'s
/// style.
#[derive(Default)]
pub struct CanonicalWriter(Vec<u8>);

impl CanonicalWriter {
    pub fn new() -> Self {
        Self(Vec::new())
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }

    pub fn write_u8(&mut self, v: u8) {
        self.0.push(v);
    }

    pub fn write_u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }

    pub fn write_u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }

    pub fn write_u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }

    pub fn write_bool(&mut self, v: bool) {
        self.write_u8(if v { 0x01 } else { 0x00 });
    }

    pub fn write_bytes_fixed(&mut self, v: &[u8]) {
        self.0.extend_from_slice(v);
    }

    pub fn write_bytes(&mut self, v: &[u8]) {
        self.write_u32(v.len() as u32);
        self.0.extend_from_slice(v);
    }

    pub fn write_str(&mut self, v: &str) {
        self.write_bytes(v.as_bytes());
    }

    pub fn write_option<T>(&mut self, v: &Option<T>, encode_some: impl FnOnce(&mut Self, &T)) {
        match v {
            None => self.write_u8(0x00),
            Some(inner) => {
                self.write_u8(0x01);
                encode_some(self, inner);
            }
        }
    }

    pub fn write_seq<T>(
        &mut self,
        items: impl ExactSizeIterator<Item = T>,
        mut encode_item: impl FnMut(&mut Self, T),
    ) {
        self.write_u32(items.len() as u32);
        for item in items {
            encode_item(self, item);
        }
    }
}

/// Cursor over canonical bytes for decoding. Every read is bounds-checked;
/// truncated/malformed input is a `CanonicalDecodeError`, never a panic.
pub struct CanonicalReader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CanonicalDecodeError {
    #[error("canonical decode: unexpected end of input at offset {offset}")]
    UnexpectedEof { offset: usize },
    #[error("canonical decode: invalid UTF-8 in string field")]
    InvalidUtf8,
    #[error("canonical decode: unknown enum tag {tag} for {type_name}")]
    UnknownTag { tag: u8, type_name: &'static str },
    #[error("canonical decode: {0} trailing bytes after decode")]
    TrailingBytes(usize),
}

impl<'a> CanonicalReader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    /// Assert the reader consumed every byte — proves the encoder/decoder
    /// pair agree on the frame's exact length, catching truncation and
    /// over-read bugs alike.
    pub fn finish(self) -> Result<(), CanonicalDecodeError> {
        if self.pos == self.bytes.len() {
            Ok(())
        } else {
            Err(CanonicalDecodeError::TrailingBytes(
                self.bytes.len() - self.pos,
            ))
        }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], CanonicalDecodeError> {
        if self.pos + n > self.bytes.len() {
            return Err(CanonicalDecodeError::UnexpectedEof { offset: self.pos });
        }
        let slice = &self.bytes[self.pos..self.pos + n];
        self.pos += n;
        Ok(slice)
    }

    pub fn read_u8(&mut self) -> Result<u8, CanonicalDecodeError> {
        Ok(self.take(1)?[0])
    }

    pub fn read_u16(&mut self) -> Result<u16, CanonicalDecodeError> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn read_u32(&mut self) -> Result<u32, CanonicalDecodeError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn read_u64(&mut self) -> Result<u64, CanonicalDecodeError> {
        let b = self.take(8)?;
        Ok(u64::from_le_bytes(b.try_into().unwrap()))
    }

    pub fn read_bool(&mut self) -> Result<bool, CanonicalDecodeError> {
        match self.read_u8()? {
            0x00 => Ok(false),
            0x01 => Ok(true),
            tag => Err(CanonicalDecodeError::UnknownTag {
                tag,
                type_name: "bool",
            }),
        }
    }

    pub fn read_bytes_fixed<const N: usize>(&mut self) -> Result<[u8; N], CanonicalDecodeError> {
        let b = self.take(N)?;
        Ok(b.try_into().unwrap())
    }

    pub fn read_bytes(&mut self) -> Result<Vec<u8>, CanonicalDecodeError> {
        let len = self.read_u32()? as usize;
        Ok(self.take(len)?.to_vec())
    }

    pub fn read_str(&mut self) -> Result<String, CanonicalDecodeError> {
        String::from_utf8(self.read_bytes()?).map_err(|_| CanonicalDecodeError::InvalidUtf8)
    }

    pub fn read_option<T>(
        &mut self,
        decode_some: impl FnOnce(&mut Self) -> Result<T, CanonicalDecodeError>,
    ) -> Result<Option<T>, CanonicalDecodeError> {
        match self.read_u8()? {
            0x00 => Ok(None),
            0x01 => Ok(Some(decode_some(self)?)),
            tag => Err(CanonicalDecodeError::UnknownTag {
                tag,
                type_name: "Option presence tag",
            }),
        }
    }

    pub fn read_seq<T>(
        &mut self,
        mut decode_item: impl FnMut(&mut Self) -> Result<T, CanonicalDecodeError>,
    ) -> Result<Vec<T>, CanonicalDecodeError> {
        let len = self.read_u32()? as usize;
        let mut out = Vec::with_capacity(len.min(1 << 20));
        for _ in 0..len {
            out.push(decode_item(self)?);
        }
        Ok(out)
    }
}

/// A type with a canonical, round-trippable binary encoding.
pub trait CanonicalEncode: Sized {
    fn canonical_encode(&self, w: &mut CanonicalWriter);
    fn canonical_decode(r: &mut CanonicalReader) -> Result<Self, CanonicalDecodeError>;

    /// Convenience: encode to a standalone byte vector.
    fn to_canonical_bytes(&self) -> Vec<u8> {
        let mut w = CanonicalWriter::new();
        self.canonical_encode(&mut w);
        w.into_bytes()
    }

    /// Convenience: decode from a standalone byte slice, requiring the
    /// whole slice to be consumed (no trailing garbage).
    fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, CanonicalDecodeError> {
        let mut r = CanonicalReader::new(bytes);
        let value = Self::canonical_decode(&mut r)?;
        r.finish()?;
        Ok(value)
    }
}

impl CanonicalEncode for uuid::Uuid {
    fn canonical_encode(&self, w: &mut CanonicalWriter) {
        w.write_bytes_fixed(self.as_bytes());
    }
    fn canonical_decode(r: &mut CanonicalReader) -> Result<Self, CanonicalDecodeError> {
        Ok(uuid::Uuid::from_bytes(r.read_bytes_fixed::<16>()?))
    }
}

impl CanonicalEncode for crate::types::Addr {
    fn canonical_encode(&self, w: &mut CanonicalWriter) {
        w.write_u32(self.get());
    }
    fn canonical_decode(r: &mut CanonicalReader) -> Result<Self, CanonicalDecodeError> {
        Ok(crate::types::Addr::new(r.read_u32()?))
    }
}

impl CanonicalEncode for crate::concurrency::RecordId {
    fn canonical_encode(&self, w: &mut CanonicalWriter) {
        self.as_uuid().canonical_encode(w);
    }
    fn canonical_decode(r: &mut CanonicalReader) -> Result<Self, CanonicalDecodeError> {
        Ok(crate::concurrency::RecordId::new(uuid::Uuid::canonical_decode(r)?))
    }
}

impl CanonicalEncode for crate::EffectId {
    fn canonical_encode(&self, w: &mut CanonicalWriter) {
        (*self).as_uuid().canonical_encode(w);
    }
    fn canonical_decode(r: &mut CanonicalReader) -> Result<Self, CanonicalDecodeError> {
        Ok(crate::EffectId::from_uuid(uuid::Uuid::canonical_decode(r)?))
    }
}

/// Tags: `0x00` Guard, `0x01` Race, `0x02` Barrier, `0x03` Compensation.
impl CanonicalEncode for crate::concurrency::RecordKind {
    fn canonical_encode(&self, w: &mut CanonicalWriter) {
        match self {
            Self::Guard { interrupting } => {
                w.write_u8(0x00);
                w.write_bool(*interrupting);
            }
            Self::Race => w.write_u8(0x01),
            Self::Barrier => w.write_u8(0x02),
            Self::Compensation => w.write_u8(0x03),
        }
    }
    fn canonical_decode(r: &mut CanonicalReader) -> Result<Self, CanonicalDecodeError> {
        Ok(match r.read_u8()? {
            0x00 => Self::Guard {
                interrupting: r.read_bool()?,
            },
            0x01 => Self::Race,
            0x02 => Self::Barrier,
            0x03 => Self::Compensation,
            tag => {
                return Err(CanonicalDecodeError::UnknownTag {
                    tag,
                    type_name: "RecordKind",
                })
            }
        })
    }
}

/// Tags: `0x00` Armed, `0x01` Retired.
impl CanonicalEncode for crate::concurrency::RecordState {
    fn canonical_encode(&self, w: &mut CanonicalWriter) {
        w.write_u8(match self {
            Self::Armed => 0x00,
            Self::Retired => 0x01,
        });
    }
    fn canonical_decode(r: &mut CanonicalReader) -> Result<Self, CanonicalDecodeError> {
        Ok(match r.read_u8()? {
            0x00 => Self::Armed,
            0x01 => Self::Retired,
            tag => {
                return Err(CanonicalDecodeError::UnknownTag {
                    tag,
                    type_name: "RecordState",
                })
            }
        })
    }
}

impl CanonicalEncode for crate::concurrency::RecordCounters {
    fn canonical_encode(&self, w: &mut CanonicalWriter) {
        w.write_u32(self.arity);
        w.write_u32(self.count);
    }
    fn canonical_decode(r: &mut CanonicalReader) -> Result<Self, CanonicalDecodeError> {
        Ok(Self {
            arity: r.read_u32()?,
            count: r.read_u32()?,
        })
    }
}

impl CanonicalEncode for crate::concurrency::ConcurrencyRecord {
    fn canonical_encode(&self, w: &mut CanonicalWriter) {
        self.id.canonical_encode(w);
        self.kind.canonical_encode(w);
        w.write_seq(self.members.iter(), |w, m| m.canonical_encode(w));
        w.write_option(&self.handler, |w, a| a.canonical_encode(w));
        self.state.canonical_encode(w);
        self.counters.canonical_encode(w);
    }
    fn canonical_decode(r: &mut CanonicalReader) -> Result<Self, CanonicalDecodeError> {
        let id = crate::concurrency::RecordId::canonical_decode(r)?;
        let kind = crate::concurrency::RecordKind::canonical_decode(r)?;
        let members: BTreeSet<uuid::Uuid> = r
            .read_seq(uuid::Uuid::canonical_decode)?
            .into_iter()
            .collect();
        let handler = r.read_option(crate::types::Addr::canonical_decode)?;
        let state = crate::concurrency::RecordState::canonical_decode(r)?;
        let counters = crate::concurrency::RecordCounters::canonical_decode(r)?;
        Ok(Self {
            id,
            kind,
            members,
            handler,
            state,
            counters,
        })
    }
}

impl CanonicalEncode for crate::concurrency::ConcurrencyTable {
    fn canonical_encode(&self, w: &mut CanonicalWriter) {
        // `iter()` walks the underlying BTreeMap in RecordId (Uuid) order —
        // canonical by construction, no explicit sort needed.
        w.write_seq(self.iter(), |w, (_, record)| record.canonical_encode(w));
    }
    fn canonical_decode(r: &mut CanonicalReader) -> Result<Self, CanonicalDecodeError> {
        let records = r.read_seq(crate::concurrency::ConcurrencyRecord::canonical_decode)?;
        let mut table = Self::new();
        for record in records {
            table.insert(record);
        }
        Ok(table)
    }
}

/// The pending-effect set (D3 Ring 2 hash domain component): the identity
/// set of effects a snapshot's fibres are currently waiting on. Ordered by
/// `BTreeSet` iteration — canonical by construction.
impl CanonicalEncode for BTreeSet<crate::EffectId> {
    fn canonical_encode(&self, w: &mut CanonicalWriter) {
        w.write_seq(self.iter(), |w, id| id.canonical_encode(w));
    }
    fn canonical_decode(r: &mut CanonicalReader) -> Result<Self, CanonicalDecodeError> {
        Ok(r.read_seq(crate::EffectId::canonical_decode)?.into_iter().collect())
    }
}

/// Tags: `0x00` Bool, `0x01` I64, `0x02` Str(interned id), `0x03` Ref(handle).
impl CanonicalEncode for crate::types::Value {
    fn canonical_encode(&self, w: &mut CanonicalWriter) {
        match self {
            Self::Bool(b) => {
                w.write_u8(0x00);
                w.write_bool(*b);
            }
            Self::I64(v) => {
                w.write_u8(0x01);
                w.write_u64(*v as u64);
            }
            Self::Str(id) => {
                w.write_u8(0x02);
                w.write_u32(*id);
            }
            Self::Ref(id) => {
                w.write_u8(0x03);
                w.write_u32(*id);
            }
        }
    }
    fn canonical_decode(r: &mut CanonicalReader) -> Result<Self, CanonicalDecodeError> {
        Ok(match r.read_u8()? {
            0x00 => Self::Bool(r.read_bool()?),
            0x01 => Self::I64(r.read_u64()? as i64),
            0x02 => Self::Str(r.read_u32()?),
            0x03 => Self::Ref(r.read_u32()?),
            tag => {
                return Err(CanonicalDecodeError::UnknownTag {
                    tag,
                    type_name: "Value",
                })
            }
        })
    }
}

/// Tags: `0x00` Running, `0x01` Timer, `0x02` Msg, `0x03` Job, `0x04` Effect,
/// `0x05` Join, `0x06` Race, `0x07` Incident.
impl CanonicalEncode for crate::types::WaitState {
    fn canonical_encode(&self, w: &mut CanonicalWriter) {
        match self {
            Self::Running => w.write_u8(0x00),
            Self::Timer { deadline_ms } => {
                w.write_u8(0x01);
                w.write_u64(*deadline_ms);
            }
            Self::Msg {
                wait_id,
                name,
                corr_key,
            } => {
                w.write_u8(0x02);
                w.write_u32(*wait_id);
                w.write_u32(*name);
                corr_key.canonical_encode(w);
            }
            Self::Job { job_key } => {
                w.write_u8(0x03);
                w.write_str(job_key);
            }
            Self::Effect { effect_id } => {
                w.write_u8(0x04);
                effect_id.canonical_encode(w);
            }
            Self::Join { join_id } => {
                w.write_u8(0x05);
                w.write_u32(*join_id);
            }
            Self::Race {
                race_id,
                timer_deadline_ms,
                job_key,
                interrupting,
                timer_arm_index,
                cycle_remaining,
                cycle_fired_count,
            } => {
                w.write_u8(0x06);
                w.write_u32(*race_id);
                w.write_option(timer_deadline_ms, |w, v| w.write_u64(*v));
                w.write_option(job_key, |w, v| w.write_str(v));
                w.write_bool(*interrupting);
                w.write_option(timer_arm_index, |w, v| w.write_u64(*v as u64));
                w.write_option(cycle_remaining, |w, v| w.write_u32(*v));
                w.write_u32(*cycle_fired_count);
            }
            Self::Incident { incident_id } => {
                w.write_u8(0x07);
                incident_id.canonical_encode(w);
            }
        }
    }
    fn canonical_decode(r: &mut CanonicalReader) -> Result<Self, CanonicalDecodeError> {
        Ok(match r.read_u8()? {
            0x00 => Self::Running,
            0x01 => Self::Timer {
                deadline_ms: r.read_u64()?,
            },
            0x02 => Self::Msg {
                wait_id: r.read_u32()?,
                name: r.read_u32()?,
                corr_key: crate::types::Value::canonical_decode(r)?,
            },
            0x03 => Self::Job {
                job_key: r.read_str()?,
            },
            0x04 => Self::Effect {
                effect_id: crate::EffectId::canonical_decode(r)?,
            },
            0x05 => Self::Join {
                join_id: r.read_u32()?,
            },
            0x06 => Self::Race {
                race_id: r.read_u32()?,
                timer_deadline_ms: r.read_option(|r| r.read_u64())?,
                job_key: r.read_option(|r| r.read_str())?,
                interrupting: r.read_bool()?,
                timer_arm_index: r.read_option(|r| Ok(r.read_u64()? as usize))?,
                cycle_remaining: r.read_option(|r| r.read_u32())?,
                cycle_fired_count: r.read_u32()?,
            },
            0x07 => Self::Incident {
                incident_id: uuid::Uuid::canonical_decode(r)?,
            },
            tag => {
                return Err(CanonicalDecodeError::UnknownTag {
                    tag,
                    type_name: "WaitState",
                })
            }
        })
    }
}

/// Tags: `0x00` Running, `0x01` Completed, `0x02` Cancelled, `0x03`
/// Terminated, `0x04` Failed, `0x05` WaitingOnSubmission, `0x06`
/// WaitingOnInvocation.
impl CanonicalEncode for crate::types::ProcessState {
    fn canonical_encode(&self, w: &mut CanonicalWriter) {
        match self {
            Self::Running => w.write_u8(0x00),
            Self::Completed { at } => {
                w.write_u8(0x01);
                w.write_u64(*at as u64);
            }
            Self::Cancelled { reason, at } => {
                w.write_u8(0x02);
                w.write_str(reason);
                w.write_u64(*at as u64);
            }
            Self::Terminated { at } => {
                w.write_u8(0x03);
                w.write_u64(*at as u64);
            }
            Self::Failed { incident_id } => {
                w.write_u8(0x04);
                incident_id.canonical_encode(w);
            }
            Self::WaitingOnSubmission {
                callout_id,
                node_id,
            } => {
                w.write_u8(0x05);
                callout_id.canonical_encode(w);
                w.write_str(node_id);
            }
            Self::WaitingOnInvocation {
                execution_id,
                node_id,
            } => {
                w.write_u8(0x06);
                execution_id.canonical_encode(w);
                w.write_str(node_id);
            }
        }
    }
    fn canonical_decode(r: &mut CanonicalReader) -> Result<Self, CanonicalDecodeError> {
        Ok(match r.read_u8()? {
            0x00 => Self::Running,
            0x01 => Self::Completed {
                at: r.read_u64()? as i64,
            },
            0x02 => Self::Cancelled {
                reason: r.read_str()?,
                at: r.read_u64()? as i64,
            },
            0x03 => Self::Terminated {
                at: r.read_u64()? as i64,
            },
            0x04 => Self::Failed {
                incident_id: uuid::Uuid::canonical_decode(r)?,
            },
            0x05 => Self::WaitingOnSubmission {
                callout_id: uuid::Uuid::canonical_decode(r)?,
                node_id: r.read_str()?,
            },
            0x06 => Self::WaitingOnInvocation {
                execution_id: uuid::Uuid::canonical_decode(r)?,
                node_id: r.read_str()?,
            },
            tag => {
                return Err(CanonicalDecodeError::UnknownTag {
                    tag,
                    type_name: "ProcessState",
                })
            }
        })
    }
}

/// Tags: `0x00` Transient, `0x01` ContractViolation, `0x02` BusinessRejection.
impl CanonicalEncode for crate::types::ErrorClass {
    fn canonical_encode(&self, w: &mut CanonicalWriter) {
        match self {
            Self::Transient => w.write_u8(0x00),
            Self::ContractViolation => w.write_u8(0x01),
            Self::BusinessRejection { rejection_code } => {
                w.write_u8(0x02);
                w.write_str(rejection_code);
            }
        }
    }
    fn canonical_decode(r: &mut CanonicalReader) -> Result<Self, CanonicalDecodeError> {
        Ok(match r.read_u8()? {
            0x00 => Self::Transient,
            0x01 => Self::ContractViolation,
            0x02 => Self::BusinessRejection {
                rejection_code: r.read_str()?,
            },
            tag => {
                return Err(CanonicalDecodeError::UnknownTag {
                    tag,
                    type_name: "ErrorClass",
                })
            }
        })
    }
}

impl CanonicalEncode for crate::types::Incident {
    fn canonical_encode(&self, w: &mut CanonicalWriter) {
        self.incident_id.canonical_encode(w);
        self.process_instance_id.canonical_encode(w);
        self.fiber_id.canonical_encode(w);
        w.write_str(&self.service_task_id);
        self.bytecode_addr.canonical_encode(w);
        self.error_class.canonical_encode(w);
        w.write_str(&self.message);
        w.write_u32(self.retry_count);
        w.write_u64(self.created_at as u64);
        w.write_option(&self.resolved_at, |w, v| w.write_u64(*v as u64));
        w.write_option(&self.resolution, |w, v| w.write_str(v));
    }
    fn canonical_decode(r: &mut CanonicalReader) -> Result<Self, CanonicalDecodeError> {
        Ok(Self {
            incident_id: uuid::Uuid::canonical_decode(r)?,
            process_instance_id: uuid::Uuid::canonical_decode(r)?,
            fiber_id: uuid::Uuid::canonical_decode(r)?,
            service_task_id: r.read_str()?,
            bytecode_addr: crate::types::Addr::canonical_decode(r)?,
            error_class: crate::types::ErrorClass::canonical_decode(r)?,
            message: r.read_str()?,
            retry_count: r.read_u32()?,
            created_at: r.read_u64()? as i64,
            resolved_at: r.read_option(|r| Ok(r.read_u64()? as i64))?,
            resolution: r.read_option(|r| r.read_str())?,
        })
    }
}

impl CanonicalEncode for crate::types::Fiber {
    fn canonical_encode(&self, w: &mut CanonicalWriter) {
        self.fiber_id.canonical_encode(w);
        self.pc.canonical_encode(w);
        w.write_seq(self.stack.iter(), |w, v| v.canonical_encode(w));
        for reg in &self.regs {
            reg.canonical_encode(w);
        }
        self.wait.canonical_encode(w);
        w.write_u32(self.loop_epoch);
        w.write_seq(self.control_stack.iter(), |w, h| h.canonical_encode(w));
    }
    fn canonical_decode(r: &mut CanonicalReader) -> Result<Self, CanonicalDecodeError> {
        let fiber_id = uuid::Uuid::canonical_decode(r)?;
        let pc = crate::types::Addr::canonical_decode(r)?;
        let stack = r.read_seq(crate::types::Value::canonical_decode)?;
        let mut regs_vec = Vec::with_capacity(8);
        for _ in 0..8 {
            regs_vec.push(crate::types::Value::canonical_decode(r)?);
        }
        // `regs_vec` always has exactly 8 elements by construction (the loop
        // above pushes exactly 8 times, or returns early via `?` on decode
        // failure) — `try_into` cannot actually fail; the error arm exists
        // only because the conversion is fallible in its type signature, not
        // because this path is reachable.
        let regs: [crate::types::Value; 8] = match regs_vec.try_into() {
            Ok(regs) => regs,
            Err(_) => return Err(CanonicalDecodeError::UnexpectedEof { offset: 0 }),
        };
        let wait = crate::types::WaitState::canonical_decode(r)?;
        let loop_epoch = r.read_u32()?;
        let control_stack = r.read_seq(crate::concurrency::RecordId::canonical_decode)?;
        Ok(Self {
            fiber_id,
            pc,
            stack,
            regs,
            wait,
            loop_epoch,
            control_stack,
        })
    }
}

/// Tags 0x00-0x0A in declaration order.
impl CanonicalEncode for crate::session_stack::SessionWorkspaceKind {
    fn canonical_encode(&self, w: &mut CanonicalWriter) {
        w.write_u8(match self {
            Self::ProductMaintenance => 0x00,
            Self::Catalogue => 0x01,
            Self::Deal => 0x02,
            Self::Cbu => 0x03,
            Self::Kyc => 0x04,
            Self::InstrumentMatrix => 0x05,
            Self::OnBoarding => 0x06,
            Self::SemOsMaintenance => 0x07,
            Self::LifecycleResources => 0x08,
            Self::BookingPrincipal => 0x09,
            Self::Bpmn => 0x0A,
        });
    }
    fn canonical_decode(r: &mut CanonicalReader) -> Result<Self, CanonicalDecodeError> {
        Ok(match r.read_u8()? {
            0x00 => Self::ProductMaintenance,
            0x01 => Self::Catalogue,
            0x02 => Self::Deal,
            0x03 => Self::Cbu,
            0x04 => Self::Kyc,
            0x05 => Self::InstrumentMatrix,
            0x06 => Self::OnBoarding,
            0x07 => Self::SemOsMaintenance,
            0x08 => Self::LifecycleResources,
            0x09 => Self::BookingPrincipal,
            0x0A => Self::Bpmn,
            tag => {
                return Err(CanonicalDecodeError::UnknownTag {
                    tag,
                    type_name: "SessionWorkspaceKind",
                })
            }
        })
    }
}

impl CanonicalEncode for crate::session_stack::SessionScopeState {
    fn canonical_encode(&self, w: &mut CanonicalWriter) {
        self.client_group_id.canonical_encode(w);
        w.write_option(&self.client_group_name, |w, v| w.write_str(v));
    }
    fn canonical_decode(r: &mut CanonicalReader) -> Result<Self, CanonicalDecodeError> {
        Ok(Self {
            client_group_id: uuid::Uuid::canonical_decode(r)?,
            client_group_name: r.read_option(|r| r.read_str())?,
        })
    }
}

/// V2.1d: fallible canonicalization for opaque, externally-supplied JSON
/// (`ProcessInstance::domain_payload`/`placeholder_values`,
/// `SessionStackState::workspace_stack`). Unlike every other type in this
/// module, this is **encode-only** — like `ffi-types/src/canonical.rs`'s
/// original one-way hash, nothing in the system ever reconstructs a live
/// value from this encoding; it exists solely to contribute deterministic
/// bytes to the Ring 2 hash domain (`docs/todo/EOP-VS-BPMN-ISA-002.md` §6).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CanonicalJsonError {
    #[error("canonical encoding rejects a non-finite float (NaN or Infinity) in a JSON payload")]
    NonFiniteFloat,
    #[error("canonical encoding: payload is not valid JSON: {0}")]
    InvalidJson(String),
}

/// Recursively rejects NaN/Infinity. JSON's grammar forbids literal NaN or
/// Infinity tokens; the originally-suspected bypass — a huge exponent like
/// `1e400` silently becoming `f64::INFINITY` — does **not** reach this
/// function in practice: `serde_json` (without `arbitrary_precision`)
/// bounds-checks exponents at parse time and rejects overflow as a parse
/// error, and `Number::from_f64` independently rejects NaN/Infinity, so a
/// `serde_json::Value` cannot hold a non-finite float through any safe
/// public API (verified: `domain_payload_with_huge_exponent_is_a_typed_parse_error_not_silent_infinity`,
/// `placeholder_values_cannot_carry_a_non_finite_float_via_serde_json_public_api`
/// below). This check is retained as defense-in-depth — a future JSON
/// source that isn't `serde_json`-parsed (or a `serde_json` upgrade
/// enabling `arbitrary_precision`) could reintroduce the bypass — not
/// because the current call sites can trigger it. Must run before
/// `encode_canonical_json`; that function assumes pre-validated input and
/// does not re-check.
pub fn validate_finite_json(value: &serde_json::Value) -> Result<(), CanonicalJsonError> {
    match value {
        serde_json::Value::Number(n) => {
            if n.as_i64().is_some() || n.as_u64().is_some() {
                return Ok(());
            }
            match n.as_f64() {
                Some(f) if f.is_finite() => Ok(()),
                _ => Err(CanonicalJsonError::NonFiniteFloat),
            }
        }
        serde_json::Value::Array(items) => items.iter().try_for_each(validate_finite_json),
        serde_json::Value::Object(map) => map.values().try_for_each(validate_finite_json),
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::String(_) => {
            Ok(())
        }
    }
}

/// Tags: `0x00` Null, `0x01` Bool, `0x02` Int(i64), `0x03` UInt(u64 beyond
/// `i64::MAX`), `0x04` Float(f64 bit pattern — deterministic regardless of
/// source text form; `1.0`/`1.00`/`1e0` converge), `0x05` String, `0x06`
/// Array, `0x07` Object. `serde_json::Map`'s default feature set has no
/// `preserve_order`, so iteration is already key-sorted — canonical by
/// construction, matching this module's `BTreeMap` convention elsewhere.
/// Requires `validate_finite_json` to have already passed.
pub fn encode_canonical_json(w: &mut CanonicalWriter, value: &serde_json::Value) {
    match value {
        serde_json::Value::Null => w.write_u8(0x00),
        serde_json::Value::Bool(b) => {
            w.write_u8(0x01);
            w.write_bool(*b);
        }
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                w.write_u8(0x02);
                w.write_u64(i as u64);
            } else if let Some(u) = n.as_u64() {
                w.write_u8(0x03);
                w.write_u64(u);
            } else {
                w.write_u8(0x04);
                w.write_u64(n.as_f64().unwrap_or(0.0).to_bits());
            }
        }
        serde_json::Value::String(s) => {
            w.write_u8(0x05);
            w.write_str(s);
        }
        serde_json::Value::Array(items) => {
            w.write_u8(0x06);
            w.write_seq(items.iter(), encode_canonical_json);
        }
        serde_json::Value::Object(map) => {
            w.write_u8(0x07);
            w.write_u32(map.len() as u32);
            for (k, v) in map {
                w.write_str(k);
                encode_canonical_json(w, v);
            }
        }
    }
}

/// Assumes `state.workspace_stack` frames already passed `validate_finite_json`.
fn encode_session_stack(w: &mut CanonicalWriter, state: &crate::session_stack::SessionStackState) {
    state.session_id.canonical_encode(w);
    w.write_option(&state.scope, |w, scope| scope.canonical_encode(w));
    w.write_option(&state.active_workspace, |w, kind| kind.canonical_encode(w));
    w.write_seq(state.workspace_stack.iter(), |w, frame| {
        encode_canonical_json(w, frame)
    });
    w.write_u64(state.trace_sequence);
}

impl crate::types::ProcessInstance {
    /// V2.1e: fallible canonical encoding for the Ring 2 hash domain. Not a
    /// `CanonicalEncode` impl — `domain_payload`/`placeholder_values` carry
    /// opaque externally-supplied JSON that can fail to parse or carry a
    /// non-finite float (V2.1d), so this composition must be allowed to
    /// reject, unlike every other impl in this module.
    ///
    /// `integrity_hash` is deliberately excluded: it is derived from this
    /// same state (A19) and is always `None` by the time this method runs
    /// (`PersistedSnapshotState::new` clears it), so including it would be
    /// either a no-op or, if that invariant ever broke, a state authenticating
    /// itself circularly.
    pub fn try_canonical_hash_bytes(&self) -> Result<Vec<u8>, CanonicalJsonError> {
        let domain_payload_value: serde_json::Value = serde_json::from_str(&self.domain_payload)
            .map_err(|error| CanonicalJsonError::InvalidJson(error.to_string()))?;
        validate_finite_json(&domain_payload_value)?;
        if let Some(placeholders) = &self.placeholder_values {
            validate_finite_json(placeholders)?;
        }
        for frame in &self.session_stack.workspace_stack {
            validate_finite_json(frame)?;
        }

        let mut w = CanonicalWriter::new();
        self.instance_id.canonical_encode(&mut w);
        w.write_str(&self.tenant_id);
        w.write_str(&self.process_key);
        w.write_bytes_fixed(&self.bytecode_version);
        encode_canonical_json(&mut w, &domain_payload_value);
        w.write_bytes_fixed(&self.domain_payload_hash);
        encode_session_stack(&mut w, &self.session_stack);
        w.write_seq(self.flags.iter(), |w, (k, v)| {
            w.write_u32(*k);
            v.canonical_encode(w);
        });
        w.write_seq(self.counters.iter(), |w, (k, v)| {
            w.write_u32(*k);
            w.write_u32(*v);
        });
        w.write_seq(self.join_expected.iter(), |w, (k, v)| {
            w.write_u32(*k);
            w.write_u16(*v);
        });
        self.state.canonical_encode(&mut w);
        w.write_str(&self.correlation_id);
        self.entry_id.canonical_encode(&mut w);
        self.runbook_id.canonical_encode(&mut w);
        w.write_u64(self.created_at as u64);
        w.write_option(&self.quarantine_state, |w, v| w.write_str(v));
        w.write_option(&self.plan_hash, |w, v| w.write_bytes_fixed(v));
        w.write_option(&self.current_node_id, |w, v| w.write_str(v));
        w.write_option(&self.placeholder_values, encode_canonical_json);
        Ok(w.into_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::concurrency::{ConcurrencyRecord, ConcurrencyTable, RecordId, RecordKind};
    use uuid::Uuid;

    fn sample_table() -> ConcurrencyTable {
        let mut table = ConcurrencyTable::new();
        let mut barrier = ConcurrencyRecord::new(RecordId::new(Uuid::from_u128(1)), RecordKind::Barrier);
        barrier.members.insert(Uuid::from_u128(100));
        barrier.members.insert(Uuid::from_u128(101));
        barrier.counters.arity = 2;
        table.insert(barrier);
        let mut guard = ConcurrencyRecord::new(
            RecordId::new(Uuid::from_u128(2)),
            RecordKind::Guard { interrupting: true },
        );
        guard.handler = Some(crate::types::Addr::new(42));
        table.insert(guard);
        table
    }

    #[test]
    fn concurrency_table_round_trips_byte_identically() {
        let table = sample_table();
        let bytes = table.to_canonical_bytes();
        let decoded = ConcurrencyTable::from_canonical_bytes(&bytes).unwrap();
        assert_eq!(decoded.to_canonical_bytes(), bytes);
    }

    #[test]
    fn empty_table_is_stable() {
        let table = ConcurrencyTable::new();
        let bytes = table.to_canonical_bytes();
        assert_eq!(bytes, 0u32.to_le_bytes().to_vec());
    }

    #[test]
    fn truncated_bytes_are_a_typed_decode_error_not_a_panic() {
        let table = sample_table();
        let mut bytes = table.to_canonical_bytes();
        bytes.truncate(bytes.len() - 3);
        assert!(matches!(
            ConcurrencyTable::from_canonical_bytes(&bytes),
            Err(CanonicalDecodeError::UnexpectedEof { .. })
        ));
    }

    #[test]
    fn unknown_record_kind_tag_is_a_typed_decode_error() {
        let table = sample_table();
        let mut bytes = table.to_canonical_bytes();
        // First record's kind tag sits right after: count(u32) + id(16 bytes).
        let tag_offset = 4 + 16;
        assert!(bytes[tag_offset] <= 0x03, "test assumption: tag byte located correctly");
        bytes[tag_offset] = 0xFF;
        assert!(matches!(
            ConcurrencyTable::from_canonical_bytes(&bytes),
            Err(CanonicalDecodeError::UnknownTag {
                tag: 0xFF,
                type_name: "RecordKind"
            })
        ));
    }

    /// V2.1 gate: golden-bytes corpus. This fixture's exact bytes are
    /// committed here; any change to the canonical encoding (field order,
    /// tag values, primitive widths) changes this assertion — a diff a
    /// reviewer must see and ratify, not a silent drift.
    #[test]
    fn golden_bytes_concurrency_table() {
        let table = sample_table();
        let bytes = table.to_canonical_bytes();
        let golden: Vec<u8> = vec![
            // record count = 2
            0x02, 0x00, 0x00, 0x00,
            // -- record 1: id = Uuid::from_u128(1) --
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
            // kind = Barrier (0x02)
            0x02,
            // members: count = 2, then two UUIDs sorted ascending
            0x02, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x64,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x65,
            // handler: None
            0x00,
            // state: Armed (0x00)
            0x00,
            // counters: arity=2, count=0
            0x02, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00,
            // -- record 2: id = Uuid::from_u128(2) --
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02,
            // kind = Guard { interrupting: true } (0x00, 0x01)
            0x00, 0x01,
            // members: count = 0
            0x00, 0x00, 0x00, 0x00,
            // handler: Some(Addr(42))
            0x01, 0x2a, 0x00, 0x00, 0x00,
            // state: Armed
            0x00,
            // counters: arity=0, count=0
            0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00,
        ];
        assert_eq!(bytes, golden, "canonical encoding drifted from the committed golden bytes — this is exactly the R1 risk V2.1 exists to catch; any change here must be a reviewed, deliberate encoding change, not an incidental one");
    }

    /// V2.1a gate: golden-bytes fixture per `Value` variant.
    #[test]
    fn golden_bytes_value_variants() {
        assert_eq!(crate::types::Value::Bool(true).to_canonical_bytes(), vec![0x00, 0x01]);
        assert_eq!(crate::types::Value::Bool(false).to_canonical_bytes(), vec![0x00, 0x00]);
        assert_eq!(
            crate::types::Value::I64(-1).to_canonical_bytes(),
            vec![0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]
        );
        assert_eq!(
            crate::types::Value::Str(0x2A).to_canonical_bytes(),
            vec![0x02, 0x2A, 0x00, 0x00, 0x00]
        );
        assert_eq!(
            crate::types::Value::Ref(0x2A).to_canonical_bytes(),
            vec![0x03, 0x2A, 0x00, 0x00, 0x00]
        );
    }

    #[test]
    fn value_round_trips_byte_identically() {
        for value in [
            crate::types::Value::Bool(true),
            crate::types::Value::Bool(false),
            crate::types::Value::I64(i64::MIN),
            crate::types::Value::I64(i64::MAX),
            crate::types::Value::Str(7),
            crate::types::Value::Ref(7),
        ] {
            let bytes = value.to_canonical_bytes();
            let decoded = crate::types::Value::from_canonical_bytes(&bytes).unwrap();
            assert_eq!(decoded.to_canonical_bytes(), bytes);
        }
    }

    /// V2.1d gate, corrected: the plan's premise ("a huge exponent like
    /// `1e400` parses to `f64::INFINITY` and is a real reachable input") is
    /// **false** for this codebase's JSON parser — `serde_json` (without
    /// `arbitrary_precision`) bounds-checks exponents at parse time and
    /// rejects overflow as a parse error, never producing a non-finite
    /// `f64::Number`. `Number::from_f64` also rejects NaN/Infinity, so a
    /// `serde_json::Value` can never hold a non-finite float through any
    /// safe public API. `CanonicalJsonError::NonFiniteFloat` is therefore
    /// unreachable via `domain_payload`/`placeholder_values` today — this
    /// test proves the *actual* failure mode (a typed parse error, not
    /// silent normalization) and the module docs are corrected accordingly;
    /// `validate_finite_json`'s check is retained as defense-in-depth
    /// against a future non-`serde_json` JSON source, not because this path
    /// exercises it.
    #[test]
    fn domain_payload_with_huge_exponent_is_a_typed_parse_error_not_silent_infinity() {
        let mut instance = sample_process_instance();
        instance.domain_payload = std::sync::Arc::from(r#"{"amount":1e400}"#);
        assert!(matches!(
            instance.try_canonical_hash_bytes(),
            Err(CanonicalJsonError::InvalidJson(_))
        ));
    }

    #[test]
    fn domain_payload_with_ordinary_float_is_accepted_and_deterministic_across_text_forms() {
        let mut a = sample_process_instance();
        a.domain_payload = std::sync::Arc::from(r#"{"amount":1.0}"#);
        let mut b = sample_process_instance();
        b.domain_payload = std::sync::Arc::from(r#"{"amount":1.00}"#);
        let mut c = sample_process_instance();
        c.domain_payload = std::sync::Arc::from(r#"{"amount":1e0}"#);
        let bytes_a = a.try_canonical_hash_bytes().unwrap();
        let bytes_b = b.try_canonical_hash_bytes().unwrap();
        let bytes_c = c.try_canonical_hash_bytes().unwrap();
        assert_eq!(bytes_a, bytes_b);
        assert_eq!(bytes_a, bytes_c);
    }

    /// `Some(serde_json::from_str(...).unwrap())` would panic here for the
    /// same reason as the `domain_payload` case above — the parser rejects
    /// the overflowing exponent before a `Value` is ever constructed, so
    /// there is no way to build a `placeholder_values` fixture carrying a
    /// non-finite float through the public `serde_json` API. This test
    /// documents that absence rather than asserting a false premise.
    #[test]
    fn placeholder_values_cannot_carry_a_non_finite_float_via_serde_json_public_api() {
        assert!(serde_json::from_str::<serde_json::Value>(r#"{"score":1e400}"#).is_err());
        assert!(serde_json::Number::from_f64(f64::NAN).is_none());
        assert!(serde_json::Number::from_f64(f64::INFINITY).is_none());
    }

    #[test]
    fn malformed_domain_payload_json_is_a_typed_error_not_a_silent_default() {
        let mut instance = sample_process_instance();
        instance.domain_payload = std::sync::Arc::from("not json");
        assert!(matches!(
            instance.try_canonical_hash_bytes(),
            Err(CanonicalJsonError::InvalidJson(_))
        ));
    }

    fn sample_process_instance() -> crate::types::ProcessInstance {
        crate::types::ProcessInstance {
            instance_id: Uuid::from_u128(1),
            tenant_id: "tenant".to_string(),
            process_key: "process".to_string(),
            bytecode_version: [1; 32],
            domain_payload: std::sync::Arc::from("{}"),
            domain_payload_hash: [0; 32],
            session_stack: crate::session_stack::SessionStackState::default(),
            flags: Default::default(),
            counters: Default::default(),
            join_expected: Default::default(),
            state: crate::types::ProcessState::Running,
            correlation_id: "correlation".to_string(),
            entry_id: Uuid::nil(),
            runbook_id: Uuid::nil(),
            created_at: 1,
            integrity_hash: None,
            quarantine_state: None,
            plan_hash: None,
            current_node_id: None,
            placeholder_values: None,
        }
    }
}

#[cfg(test)]
mod proptest_round_trip {
    use super::*;
    use crate::concurrency::{ConcurrencyRecord, ConcurrencyTable, RecordCounters, RecordId, RecordKind, RecordState};
    use crate::types::Addr;
    use proptest::collection::{btree_set, vec as pvec};
    use proptest::prelude::*;
    use uuid::Uuid;

    fn arb_uuid() -> impl Strategy<Value = Uuid> {
        any::<u128>().prop_map(Uuid::from_u128)
    }

    fn arb_record_kind() -> impl Strategy<Value = RecordKind> {
        prop_oneof![
            any::<bool>().prop_map(|interrupting| RecordKind::Guard { interrupting }),
            Just(RecordKind::Race),
            Just(RecordKind::Barrier),
            Just(RecordKind::Compensation),
        ]
    }

    fn arb_record_state() -> impl Strategy<Value = RecordState> {
        prop_oneof![Just(RecordState::Armed), Just(RecordState::Retired)]
    }

    fn arb_record() -> impl Strategy<Value = ConcurrencyRecord> {
        (
            arb_uuid(),
            arb_record_kind(),
            btree_set(arb_uuid(), 0..4),
            proptest::option::of(any::<u32>()),
            arb_record_state(),
            any::<u32>(),
            any::<u32>(),
        )
            .prop_map(|(id, kind, members, handler, state, arity, count)| ConcurrencyRecord {
                id: RecordId::new(id),
                kind,
                members,
                handler: handler.map(Addr::new),
                state,
                counters: RecordCounters { arity, count },
            })
    }

    fn arb_table() -> impl Strategy<Value = ConcurrencyTable> {
        pvec(arb_record(), 0..6).prop_map(|records| {
            let mut table = ConcurrencyTable::new();
            for record in records {
                table.insert(record);
            }
            table
        })
    }

    proptest! {
        /// V2.1 gate: round-trip fixed-point law, `canonicalize(decode(b)) == b`,
        /// as a property test over arbitrary (fuzzed) concurrency tables —
        /// not just the one hand-picked golden fixture.
        #[test]
        fn canonical_round_trip_is_a_fixed_point(table in arb_table()) {
            let bytes = table.to_canonical_bytes();
            let decoded = ConcurrencyTable::from_canonical_bytes(&bytes)
                .expect("a freshly-encoded frame must always decode");
            prop_assert_eq!(decoded.to_canonical_bytes(), bytes);
        }

        /// Corruption-injection style property: any single flipped byte in a
        /// canonical frame either fails to decode or decodes to something
        /// that no longer round-trips to the same bytes — it never silently
        /// decodes to the *original* value. This is the general form of the
        /// V2.5 "flipped byte under hash" fixture.
        #[test]
        fn flipping_any_byte_never_reproduces_the_original_value(
            table in arb_table().prop_filter("need at least one record to have bytes to flip", |t| !t.is_empty()),
            flip_index in any::<usize>(),
            flip_bits in any::<u8>().prop_filter("must actually flip something", |b| *b != 0),
        ) {
            let bytes = table.to_canonical_bytes();
            let idx = flip_index % bytes.len();
            let mut corrupted = bytes.clone();
            corrupted[idx] ^= flip_bits;
            let still_same_value = ConcurrencyTable::from_canonical_bytes(&corrupted)
                .map(|decoded| decoded.to_canonical_bytes() == bytes)
                .unwrap_or(false);
            prop_assert!(!still_same_value, "a corrupted byte must never decode back to the original frame");
        }
    }
}
