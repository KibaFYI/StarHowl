fn main() {
    println!("cargo:rerun-if-changed=assets/starhowl.rc");
    println!("cargo:rerun-if-changed=assets/starhowl.ico");
    println!("cargo:rerun-if-changed=assets/starhowl.manifest");
    embed_resource::compile("assets/starhowl.rc", embed_resource::NONE);
}
