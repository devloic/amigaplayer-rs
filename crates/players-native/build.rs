// Builds an empty registry until tools/native-players.py has written gen/, or
// when the `gen` feature is off (a build to distribute: gen/ is derived from the
// user's own player files).
fn main() {
    println!("cargo::rustc-check-cfg=cfg(has_gen)");
    println!("cargo::rerun-if-changed=gen/registry.rs");
    if std::env::var_os("CARGO_FEATURE_GEN").is_some() && std::path::Path::new("gen/registry.rs").exists() {
        println!("cargo::rustc-cfg=has_gen");
    }
}
