"""Verify the saved `aws acm-pca describe-certificate-authority` output of the configured upstream authority.

The renderer cannot prove that the private CA root is hardware protected, so
the operator saves the describe output and this verifier refuses unless it
shows the CA the deployment input names, usable, with key storage at the level
the assurance level requires. Response shape (AWS ACM PCA API reference,
DescribeCertificateAuthority, as printed by the AWS CLI): {"CertificateAuthority":
{"Arn", "OwnerAccount", "Type", "Status", "CertificateAuthorityConfiguration":
{"KeyAlgorithm", "SigningAlgorithm", "Subject"}, "KeyStorageSecurityStandard",
"UsageMode", ...}}. The fixture in testdata/ follows the documentation; it is
not a capture from a live call.
"""

import argparse
import json
import sys
from pathlib import Path

from profile import validate

# Documented KeyStorageSecurityStandard values that are hardware level 3 or above; the CCPC value is the
# equivalent offered where FIPS 140-2 level 3 is not (AWS China regions).
HARDWARE_STANDARDS = {"FIPS_140_2_LEVEL_3_OR_HIGHER", "CCPC_LEVEL_1_OR_HIGHER"}
KNOWN_STANDARDS = HARDWARE_STANDARDS | {"FIPS_140_2_LEVEL_2_OR_HIGHER"}


class EvidenceRefused(ValueError):
    pass


def verify(config, described):
    """Return the verified facts, or raise EvidenceRefused naming the rule that failed."""
    validate(config)
    authority = config["upstream_authority"]
    if authority is None:
        raise EvidenceRefused("the deployment configures no upstream authority, so there is no CA to verify")
    if not isinstance(described, dict) or not isinstance(described.get("CertificateAuthority"), dict):
        raise EvidenceRefused("the file is not `aws acm-pca describe-certificate-authority` output")
    ca = described["CertificateAuthority"]
    if ca.get("Arn") != authority["certificate_authority_arn"]:
        raise EvidenceRefused("the described CA is not the certificate_authority_arn in the deployment input")
    if "OwnerAccount" in ca and ca["OwnerAccount"] != authority["certificate_authority_arn"].split(":")[4]:
        raise EvidenceRefused("the CA owner account differs from the account in its ARN")
    if ca.get("Status") != "ACTIVE":
        raise EvidenceRefused(f"the CA status is {ca.get('Status')!r}, not ACTIVE")
    settings = ca.get("CertificateAuthorityConfiguration")
    algorithm = settings.get("KeyAlgorithm") if isinstance(settings, dict) else None
    if not isinstance(algorithm, str) or not algorithm.startswith("EC_") or not authority["signing_algorithm"].endswith("WITHECDSA"):
        raise EvidenceRefused("the CA key algorithm must be elliptic curve to match the configured ECDSA signing algorithm")
    standard = ca.get("KeyStorageSecurityStandard")
    if standard not in KNOWN_STANDARDS:
        raise EvidenceRefused("the CA does not report a known KeyStorageSecurityStandard, so its key storage is unproven")
    if config["assurance_level"] == "regulated" and standard not in HARDWARE_STANDARDS:
        raise EvidenceRefused(f"the regulated assurance level requires FIPS 140-2 level 3 or higher key storage, not {standard}")
    return {
        "verified": True, "certificate_authority_arn": ca["Arn"], "status": ca["Status"], "type": ca.get("Type"),
        "key_algorithm": algorithm, "key_storage_security_standard": standard, "assurance_level": config["assurance_level"],
    }


def main(argv=None):
    parser = argparse.ArgumentParser(description="Refuse unless the saved describe-certificate-authority output proves the configured upstream CA.")
    parser.add_argument("--config", type=Path, required=True, help="deployment input JSON")
    parser.add_argument("--describe", type=Path, required=True, help="saved output of `aws acm-pca describe-certificate-authority`")
    args = parser.parse_args(argv)
    try:
        facts = verify(json.loads(args.config.read_text()), json.loads(args.describe.read_text()))
    except EvidenceRefused as error:
        parser.exit(1, f"upstream CA evidence refused: {error}\n")
    except (OSError, ValueError, KeyError, TypeError) as error:
        parser.exit(1, f"upstream CA evidence refused: input is not usable: {error}\n")
    sys.stdout.write(json.dumps(facts, indent=2, sort_keys=True) + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
