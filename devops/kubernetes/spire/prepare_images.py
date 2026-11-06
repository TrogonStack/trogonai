import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import subprocess
import tarfile
import tempfile

from render import VERSION, COMMIT


TAG_OBJECT = "19ad0b43ce14493118d267ff142e87897a3166ee"
BASE = "gcr.io/distroless/static-debian12@sha256:afa5c872c891853ca7fcf1f12c3edb23f7eeef36189728842dd51042ff57f7ab"
BASE_IDENTITY = "keyless@distroless.iam.gserviceaccount.com"
ARCHIVES = {
    "amd64": "7bcf5e167151a1fc58293768462b44e155ba8784214582aefa6adf18c633d1d5",
    "arm64": "ba2804905a27ca72b0478ae790b580ea26233abafac7e480bbaf54152d5c2c5b",
}
RELEASE_SIGNER = r"^https://dotcom\.releases\.github\.com$"
RELEASE_CERTIFICATE_ISSUER = r"CN=Fulcio Intermediate l1,O=GitHub\, Inc."


def run(command):
    return subprocess.run(command, capture_output=True, text=True, timeout=180)


def checked(command, runner):
    result = runner(command)
    if result.returncode:
        raise ValueError("artifact verification failed")
    return result.stdout


def release_statement(data, architecture):
    try:
        verified = data["verificationResult"]
        if verified["verifiedIdentity"]["subjectAlternativeName"]["regexp"] != RELEASE_SIGNER:
            raise ValueError("artifact verification failed")
        certificate = verified["signature"]["certificate"]
        if certificate["subjectAlternativeName"] != "https://dotcom.releases.github.com" or certificate["certificateIssuer"] != RELEASE_CERTIFICATE_ISSUER:
            raise ValueError("artifact verification failed")
        statement = verified["statement"]
        predicate = statement["predicate"]
        if statement["predicateType"] != "https://in-toto.io/attestation/release/v0.2" or predicate["repository"] != "spiffe/spire" or predicate["tag"] != "v" + VERSION:
            raise ValueError("artifact verification failed")
        if predicate["repositoryId"] != "100061496" or predicate["ownerId"] != "19497462":
            raise ValueError("artifact verification failed")
        subjects = statement["subject"]
        if not any(subject.get("uri") == f"pkg:github/spiffe/spire@v{VERSION}" and subject["digest"].get("sha1") == TAG_OBJECT for subject in subjects):
            raise ValueError("artifact verification failed")
        archive_name = f"spire-{VERSION}-linux-{architecture}-musl.tar.gz"
        if not any(subject.get("name") == archive_name and subject["digest"].get("sha256") == ARCHIVES[architecture] for subject in subjects):
            raise ValueError("artifact verification failed")
        return statement
    except (KeyError, TypeError) as error:
        raise ValueError("artifact verification failed") from error


def base_verification(runner):
    command = ["cosign", "verify", "--certificate-oidc-issuer", "https://accounts.google.com",
               "--certificate-identity", BASE_IDENTITY, "--output", "json", BASE]
    signatures = json.loads(checked(command, runner))
    try:
        if not signatures or not all(signature["critical"]["image"]["docker-manifest-digest"] == BASE.split("@", 1)[1]
                                     for signature in signatures):
            raise ValueError("artifact verification failed")
    except (KeyError, TypeError) as error:
        raise ValueError("artifact verification failed") from error
    return signatures


def read_archive(archive, architecture):
    if archive.stat().st_size > 160 * 1024 * 1024:
        raise ValueError("release archive exceeds the profile bound")
    with archive.open("rb") as handle:
        digest = hashlib.file_digest(handle, "sha256").hexdigest()
    if digest != ARCHIVES[architecture]:
        raise ValueError("release archive does not match the authenticated pin")
    wanted = {f"spire-{VERSION}/bin/spire-{kind}": f"bin/spire-{kind}" for kind in ["server", "agent"]}
    wanted[f"spire-{VERSION}/LICENSE"] = "LICENSE"
    result, names = {}, set()
    with tarfile.open(archive) as source:
        for member in source:
            path = PurePosixPath(member.name)
            if len(names) >= 64 or member.name in names or path.is_absolute() or ".." in path.parts or not (member.isfile() or member.isdir()):
                raise ValueError("release archive layout is not usable")
            names.add(member.name)
            if member.size > 256 * 1024 * 1024:
                raise ValueError("release member exceeds the profile bound")
            if member.name in wanted:
                if not member.isfile():
                    raise ValueError("release binary is not a regular file")
                handle = source.extractfile(member)
                if handle is None:
                    raise ValueError("release binary is unavailable")
                with handle:
                    result[wanted[member.name]] = handle.read(256 * 1024 * 1024 + 1)
    if set(result) != set(wanted.values()):
        raise ValueError("release archive is missing a binary or license")
    machine = {"amd64": 62, "arm64": 183}[architecture]
    for kind in ["server", "agent"]:
        binary = result["bin/spire-" + kind]
        if len(binary) < 64 or binary[:6] != b"\x7fELF\x02\x01" or int.from_bytes(binary[18:20], "little") != machine:
            raise ValueError("release binary architecture does not match the selected platform")
    return result


def dockerfile(component, architecture):
    user = "1000:1000" if component == "server" else "0:0"
    return f'''FROM --platform=linux/{architecture} {BASE}
COPY --chmod=0555 bin/spire-{component} /opt/spire/bin/spire-{component}
COPY --chmod=0444 LICENSE /usr/share/licenses/spire/LICENSE
LABEL org.opencontainers.image.source="https://github.com/spiffe/spire" \\
      org.opencontainers.image.revision="{COMMIT}" \\
      org.opencontainers.image.version="{VERSION}"
USER {user}
WORKDIR /opt/spire
ENTRYPOINT ["/opt/spire/bin/spire-{component}", "run"]
'''


def prepare(architecture, output, scratch, runner=run):
    if architecture not in ARCHIVES or output.exists():
        raise ValueError("select a supported architecture and a new output directory")
    release_command = ["gh", "release", "verify", "v" + VERSION, "--repo", "spiffe/spire", "--format", "json"]
    release = json.loads(checked(release_command, runner))
    release_statement(release, architecture)
    tag = json.loads(checked(["gh", "api", "repos/spiffe/spire/git/tags/" + TAG_OBJECT], runner))
    try:
        if tag["object"]["type"] != "commit" or tag["object"]["sha"] != COMMIT or tag["tag"] != "v" + VERSION:
            raise ValueError("release tag does not name the pinned source commit")
    except (KeyError, TypeError) as error:
        raise ValueError("release tag is unavailable") from error
    base = base_verification(runner)
    with tempfile.TemporaryDirectory(prefix="spire-release-", dir=scratch) as directory:
        archive = Path(directory) / f"spire-{VERSION}-linux-{architecture}-musl.tar.gz"
        checked(["gh", "release", "download", "v" + VERSION, "--repo", "spiffe/spire", "--pattern", archive.name, "--dir", directory], runner)
        verified_asset = json.loads(checked(["gh", "release", "verify-asset", "v" + VERSION, str(archive), "--repo", "spiffe/spire", "--format", "json"], runner))
        release_statement(verified_asset, architecture)
        files = read_archive(archive, architecture)
    for component in ["server", "agent"]:
        files["Dockerfile." + component] = dockerfile(component, architecture).encode()
    proof = {"trust": "authenticated release publisher and pinned base; no downstream build provenance", "architecture": architecture,
             "upstream_release": release["verificationResult"], "upstream_asset": verified_asset["verificationResult"],
             "base_signatures": base, "base": BASE, "source_commit": COMMIT, "archive_sha256": ARCHIVES[architecture]}
    files["authenticated-inputs.json"] = (json.dumps(proof, indent=2, sort_keys=True) + "\n").encode()
    output.mkdir(mode=0o700)
    (output / "bin").mkdir(mode=0o700)
    for name, content in files.items():
        descriptor = os.open(output / name, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
        with os.fdopen(descriptor, "wb") as handle:
            handle.write(content)
        os.utime(output / name, (0, 0))


def main():
    parser = argparse.ArgumentParser(description="Prepare authenticated SPIRE image build inputs without building or publishing images.")
    parser.add_argument("--architecture", choices=ARCHIVES, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--scratch-root", type=Path, required=True)
    arguments = parser.parse_args()
    try:
        prepare(arguments.architecture, arguments.output, arguments.scratch_root)
    except (OSError, ValueError, subprocess.SubprocessError, tarfile.TarError):
        parser.exit(1, "build source refused: authenticated inputs or output are not usable\n")


if __name__ == "__main__":
    main()
