fn main() {
    println!("cargo:rerun-if-changed=proto/");
    println!("cargo:rerun-if-changed=src/");

    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .compile_protos(&["proto/shim.proto"], &["proto/"])
        .expect("Failed to compile proto files");
}
