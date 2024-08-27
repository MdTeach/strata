fn main() {
    if cfg!(feature = "prover") && std::env::var("SKIP_GUEST_BUILD").is_err() {
        risc0_build::embed_methods();
    } else {
        // Return mock ELF
        let out_dir = std::env::var_os("OUT_DIR").unwrap();
        let out_dir = std::path::Path::new(&out_dir);
        let methods_path = out_dir.join("methods.rs");

        let elf = r#"
            pub const RETH_RISC0_ELF: &[u8] = &[];
        "#;

        std::fs::write(methods_path, elf).expect("Failed to write mock rollup elf");
    }
}