fn main() {
    let _ = utterance_engine::context::ContextProjection {
        schema_version: 1,
        pack_identity: "unchecked".to_string(),
        graph_identity: "unchecked".to_string(),
        anchor: None,
        node_kind_counts: Vec::new(),
    };
}
