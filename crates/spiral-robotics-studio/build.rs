fn main() {
    println!("cargo:rerun-if-env-changed=DEP_TCH_LIBTORCH_LIB");
    let Ok(library_directory) = std::env::var("DEP_TCH_LIBTORCH_LIB") else {
        return;
    };
    println!("cargo:rustc-link-arg=-Wl,-rpath,{library_directory}");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path/../Frameworks");
    } else {
        println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/../lib");
    }
}
