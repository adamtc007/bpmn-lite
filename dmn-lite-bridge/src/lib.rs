//! dmn-lite FFI execution owner (A10).
//!
//! Registers compiled dmn-lite decisions as FFI templates and dispatches
//! calls from bpmn-lite processes through the `FfiExecutionOwner` trait.
//!
//! ## Registration
//!
//! ```rust,ignore
//! let owner = Arc::new(DmnLiteOwner::new());
//! // Register a decision; receive the FfiTemplate to publish in the catalogue.
//! let template = owner.register_decision(
//!     verified_decision,
//!     input_schema,   // Vec<ffi_types::FieldSchema>
//!     output_schema,
//!     Idempotency::Idempotent,
//!     "tenant-a".to_string(),
//!     "auth-service".to_string(),
//! );
//! // Publish `template` to FfiTemplateStore, then register `owner` with FfiDispatcher.
//! ```
//!
//! ## SemOsDomain field resolution
//!
//! For `SchemaKind::SemOsDomain` input fields, JSON string values (e.g.
//! `"LU"`) are passed to the optional `ValueResolver`. If no resolver is
//! set, or the resolver returns `None`, the field is treated as
//! `TypedValue::Str(symbol)` — which causes an `InputTypeMismatch` in
//! dmn-lite. Full Sem OS catalogue integration is A12+ scope.

#![forbid(unsafe_code)]

pub mod owner;
pub mod resolver;

pub use owner::DmnLiteOwner;
pub use resolver::ValueResolver;
