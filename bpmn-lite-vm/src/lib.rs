//! Compatibility utilities retained after the interpreter moved into
//! `bpmn-lite-kernel` in T7. This crate owns no workflow mutation path.

pub mod json_path;

pub fn compute_hash(data: &str) -> [u8; 32] {
    blake3::hash(data.as_bytes()).into()
}
