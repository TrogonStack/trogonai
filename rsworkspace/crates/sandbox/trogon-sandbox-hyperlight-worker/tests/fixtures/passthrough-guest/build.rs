// A small linear memory, as hyperlight-wasm's own samples use: the guest's Wasm memory comes out
// of the micro-VM heap (1.25 MiB in the worker), and the default 1 MiB stack would nearly fill it.
fn main() {
    println!("cargo:rustc-link-arg=-zstack-size=65536");
    println!("cargo:rustc-link-arg=--initial-memory=131072");
}
