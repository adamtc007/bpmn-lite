fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .compile_protos(&["proto/bpmn_lite/v1/bpmn_lite.proto"], &["proto"])?;
    Ok(())
}
