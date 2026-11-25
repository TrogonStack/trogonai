"""Verify saved AWS IAM and KMS output against the trust, permission and key policies this compiler renders.

The renderer cannot see what is attached to the server role in AWS, so the operator saves the output of
`aws iam get-role`, `aws iam get-role-policy`, `aws iam list-role-policies`, `aws iam list-attached-role-policies`
and `aws kms get-key-policy` (one per SPIRE signing key), and this verifier refuses unless the role is the one the
input names, its trust policy and its single inline policy equal the rendered documents exactly, no managed policy
is attached, and every key policy equals the rendered one. Equality is strict, so an extra principal, action,
resource, condition or statement is a refusal, and the message names what differs. Response shapes (AWS IAM and
KMS API references, as printed by the AWS CLI): get-role {"Role": {"RoleName", "Arn", "AssumeRolePolicyDocument"}},
get-role-policy {"RoleName", "PolicyName", "PolicyDocument"}, list-role-policies {"PolicyNames": [...]},
list-attached-role-policies {"AttachedPolicies": [...]}, get-key-policy {"PolicyName", "Policy": "<JSON text>"}.
The fixtures in testdata/ follow the documentation; they are not captures from a live call. Permission boundaries,
service control policies and session policies are not visible in this output and are not verified.
"""

import argparse
import json
import sys
from pathlib import Path

from iam import policies
from profile import validate


class EvidenceRefused(ValueError):
    pass


def grants(document):
    """Return the set of (effect, principal, action, resource) tuples a policy document grants, for naming differences."""
    found = set()
    statements = document.get("Statement", []) if isinstance(document, dict) else []
    for statement in [statements] if isinstance(statements, dict) else statements:
        listed = lambda value: [value] if isinstance(value, str) else sorted(json.dumps(item, sort_keys=True) for item in (value if isinstance(value, list) else [value]))
        for action in listed(statement.get("Action")):
            for principal in listed(statement.get("Principal", "")):
                for resource in listed(statement.get("Resource", "")):
                    found.add((statement.get("Effect"), principal, action, resource, json.dumps(statement.get("Condition"), sort_keys=True)))
    return found


def same(label, rendered, saved):
    if saved == rendered:
        return
    extra, missing = sorted(grants(saved) - grants(rendered)), sorted(grants(rendered) - grants(saved))
    detail = "; ".join(part for part in [
        f"extra grants {[item[:4] for item in extra][:3]}" if extra else "", f"missing grants {[item[:4] for item in missing][:3]}" if missing else "",
        "" if extra or missing else "the documents differ in structure or conditions"] if part)
    raise EvidenceRefused(f"{label} does not equal the rendered document: {detail}")


def document(value, key, label):
    if not isinstance(value, dict) or not isinstance(value.get(key), dict if key != "Policy" else str):
        raise EvidenceRefused(f"{label} is not the expected AWS CLI output (missing {key})")
    return json.loads(value[key]) if key == "Policy" else value[key]


def verify(config, role, role_policy, role_policies, attached, key_policies):
    """Return the verified facts, or raise EvidenceRefused naming the rule that failed."""
    validate(config)
    rendered = policies(config)
    name = config["server_role_arn"].rsplit("/", 1)[1]
    described = role.get("Role") if isinstance(role, dict) else None
    if not isinstance(described, dict) or described.get("Arn") != config["server_role_arn"]:
        raise EvidenceRefused("the described role is not the server_role_arn in the deployment input")
    same("the role trust policy", rendered["server-role-trust.json"], described.get("AssumeRolePolicyDocument"))
    names = role_policies.get("PolicyNames") if isinstance(role_policies, dict) else None
    if not isinstance(names, list) or len(names) != 1:
        raise EvidenceRefused("the role must carry exactly one inline policy")
    if not isinstance(attached, dict) or attached.get("AttachedPolicies") != []:
        raise EvidenceRefused("the role must carry no attached managed policy")
    if not isinstance(role_policy, dict) or role_policy.get("RoleName") != name or role_policy.get("PolicyName") != names[0]:
        raise EvidenceRefused("the saved role policy is not the role's single inline policy")
    same("the role permission policy", rendered["server-role-policy.json"], document(role_policy, "PolicyDocument", "get-role-policy output"))
    if not key_policies:
        raise EvidenceRefused("at least one saved KMS key policy is required")
    for index, saved in enumerate(key_policies):
        same(f"KMS key policy {index + 1}", rendered["kms-key-policy.json"], document(saved, "Policy", "get-key-policy output"))
    return {"verified": True, "server_role_arn": config["server_role_arn"], "inline_policy": names[0], "key_policies": len(key_policies)}


def main(argv=None):
    parser = argparse.ArgumentParser(description="Refuse unless saved IAM and KMS output equals the rendered server role and key policies.")
    parser.add_argument("--config", type=Path, required=True, help="deployment input JSON")
    parser.add_argument("--role", type=Path, required=True, help="saved `aws iam get-role --role-name <role>`")
    parser.add_argument("--role-policy", type=Path, required=True, help="saved `aws iam get-role-policy --role-name <role> --policy-name <name>`")
    parser.add_argument("--role-policies", type=Path, required=True, help="saved `aws iam list-role-policies --role-name <role>`")
    parser.add_argument("--attached-policies", type=Path, required=True, help="saved `aws iam list-attached-role-policies --role-name <role>`")
    parser.add_argument("--key-policy", type=Path, action="append", required=True, help="saved `aws kms get-key-policy --policy-name default`, once per SPIRE signing key")
    args = parser.parse_args(argv)
    load = lambda path: json.loads(path.read_text())
    try:
        facts = verify(load(args.config), load(args.role), load(args.role_policy), load(args.role_policies), load(args.attached_policies),
                       [load(path) for path in args.key_policy])
    except EvidenceRefused as error:
        parser.exit(1, f"IAM evidence refused: {error}\n")
    except (OSError, ValueError, KeyError, TypeError, AttributeError) as error:
        parser.exit(1, f"IAM evidence refused: input is not usable: {error}\n")
    sys.stdout.write(json.dumps(facts, indent=2, sort_keys=True) + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
