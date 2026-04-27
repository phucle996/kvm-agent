fn main() -> Result<(), Box<dyn std::error::Error>> {
    std::env::set_var("PROTOC", protoc_bin_vendored::protoc_bin_path()?);
    let protos = ["proto/agent/registry/v1/agent_registry.proto"];

    tonic_build::configure()
        .build_client(true)
        .build_server(true)
        .extern_path(".google.protobuf.Timestamp", "::prost_types::Timestamp")
        .compile_well_known_types(true)
        .compile_protos(&protos, &["proto"])?;

    Ok(())
}
