//! `trogon-aauth-person` binary: stub that prints the configuration the binary
//! would honor and exits. The library is the actual integration surface for v0;
//! production launches will be added once the operator CRD lands.

fn main() {
    eprintln!(
        "trogon-aauth-person v{} — library-only MVP. Embed PersonCore and call run()/router() directly.",
        env!("CARGO_PKG_VERSION")
    );
}
