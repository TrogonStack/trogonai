import contextlib
import copy
import io
import json
import unittest
from pathlib import Path

from iam_evidence import EvidenceRefused, main, verify

HERE = Path(__file__).parent
FILES = {"role": "iam-get-role.json", "role_policy": "iam-get-role-policy.json", "role_policies": "iam-list-role-policies.json",
         "attached": "iam-list-attached-role-policies.json", "key": "kms-get-key-policy.json"}


def configuration():
    return json.loads((HERE / "deployment.example.json").read_text())


def saved():
    return {name: json.loads((HERE / "testdata" / file).read_text()) for name, file in FILES.items()}


def run(evidence, config=None):
    return verify(config or configuration(), evidence["role"], evidence["role_policy"], evidence["role_policies"], evidence["attached"], [evidence["key"]])


def with_key_policy(evidence, change):
    policy = json.loads(evidence["key"]["Policy"])
    change(policy)
    evidence["key"]["Policy"] = json.dumps(policy)


class IamEvidenceTests(unittest.TestCase):
    def refused(self, rule, evidence=None, config=None):
        with self.assertRaises(EvidenceRefused) as raised:
            run(evidence or saved(), config)
        self.assertIn(rule, str(raised.exception))

    def test_the_documented_output_equal_to_the_rendered_policies_is_accepted(self):
        facts = run(saved())
        self.assertEqual((facts["verified"], facts["inline_policy"], facts["key_policies"]), (True, "identity-server-signing", 1))

    def test_the_fixtures_equal_what_the_example_deployment_renders(self):
        from iam import policies
        rendered, evidence = policies(configuration()), saved()
        self.assertEqual(evidence["role"]["Role"]["AssumeRolePolicyDocument"], rendered["server-role-trust.json"])
        self.assertEqual(evidence["role_policy"]["PolicyDocument"], rendered["server-role-policy.json"])
        self.assertEqual(json.loads(evidence["key"]["Policy"]), rendered["kms-key-policy.json"])

    def test_a_different_role_is_refused(self):
        evidence = saved()
        evidence["role"]["Role"]["Arn"] = evidence["role"]["Role"]["Arn"].replace("identity-server", "other")
        self.refused("not the server_role_arn", evidence)

    def test_an_extra_trusted_principal_is_refused_and_named(self):
        evidence = saved()
        evidence["role"]["Role"]["AssumeRolePolicyDocument"]["Statement"].append(
            {"Effect": "Allow", "Principal": {"AWS": "arn:aws:iam::999999999999:root"}, "Action": "sts:AssumeRole"})
        self.refused("extra grants", evidence)
        self.refused("999999999999", evidence)

    def test_a_widened_trust_subject_condition_is_refused(self):
        evidence = saved()
        condition = evidence["role"]["Role"]["AssumeRolePolicyDocument"]["Statement"][0]["Condition"]["StringEquals"]
        key = next(key for key in condition if key.endswith(":sub"))
        evidence["role"]["Role"]["AssumeRolePolicyDocument"]["Statement"][0]["Condition"] = {"StringLike": {key: "system:serviceaccount:*:*"}}
        self.refused("trust policy does not equal", evidence)

    def test_an_extra_permission_action_is_refused_and_named(self):
        evidence = saved()
        evidence["role_policy"]["PolicyDocument"]["Statement"][0]["Action"] = ["kms:CreateKey", "kms:Decrypt"]
        self.refused("kms:Decrypt", evidence)

    def test_an_extra_permission_statement_is_refused(self):
        evidence = saved()
        evidence["role_policy"]["PolicyDocument"]["Statement"].append({"Effect": "Allow", "Action": "s3:*", "Resource": "*"})
        self.refused("s3:*", evidence)

    def test_a_removed_condition_is_refused(self):
        evidence = saved()
        del evidence["role_policy"]["PolicyDocument"]["Statement"][1]["Condition"]
        self.refused("role permission policy does not equal", evidence)

    def test_a_second_inline_policy_or_an_attached_managed_policy_is_refused(self):
        evidence = saved()
        evidence["role_policies"]["PolicyNames"].append("extra")
        self.refused("exactly one inline policy", evidence)
        evidence = saved()
        evidence["attached"]["AttachedPolicies"] = [{"PolicyName": "AdministratorAccess", "PolicyArn": "arn:aws:iam::aws:policy/AdministratorAccess"}]
        self.refused("no attached managed policy", evidence)

    def test_a_policy_that_is_not_the_listed_inline_policy_is_refused(self):
        evidence = saved()
        evidence["role_policy"]["PolicyName"] = "another"
        self.refused("single inline policy", evidence)

    def test_an_extra_key_policy_principal_is_refused_and_named(self):
        evidence = saved()
        with_key_policy(evidence, lambda policy: policy["Statement"][0]["Principal"].update(AWS=[policy["Statement"][0]["Principal"]["AWS"], "arn:aws:iam::999999999999:root"]))
        self.refused("999999999999", evidence)

    def test_an_extra_key_policy_action_is_refused(self):
        evidence = saved()
        with_key_policy(evidence, lambda policy: policy["Statement"][1]["Action"].append("kms:Decrypt"))
        self.refused("kms:Decrypt", evidence)

    def test_every_saved_key_policy_is_checked(self):
        evidence = saved()
        bad = copy.deepcopy(evidence["key"])
        policy = json.loads(bad["Policy"])
        policy["Statement"].append({"Effect": "Allow", "Principal": "*", "Action": "kms:*", "Resource": "*"})
        bad["Policy"] = json.dumps(policy)
        with self.assertRaises(EvidenceRefused) as raised:
            verify(configuration(), evidence["role"], evidence["role_policy"], evidence["role_policies"], evidence["attached"], [evidence["key"], bad])
        self.assertIn("KMS key policy 2", str(raised.exception))

    def test_no_key_policy_and_output_of_the_wrong_shape_are_refused(self):
        evidence = saved()
        with self.assertRaises(EvidenceRefused):
            verify(configuration(), evidence["role"], evidence["role_policy"], evidence["role_policies"], evidence["attached"], [])
        for field, value in [("role", {}), ("role_policy", {"RoleName": "identity-server", "PolicyName": "identity-server-signing"}),
                             ("role_policies", []), ("attached", {}), ("key", {"Policy": {}})]:
            with self.subTest(field=field):
                changed = saved()
                changed[field] = value
                with self.assertRaises(EvidenceRefused):
                    run(changed)

    def test_an_invalid_deployment_input_is_refused(self):
        config = configuration()
        config["assurance_level"] = "unrestricted"
        with self.assertRaises(ValueError):
            run(saved(), config)


class CommandLineTests(unittest.TestCase):
    def command(self, evidence_dir):
        stdout, stderr = io.StringIO(), io.StringIO()
        arguments = ["--config", str(HERE / "deployment.example.json"), "--role", str(evidence_dir / FILES["role"]),
                     "--role-policy", str(evidence_dir / FILES["role_policy"]), "--role-policies", str(evidence_dir / FILES["role_policies"]),
                     "--attached-policies", str(evidence_dir / FILES["attached"]), "--key-policy", str(evidence_dir / FILES["key"])]
        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            try:
                status = main(arguments)
            except SystemExit as raised:
                status = raised.code
        return status, stdout.getvalue(), stderr.getvalue()

    def test_the_documented_fixtures_exit_zero_and_print_the_verified_facts(self):
        status, out, _ = self.command(HERE / "testdata")
        self.assertEqual(status, 0)
        self.assertTrue(json.loads(out)["verified"])

    def test_changed_evidence_exits_one_with_the_rule(self):
        import tempfile
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory)
            for file in FILES.values():
                (target / file).write_text((HERE / "testdata" / file).read_text())
            policy = json.loads((target / FILES["role_policy"]).read_text())
            policy["PolicyDocument"]["Statement"][0]["Action"] = "kms:*"
            (target / FILES["role_policy"]).write_text(json.dumps(policy))
            status, out, err = self.command(target)
        self.assertEqual((status, out), (1, ""))
        self.assertIn("role permission policy does not equal", err)


if __name__ == "__main__":
    unittest.main()
