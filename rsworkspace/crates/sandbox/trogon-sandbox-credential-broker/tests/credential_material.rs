use std::error::Error;

use trogon_sandbox_contract::{CredentialScope, LeaseId};
use trogon_sandbox_credential_broker::{CredentialMaterial, CredentialMaterialError};

fn pull() -> CredentialScope {
    CredentialScope::RegistryPull {
        repository_glob: "r/*".to_string(),
    }
}

fn value(material: &CredentialMaterial, scope: &CredentialScope) -> Result<String, Box<dyn Error>> {
    Ok(material.issue_for_scope(scope)?.expose_secret().to_string())
}

#[test]
fn each_kind_yields_its_token_for_the_matching_scope() -> Result<(), Box<dyn Error>> {
    let basic = CredentialMaterial::RegistryBasic {
        username: "u".to_string(),
        password: "p".to_string(),
    };
    assert_eq!(value(&basic, &pull())?, "u:p");
    let push = CredentialScope::RegistryPush {
        repository_glob: "r/*".to_string(),
    };
    assert_eq!(value(&basic, &push)?, "u:p");
    let token = CredentialMaterial::RegistryToken {
        token: "tok".to_string(),
    };
    assert_eq!(value(&token, &pull())?, "tok");

    let s3_scope = CredentialScope::S3ReadWrite {
        bucket: "b".to_string(),
        prefix: String::new(),
    };
    let keys = CredentialMaterial::S3Keys {
        access_key_id: "ak".to_string(),
        secret_access_key: "sk".to_string(),
        session_token: None,
    };
    assert_eq!(value(&keys, &s3_scope)?, "ak:sk");
    let session = CredentialMaterial::S3Keys {
        access_key_id: "ak".to_string(),
        secret_access_key: "sk".to_string(),
        session_token: Some("st".to_string()),
    };
    assert_eq!(value(&session, &s3_scope)?, "ak:sk:st");

    let daytona = CredentialMaterial::DaytonaApi {
        api_token: "d".to_string(),
    };
    let daytona_scope = CredentialScope::DaytonaApi {
        workspace_id: "w".to_string(),
    };
    assert_eq!(value(&daytona, &daytona_scope)?, "d");

    let azure = CredentialMaterial::AzureServicePrincipal {
        client_id: "c".to_string(),
        client_secret: "s".to_string(),
        tenant_id: "t".to_string(),
    };
    let azure_scope = CredentialScope::AzureServicePrincipal {
        resource_id: "r".to_string(),
    };
    assert_eq!(value(&azure, &azure_scope)?, "c:s:t");

    let ssh = CredentialMaterial::SshGateway {
        gateway_id: "g".to_string(),
    };
    let ssh_scope = CredentialScope::SshSession {
        lease_id: LeaseId::new("l"),
    };
    assert_eq!(value(&ssh, &ssh_scope)?, "g:l");
    Ok(())
}

#[test]
fn a_mismatched_scope_is_refused() {
    let token = CredentialMaterial::RegistryToken {
        token: "tok".to_string(),
    };
    let scope = CredentialScope::DaytonaApi {
        workspace_id: "w".to_string(),
    };
    let error = token.issue_for_scope(&scope).err();
    assert_eq!(error, Some(CredentialMaterialError::ScopeMismatch));
    assert_eq!(
        CredentialMaterialError::ScopeMismatch.to_string(),
        "credential material does not match requested scope"
    );
}

#[test]
fn debug_names_the_kind_and_hides_every_secret() {
    let all = [
        CredentialMaterial::RegistryBasic {
            username: "u".to_string(),
            password: "hunter2".to_string(),
        },
        CredentialMaterial::RegistryToken {
            token: "hunter2".to_string(),
        },
        CredentialMaterial::S3Keys {
            access_key_id: "hunter2".to_string(),
            secret_access_key: "hunter2".to_string(),
            session_token: None,
        },
        CredentialMaterial::DaytonaApi {
            api_token: "hunter2".to_string(),
        },
        CredentialMaterial::AzureServicePrincipal {
            client_id: "hunter2".to_string(),
            client_secret: "hunter2".to_string(),
            tenant_id: "hunter2".to_string(),
        },
        CredentialMaterial::SshGateway {
            gateway_id: "hunter2".to_string(),
        },
    ];
    let names = [
        "RegistryBasic",
        "RegistryToken",
        "S3Keys",
        "DaytonaApi",
        "AzureServicePrincipal",
        "SshGateway",
    ];
    for (material, name) in all.iter().zip(names) {
        let shown = format!("{material:?}");
        assert!(shown.starts_with(name));
        assert!(!shown.contains("hunter2"));
    }
}
