// Builds an empty registry until tools/native-players.py has written gen/.
fn main() {
    println!("cargo::rustc-check-cfg=cfg(has_gen)");
    println!("cargo::rerun-if-changed=gen/registry.rs");
    if std::path::Path::new("gen/registry.rs").exists() {
        println!("cargo::rustc-cfg=has_gen");
    }
}
