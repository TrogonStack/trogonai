use std::{env, fs, path::Path};

use sha2::{Digest, Sha256};

fn main() {
    let manifest = env::var("CARGO_MANIFEST_DIR").expect("Cargo provides the manifest directory");
    let root = Path::new(&manifest).join("../..");
    let bindings = [
        (
            "worker/consumer.rs",
            "79b467493e84088ddba03bc23ee19ff1198456b229a08d7ff180f64f5e379472",
        ),
        (
            "worker/processor.rs",
            "0b6e109a48bd0bd5ff86901e4864178463a668451e0e67434986011b747c0c29",
        ),
        (
            "worker/dispatcher.rs",
            "d83892a9da3f0918a0d5b27fc89dd7c7d721b9863eb488001ec762689d7369cb",
        ),
        (
            "execution_schedules/mod.rs",
            "fe948b1673c145ec36ad81f53c17301f42a232d2c1e386646d8218f1f7290290",
        ),
        (
            "reconciliation/reconcile.rs",
            "8fe21c29b6c9566e7c3e68be1daa5bf43e2e499cf324aa2797959f3f29409ca3",
        ),
        (
            "worker/processor/tests.rs",
            "6127644c811d9fb0ae9f276c9883eb3e3fac36afdafefa67a892de29fe007cab",
        ),
        (
            "checkpoints/store.rs",
            "c47af977f57be0f79f8aab9b5d2a98227475f1003d1ae9080cbb9f2c379ec874",
        ),
    ];
    for (relative, expected) in bindings {
        let source = root
            .join("rsworkspace/crates/scheduler/trogon-scheduler/src/processor/execution")
            .join(relative);
        println!("cargo:rerun-if-changed={}", source.display());
        let bytes = fs::read(&source).expect("bound scheduler source must be present");
        let digest = format!("{:x}", Sha256::digest(bytes));
        assert_eq!(
            digest, expected,
            "scheduler source changed: {relative}; review model correspondence before refreshing its binding"
        );
    }
    stageleft_tool::gen_final!();
}
