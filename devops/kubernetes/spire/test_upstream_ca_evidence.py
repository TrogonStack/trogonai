import contextlib
import copy
import io
import json
import tempfile
import unittest
from pathlib import Path

from upstream_ca_evidence import EvidenceRefused, main, verify

HERE = Path(__file__).parent


def configuration():
    return json.loads((HERE / "deployment.example.json").read_text())


def described():
    return json.loads((HERE / "testdata" / "acm-pca-describe-certificate-authority.json").read_text())


class UpstreamCaEvidenceTests(unittest.TestCase):
    def refused(self, rule, config=None, evidence=None):
        with self.assertRaises(EvidenceRefused) as raised:
            verify(config or configuration(), described() if evidence is None else evidence)
        self.assertIn(rule, str(raised.exception))

    def test_the_documented_describe_output_for_the_configured_authority_is_accepted(self):
        facts = verify(configuration(), described())
        self.assertEqual(facts["key_storage_security_standard"], "FIPS_140_2_LEVEL_3_OR_HIGHER")
        self.assertEqual(facts["certificate_authority_arn"], configuration()["upstream_authority"]["certificate_authority_arn"])

    def test_a_different_authority_arn_is_refused(self):
        evidence = described()
        evidence["CertificateAuthority"]["Arn"] = evidence["CertificateAuthority"]["Arn"].replace("12ac02bc", "22ac02bc")
        self.refused("not the certificate_authority_arn", evidence=evidence)

    def test_an_authority_that_is_not_active_is_refused(self):
        for status in ["PENDING_CERTIFICATE", "DISABLED", "EXPIRED", "DELETED", "FAILED", None]:
            with self.subTest(status=status):
                evidence = described()
                evidence["CertificateAuthority"]["Status"] = status
                self.refused("not ACTIVE", evidence=evidence)

    def test_regulated_requires_hardware_level_three_key_storage(self):
        evidence = described()
        evidence["CertificateAuthority"]["KeyStorageSecurityStandard"] = "FIPS_140_2_LEVEL_2_OR_HIGHER"
        self.refused("level 3 or higher", evidence=evidence)

    def test_the_ccpc_equivalent_level_is_accepted(self):
        evidence = described()
        evidence["CertificateAuthority"]["KeyStorageSecurityStandard"] = "CCPC_LEVEL_1_OR_HIGHER"
        self.assertEqual(verify(configuration(), evidence)["key_storage_security_standard"], "CCPC_LEVEL_1_OR_HIGHER")

    def test_missing_or_unknown_key_storage_standard_is_unproven_and_refused(self):
        for value in ["SOFTWARE", None]:
            with self.subTest(value=value):
                evidence = described()
                if value is None:
                    del evidence["CertificateAuthority"]["KeyStorageSecurityStandard"]
                else:
                    evidence["CertificateAuthority"]["KeyStorageSecurityStandard"] = value
                self.refused("unproven", evidence=evidence)

    def test_the_standard_assurance_level_accepts_level_two_key_storage(self):
        config = configuration()
        config["assurance_level"] = "standard"
        evidence = described()
        evidence["CertificateAuthority"]["KeyStorageSecurityStandard"] = "FIPS_140_2_LEVEL_2_OR_HIGHER"
        self.assertTrue(verify(config, evidence)["verified"])

    def test_a_deployment_without_an_upstream_authority_has_nothing_to_verify(self):
        config = configuration()
        config["assurance_level"], config["upstream_authority"] = "standard", None
        self.refused("no upstream authority", config=config)

    def test_a_non_elliptic_curve_authority_cannot_sign_with_the_configured_ecdsa_algorithm(self):
        evidence = described()
        evidence["CertificateAuthority"]["CertificateAuthorityConfiguration"]["KeyAlgorithm"] = "RSA_2048"
        self.refused("elliptic curve", evidence=evidence)

    def test_an_owner_account_that_differs_from_the_arn_is_refused(self):
        evidence = described()
        evidence["CertificateAuthority"]["OwnerAccount"] = "999999999999"
        self.refused("owner account", evidence=evidence)

    def test_output_that_is_not_a_describe_response_is_refused(self):
        for evidence in [{}, [], {"CertificateAuthority": []}, {"CertificateAuthorities": []}]:
            with self.subTest(evidence=evidence):
                self.refused("describe-certificate-authority", evidence=evidence)

    def test_an_invalid_deployment_input_is_refused(self):
        config = configuration()
        config["assurance_level"] = "unrestricted"
        with self.assertRaises(ValueError):
            verify(config, described())


class CommandLineTests(unittest.TestCase):
    def command(self, evidence):
        with tempfile.TemporaryDirectory() as directory:
            describe = Path(directory) / "describe.json"
            describe.write_text(json.dumps(evidence))
            stdout, stderr = io.StringIO(), io.StringIO()
            with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
                try:
                    status = main(["--config", str(HERE / "deployment.example.json"), "--describe", str(describe)])
                except SystemExit as raised:
                    status = raised.code
            return status, stdout.getvalue(), stderr.getvalue()

    def test_valid_evidence_exits_zero_and_prints_the_verified_facts(self):
        status, out, _ = self.command(described())
        self.assertEqual(status, 0)
        self.assertTrue(json.loads(out)["verified"])

    def test_refused_evidence_exits_one_with_the_rule(self):
        evidence = described()
        evidence["CertificateAuthority"]["Status"] = "DISABLED"
        status, out, err = self.command(evidence)
        self.assertEqual((status, out), (1, ""))
        self.assertIn("not ACTIVE", err)


if __name__ == "__main__":
    unittest.main()
