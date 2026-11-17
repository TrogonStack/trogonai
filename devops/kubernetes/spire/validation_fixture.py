"""Build structural validation inputs without asserting artifact trust."""

import json
from pathlib import Path

from iam import policies
from manifests import manifests, supervisor_patches
from profile import agent_config, server_config, validate
from render import IMAGES


def deployment_fixture():
    config = json.loads(Path(__file__).with_name("deployment.example.json").read_text())
    validate(config)

    def encode(value):
        return json.dumps(value, indent=2, sort_keys=True) + "\n"

    files = {name: encode(value) for name, value in policies(config).items()}
    files.update({"server.conf": encode(server_config(config)), "agent.conf": encode(agent_config(config))})
    return {"resources": manifests(config, IMAGES, files), "patches": supervisor_patches(config)}
