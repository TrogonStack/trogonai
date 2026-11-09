def policies(config):
    account = config["server_role_arn"].split(":")[4]
    key_arn = f"arn:aws:kms:{config['aws_region']}:{account}:key/*"
    alias_arn = f"arn:aws:kms:{config['aws_region']}:{account}:alias/SPIRE_SERVER/{config['trust_domain']}/*"
    key_actions = [
        "kms:DescribeKey", "kms:GetPublicKey", "kms:Sign", "kms:ScheduleKeyDeletion",
        "kms:TagResource", "kms:PutKeyPolicy", "kms:CreateAlias", "kms:UpdateAlias",
    ]
    server_policy = {"Version": "2012-10-17", "Statement": [
        {"Sid": "CreateTaggedSigningKeys", "Effect": "Allow", "Action": "kms:CreateKey", "Resource": "*",
         "Condition": {"StringEquals": {
             "aws:RequestTag/spire-server-td": config["trust_domain"],
             "aws:RequestTag/Environment": config["environment"],
             "kms:KeyUsage": "SIGN_VERIFY", "kms:KeySpec": "ECC_NIST_P256",
         }}},
        {"Sid": "ManageTrustDomainKeys", "Effect": "Allow", "Action": key_actions, "Resource": key_arn,
         "Condition": {"StringEquals": {"aws:ResourceTag/spire-server-td": config["trust_domain"]}}},
        {"Sid": "ManageTrustDomainAliases", "Effect": "Allow",
         "Action": ["kms:CreateAlias", "kms:UpdateAlias", "kms:DeleteAlias"], "Resource": alias_arn},
        {"Sid": "DiscoverManagedKeys", "Effect": "Allow", "Action": ["kms:ListAliases", "tag:GetResources"], "Resource": "*"},
    ]}
    key_policy = {"Version": "2012-10-17", "Statement": [
        {"Sid": "ManageThisSigningKey", "Effect": "Allow", "Principal": {"AWS": config["server_role_arn"]},
         "Action": key_actions, "Resource": "*"},
        {"Sid": "IndependentKeyRecovery", "Effect": "Allow", "Principal": {"AWS": config["key_admin_role_arn"]},
         "Action": ["kms:DescribeKey", "kms:GetKeyPolicy", "kms:PutKeyPolicy", "kms:DisableKey",
                    "kms:EnableKey", "kms:ScheduleKeyDeletion", "kms:CancelKeyDeletion", "kms:ListResourceTags"],
         "Resource": "*"},
    ]}
    provider = config["oidc_provider_arn"].split("oidc-provider/", 1)[1]
    trust = {"Version": "2012-10-17", "Statement": [{
        "Effect": "Allow", "Principal": {"Federated": config["oidc_provider_arn"]},
        "Action": "sts:AssumeRoleWithWebIdentity", "Condition": {"StringEquals": {
            provider + ":aud": "sts.amazonaws.com",
            provider + ":sub": f"system:serviceaccount:{config['namespace']}:spire-server",
        }},
    }]}
    return {"server-role-policy.json": server_policy, "kms-key-policy.json": key_policy, "server-role-trust.json": trust}
