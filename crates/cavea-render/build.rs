//! Translates the WGSL shaders in `shaders/` to GLSL ES 3.00 with `naga`, so no
//! shader translator ships in the WASM (`docs/DECISIONS.md` D-05). A shader that
//! does not parse, validate or translate fails the build.
//!
//! Writes `$OUT_DIR/shaders.rs` with, per program, the vertex and fragment
//! sources and the names the GL backend binds resources by. GLSL ES 3.00 has no
//! `layout(binding)`, so uniform blocks and samplers are found by name.

use std::{env, fmt::Write as _, fs, path::PathBuf};

use naga::{
    ShaderStage,
    back::glsl::{self, PipelineOptions, Version, WriterFlags},
    proc::BoundsCheckPolicies,
    valid::{Capabilities, ValidationFlags, Validator},
};

/// Programs translated at build time; `text` and `image` come in later slices.
const PROGRAMS: &[&str] = &["shape", "mesh", "glyph"];

fn main() {
    if let Err(message) = run() {
        println!("cargo::error={message}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let out_dir = PathBuf::from(env::var("OUT_DIR").map_err(|e| e.to_string())?);
    let mut rust = String::new();
    for name in PROGRAMS {
        let path = format!("shaders/{name}.wgsl");
        println!("cargo::rerun-if-changed={path}");
        let source = fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
        let module = naga::front::wgsl::parse_str(&source)
            .map_err(|e| format!("{path}: {}", e.emit_to_string(&source)))?;
        let info = Validator::new(ValidationFlags::all(), Capabilities::empty())
            .validate(&module)
            .map_err(|e| format!("{path}: {}", e.emit_to_string(&source)))?;

        let upper = name.to_uppercase();
        for (stage, entry, suffix) in [
            (ShaderStage::Vertex, "vs_main", "VERT"),
            (ShaderStage::Fragment, "fs_main", "FRAG"),
        ] {
            let options = glsl::Options {
                version: Version::Embedded {
                    version: 300,
                    is_webgl: true,
                },
                writer_flags: WriterFlags::empty(),
                binding_map: Default::default(),
                zero_initialize_workgroup_memory: false,
            };
            let pipeline = PipelineOptions {
                shader_stage: stage,
                entry_point: entry.to_owned(),
                multiview: None,
            };
            let mut glsl_source = String::new();
            let reflection = glsl::Writer::new(
                &mut glsl_source,
                &module,
                &info,
                &options,
                &pipeline,
                BoundsCheckPolicies::default(),
            )
            .and_then(|mut writer| writer.write())
            .map_err(|e| format!("{path} ({entry}) to GLSL ES 3.00: {e}"))?;

            let file = format!("{name}.{}.glsl", suffix.to_lowercase());
            fs::write(out_dir.join(&file), &glsl_source).map_err(|e| e.to_string())?;
            writeln!(
                rust,
                "pub const {upper}_{suffix}: &str = include_str!(concat!(env!(\"OUT_DIR\"), \"/{file}\"));"
            )
            .map_err(|e| e.to_string())?;

            // Name every uniform block and sampler by the WGSL variable it came from.
            let mut names: Vec<(String, String)> = reflection
                .uniforms
                .iter()
                .map(|(handle, block)| (global_name(&module, *handle), block.clone()))
                .chain(reflection.texture_mapping.iter().map(|(sampler, mapping)| {
                    (global_name(&module, mapping.texture), sampler.clone())
                }))
                .collect();
            names.sort();
            for (global, glsl_name) in names {
                writeln!(
                    rust,
                    "pub const {upper}_{suffix}_{}: &str = {glsl_name:?};",
                    global.to_uppercase()
                )
                .map_err(|e| e.to_string())?;
            }
        }
    }
    fs::write(out_dir.join("shaders.rs"), rust).map_err(|e| e.to_string())
}

fn global_name(module: &naga::Module, handle: naga::Handle<naga::GlobalVariable>) -> String {
    module.global_variables[handle]
        .name
        .clone()
        .unwrap_or_default()
}
