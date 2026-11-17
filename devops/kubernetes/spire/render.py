import json
import subprocess
import argparse
import os
from pathlib import Path

from iam import policies
from manifests import manifests, supervisor_patches
from profile import validate, server_config, agent_config, registrations

VERSION = "1.16.0"
COMMIT = "872c1d55b4d5ac7fc1bf05f0760f2ca740ab37aa"
ISSUER = "https://token.actions.githubusercontent.com"
IDENTITY = f"https://github.com/spiffe/spire/.github/workflows/release_build.yaml@refs/tags/v{VERSION}"
IMAGES = {
    "server": "ghcr.io/spiffe/spire-server@sha256:ff546f4c62dafc479a0cd79eae9a7b6df2975754d75efd31706098e0d7548837",
    "agent": "ghcr.io/spiffe/spire-agent@sha256:ab3a8ce70435374cc5d84c81eabf0278dc046ddbc0a736d59694ba9e5975b1b6",
}


def verify_image(image):
    command = [
        "cosign", "verify", "--certificate-oidc-issuer", ISSUER,
        "--certificate-identity", IDENTITY,
        "--certificate-github-workflow-sha", COMMIT,
        "--certificate-github-workflow-ref", f"refs/tags/v{VERSION}",
        "--certificate-github-workflow-repository", "spiffe/spire",
        "--output", "json", image,
    ]
    result = subprocess.run(command, capture_output=True, text=True, timeout=180)
    if result.returncode:
        raise ValueError("release verification failed")
    try:
        signatures = json.loads(result.stdout)
        digest = image.split("@", 1)[1]
        if not signatures or not all(
            signature["critical"]["image"]["docker-manifest-digest"] == digest
            for signature in signatures
        ):
            raise ValueError("release verification failed")
    except (ValueError, KeyError, TypeError) as error:
        raise ValueError("release verification failed") from error
    return signatures


def render(config, verify=verify_image):
    try:
        verified = {name: verify(image) for name, image in IMAGES.items()}
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        raise ValueError("release verification failed") from error
    validate(config)
    def encode(value):
        return json.dumps(value, indent=2, sort_keys=True) + "\n"
    files = {name: encode(value) for name, value in policies(config).items()}
    files.update({
        "server.conf": encode(server_config(config)), "agent.conf": encode(agent_config(config)),
        "registrations.json": encode(registrations(config)),
        "registration-plan.json": encode({"purpose": "review-only; not runtime authority", "recovery_epoch": config["recovery_epoch"], "registrations": config["registrations"]}),
        "supervisor-patches.json": encode(supervisor_patches(config)),
        "release-verification.json": encode({"version": VERSION, "commit": COMMIT, "images": IMAGES, "signatures": verified}),
    })
    files["manifests.json"] = encode(manifests(config, IMAGES, files))
    return files


def unique_fields(pairs):
    value = {}
    for key, item in pairs:
        if key in value:
            raise ValueError("duplicate deployment field")
        value[key] = item
    return value


def main():
    parser = argparse.ArgumentParser(description="Render the gated SPIRE deployment.")
    parser.add_argument("--config", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    try:
        with args.config.open("rb") as handle:
            source = handle.read(512 * 1024 + 1)
        if len(source) > 512 * 1024:
            raise ValueError("deployment configuration exceeds the profile bound")
        config = json.loads(source, object_pairs_hook=unique_fields)
        files = render(config)
        args.output.mkdir(mode=0o700)
        for name, content in files.items():
            descriptor = os.open(args.output / name, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
            with os.fdopen(descriptor, "w") as handle:
                handle.write(content)
    except (OSError, ValueError, subprocess.SubprocessError):
        parser.exit(1, "deployment refused: configuration, release trust or output is not usable\n")


if __name__ == "__main__":
    main()
