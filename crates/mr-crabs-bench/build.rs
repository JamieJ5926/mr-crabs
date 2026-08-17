use std::fs;
use std::path::Path;

fn main() {
    println!("cargo:rustc-check-cfg=cfg(has_storage)");
    let terminal = Path::new("../mr-crabs-terminal/src/lib.rs");
    let has_storage = terminal
        .exists()
        .then(|| fs::read_to_string(terminal).unwrap_or_default())
        .is_some_and(|text| text.contains("StorageStats") && text.contains("ScrollbackConfig"));
    if has_storage {
        println!("cargo:rustc-cfg=has_storage");
    }
    println!("cargo:rerun-if-changed=../mr-crabs-terminal/src/lib.rs");
    println!("cargo:rerun-if-changed=build.rs");
}
