fn main() {
    // The UI is embedded into the binary at build time, and cargo does not
    // otherwise know that these files are inputs: editing them and rebuilding
    // produces a binary with the previous frontend still inside it.
    println!("cargo:rerun-if-changed=../ui");
    tauri_build::build()
}
