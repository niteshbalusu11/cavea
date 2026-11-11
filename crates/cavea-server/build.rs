//! The console's files (09 §9, CON-015). `CAVEA_CONSOLE_EMBED` names the
//! table `cargo xtask build-server` writes (`include_bytes!` of the built
//! console's files and their brotli forms); without it, as in a plain
//! `cargo build` or in CI, the table is empty and the server says the console
//! is not built in. Either way `$OUT_DIR/console.rs` defines `EMBEDDED`.

use std::{env, fs, path::PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo::rerun-if-env-changed=CAVEA_CONSOLE_EMBED");
    let table = match env::var_os("CAVEA_CONSOLE_EMBED") {
        Some(path) => {
            // The files the table includes are tracked by the compiler.
            println!("cargo::rerun-if-changed={}", PathBuf::from(&path).display());
            fs::read_to_string(&path)
                .map_err(|e| format!("CAVEA_CONSOLE_EMBED={}: {e}", path.display()))?
        }
        None => "&[]".to_owned(),
    };
    let out = PathBuf::from(env::var_os("OUT_DIR").ok_or("OUT_DIR is not set")?);
    fs::write(
        out.join("console.rs"),
        format!("static EMBEDDED: &[File] = {table};\n"),
    )?;
    Ok(())
}
