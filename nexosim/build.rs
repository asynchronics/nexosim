fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(nexosim_server_codegen)]
    tonic_prost_build::configure()
        .build_client(false)
        .out_dir("src/server/codegen/")
        .compile_protos(&["simulation.proto"], &["src/server/api/"])?;

    Ok(())
}
