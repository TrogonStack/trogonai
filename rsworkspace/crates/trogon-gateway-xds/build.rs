use std::io;
use std::path::PathBuf;

fn protoc_include() -> PathBuf {
    std::env::var_os("PROTOC_INCLUDE")
        .map(PathBuf::from)
        .or_else(|| {
            std::process::Command::new("brew")
                .args(["--prefix", "protobuf"])
                .output()
                .ok()
                .filter(|output| output.status.success())
                .and_then(|output| {
                    String::from_utf8(output.stdout)
                        .ok()
                        .map(|prefix| PathBuf::from(prefix.trim()).join("include"))
                })
        })
        .unwrap_or_else(|| PathBuf::from("/usr/include"))
}

fn main() -> io::Result<()> {
    let proto_root = PathBuf::from("proto");
    let includes = [proto_root.clone(), protoc_include()];

    let protos = [
        "envoy/service/discovery/v3/ads.proto",
        "envoy/config/listener/v3/listener.proto",
        "envoy/config/route/v3/route.proto",
        "envoy/config/cluster/v3/cluster.proto",
        "envoy/config/endpoint/v3/endpoint.proto",
        "envoy/extensions/filters/http/rbac/v3/rbac.proto",
        "envoy/extensions/filters/http/ext_authz/v3/ext_authz.proto",
        "envoy/extensions/filters/network/http_connection_manager/v3/http_connection_manager.proto",
        "envoy/extensions/filters/http/router/v3/router.proto",
    ];

    for proto in protos {
        let path = proto_root.join(proto);
        println!("cargo:rerun-if-changed={}", path.display());
    }

    tonic_prost_build::configure()
        .build_server(true)
        .build_client(true)
        .compile_protos(
            &protos
                .iter()
                .map(|path| proto_root.join(path))
                .collect::<Vec<_>>(),
            &includes,
        )
}
