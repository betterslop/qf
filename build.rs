//! Compile syntect's bundled grammars plus the extra grammars in `assets/syntaxes`
//! into one compressed dump. `src/preview.rs` embeds the dump.

use std::env;
use std::path::PathBuf;

use syntect::dumps::dump_to_file;
use syntect::parsing::SyntaxSet;

fn main() {
    println!("cargo:rerun-if-changed=assets/syntaxes");
    let mut builder = SyntaxSet::load_defaults_newlines().into_builder();
    builder
        .add_from_folder("assets/syntaxes", true)
        .expect("failed to load assets/syntaxes");
    let set = builder.build();
    let out = PathBuf::from(env::var("OUT_DIR").expect("cargo sets OUT_DIR")).join("syntaxes.packdump");
    dump_to_file(&set, out).expect("failed to write syntax dump");
}
