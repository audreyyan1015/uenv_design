use prost::Message;

fn main() {
    let descriptor = "../contracts/generated/uenv.pb";
    println!("cargo:rerun-if-changed={descriptor}");
    let bytes = std::fs::read(descriptor).expect("Run scripts/build_contracts.py first");
    let files = prost_types::FileDescriptorSet::decode(bytes.as_slice()).unwrap();
    prost_build::Config::new().compile_fds(files).unwrap();
    println!("cargo:rerun-if-changed=../contracts/extensions");
    let mut paths = std::fs::read_dir("../contracts/extensions")
        .unwrap()
        .map(|p| p.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect::<Vec<_>>();
    paths.sort();
    let entries = paths
        .iter()
        .map(|p| format!("include_str!({:?})", p.canonicalize().unwrap()))
        .collect::<Vec<_>>()
        .join(",\n");
    std::fs::write(
        std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("schemas.rs"),
        format!("pub const BUILTIN_SCHEMAS: &[&str] = &[{entries}];"),
    )
    .unwrap();
}
