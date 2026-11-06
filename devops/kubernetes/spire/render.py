import json
import subprocess

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
        for image in IMAGES.values():
            verify(image)
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        raise ValueError("release verification failed") from error
    raise ValueError("deployment configuration missing")
