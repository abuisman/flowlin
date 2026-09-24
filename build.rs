use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=data/eu.ronkatil.Flowlin.gschema.xml");
    // Compile the schema next to the build output so `cargo run` works without
    // installing it. Installed builds find the system-wide compiled schema.
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("schemas");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::copy("data/eu.ronkatil.Flowlin.gschema.xml", out.join("eu.ronkatil.Flowlin.gschema.xml")).unwrap();
    match Command::new("glib-compile-schemas").arg(&out).status() {
        Ok(s) if s.success() => println!("cargo:rustc-env=FLOWLIN_SCHEMA_DIR={}", out.display()),
        _ => println!("cargo:warning=glib-compile-schemas failed; install the schema system-wide"),
    }
}
