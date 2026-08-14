fn main() {
    emit_libtorch_rpaths();
}

fn emit_libtorch_rpaths() {
    println!("cargo:rerun-if-env-changed=DEP_TCH_LIBTORCH_LIB");
    if let Ok(library_directory) = std::env::var("DEP_TCH_LIBTORCH_LIB") {
        println!("cargo:rustc-link-arg=-Wl,-rpath,{library_directory}");
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path/../Frameworks");
    } else {
        println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/../lib");
    }
}
