use async_trait::async_trait;
use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde_json::Value;
use tokio::sync::RwLock;

use crate::CLOCK_SKEW_SECS;
use crate::attestor::{AttestError, PresentedCreds, WorkloadAttestor};
use crate::spiffe_id::SpiffeId;
use crate::workload_svid::WorkloadSvid;

const K8S_NAMESPACE_CLAIM: &str = "kubernetes.io/serviceaccount/namespace";
const K8S_SA_NAME_CLAIM: &str = "kubernetes.io/serviceaccount/service-account.name";

pub struct K8sServiceAccountAttestor {
    jwks: RwLock<JwkSet>,
    issuer: String,
    audience: Vec<String>,
    trust_domain: String,
}

impl K8sServiceAccountAttestor {
    pub fn new(jwks: JwkSet, issuer: String, audience: Vec<String>, trust_domain: String) -> Self {
        Self {
            jwks: RwLock::new(jwks),
            issuer,
            audience,
            trust_domain,
        }
    }

    pub async fn update_jwks(&self, jwks: JwkSet) {
        *self.jwks.write().await = jwks;
    }
}

#[async_trait]
impl WorkloadAttestor for K8sServiceAccountAttestor {
    async fn attest(&self, presented: &PresentedCreds) -> Result<WorkloadSvid, AttestError> {
        let token = presented.actor_token.trim();
        if token.is_empty() {
            return Err(AttestError::Denied("k8s SA actor_token required".into()));
        }
        let header = decode_header(token).map_err(|e| AttestError::Denied(format!("jwt header: {e}")))?;
        let jwks = self.jwks.read().await;
        let jwk = pick_jwk(&jwks, header.alg, header.kid.as_deref())?;
        let decoding_key = DecodingKey::from_jwk(jwk).map_err(|e| AttestError::Denied(format!("jwk: {e}")))?;
        let mut validation = Validation::new(header.alg);
        validation.leeway = CLOCK_SKEW_SECS as u64;
        validation.validate_exp = true;
        validation.validate_nbf = true;
        validation.set_issuer(&[self.issuer.as_str()]);
        if self.audience.is_empty() {
            validation.validate_aud = false;
        } else {
            let aud_refs: Vec<&str> = self.audience.iter().map(String::as_str).collect();
            validation.set_audience(&aud_refs);
        }
        let claims = decode::<Value>(token, &decoding_key, &validation)
            .map_err(|e| AttestError::Denied(format!("k8s SA token invalid: {e}")))?
            .claims;
        let map = claims
            .as_object()
            .ok_or_else(|| AttestError::Denied("k8s SA claims must be object".into()))?;

        let namespace = string_claim(map, K8S_NAMESPACE_CLAIM)
            .ok_or_else(|| AttestError::Denied("k8s SA claim missing namespace".into()))?;
        let name = string_claim(map, K8S_SA_NAME_CLAIM)
            .ok_or_else(|| AttestError::Denied("k8s SA claim missing service-account.name".into()))?;

        let spiffe_uri = format!("spiffe://{}/ns/{}/sa/{}", self.trust_domain, namespace, name);
        let spiffe_id = SpiffeId::parse(&spiffe_uri).map_err(|e| AttestError::Denied(e.to_string()))?;
        Ok(WorkloadSvid::new(spiffe_id, Vec::new(), token.to_string()))
    }
}

fn pick_jwk<'a>(
    jwks: &'a JwkSet,
    alg: Algorithm,
    kid: Option<&str>,
) -> Result<&'a jsonwebtoken::jwk::Jwk, AttestError> {
    let compatible: Vec<_> = jwks.keys.iter().filter(|k| jwk_compatible_with_alg(k, alg)).collect();
    if compatible.is_empty() {
        return Err(AttestError::Unavailable(
            "k8s JWKS contained no keys compatible with token algorithm".into(),
        ));
    }
    if let Some(kid) = kid
        && let Some(found) = compatible.iter().find(|k| k.common.key_id.as_deref() == Some(kid))
    {
        return Ok(found);
    }
    if compatible.len() == 1 {
        return Ok(compatible[0]);
    }
    Err(AttestError::Denied("jwt kid did not match any k8s JWKS keys".into()))
}

fn jwk_compatible_with_alg(jwk: &jsonwebtoken::jwk::Jwk, alg: Algorithm) -> bool {
    use jsonwebtoken::jwk::AlgorithmParameters;
    matches!(
        (&jwk.algorithm, alg),
        (AlgorithmParameters::RSA(_), Algorithm::RS256)
            | (AlgorithmParameters::EllipticCurve(_), Algorithm::ES256)
            | (AlgorithmParameters::EllipticCurve(_), Algorithm::ES384)
    )
}

fn string_claim(map: &serde_json::Map<String, Value>, key: &str) -> Option<String> {
    map.get(key).and_then(|v| match v {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::jwk::{
        AlgorithmParameters, CommonParameters, Jwk, KeyAlgorithm, PublicKeyUse, RSAKeyParameters, RSAKeyType,
    };
    use jsonwebtoken::{EncodingKey, Header, encode};
    use rsa::pkcs1::EncodeRsaPrivateKey;
    use rsa::pkcs8::EncodePublicKey;
    use rsa::traits::PublicKeyParts;
    use rsa::{RsaPrivateKey, RsaPublicKey};
    use serde_json::json;

    fn b64url(bytes: &[u8]) -> String {
        use base64::Engine;
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    }

    struct TestKey {
        priv_pem: String,
        jwk: Jwk,
    }

    fn build_test_key(kid: &str) -> TestKey {
        let mut rng = rand::thread_rng();
        let private = RsaPrivateKey::new(&mut rng, 2048).expect("rsa key");
        let public = RsaPublicKey::from(&private);
        let priv_pem = private
            .to_pkcs1_pem(rsa::pkcs1::LineEnding::LF)
            .expect("priv pem")
            .to_string();
        let _ = public.to_public_key_pem(rsa::pkcs8::LineEnding::LF);
        let n = b64url(&public.n().to_bytes_be());
        let e = b64url(&public.e().to_bytes_be());
        let jwk = Jwk {
            common: CommonParameters {
                public_key_use: Some(PublicKeyUse::Signature),
                key_algorithm: Some(KeyAlgorithm::RS256),
                key_id: Some(kid.to_string()),
                ..Default::default()
            },
            algorithm: AlgorithmParameters::RSA(RSAKeyParameters {
                key_type: RSAKeyType::RSA,
                n,
                e,
            }),
        };
        TestKey { priv_pem, jwk }
    }

    fn sign_token(key: &TestKey, claims: &Value, kid: &str) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(kid.to_string());
        let enc = EncodingKey::from_rsa_pem(key.priv_pem.as_bytes()).expect("enc");
        encode(&header, claims, &enc).expect("sign")
    }

    #[tokio::test]
    async fn projected_sa_token_maps_to_spiffe_uri() {
        let kid = "k8s-1";
        let key = build_test_key(kid);
        let jwks = JwkSet {
            keys: vec![key.jwk.clone()],
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let claims = json!({
            "iss": "https://kubernetes.default.svc",
            "aud": ["mcp-sts"],
            "exp": now + 300,
            "iat": now,
            "sub": "system:serviceaccount:prod:oncall-agent",
            K8S_NAMESPACE_CLAIM: "prod",
            K8S_SA_NAME_CLAIM: "oncall-agent",
        });
        let token = sign_token(&key, &claims, kid);
        let attestor = K8sServiceAccountAttestor::new(
            jwks,
            "https://kubernetes.default.svc".into(),
            vec!["mcp-sts".into()],
            "acme.local".into(),
        );
        let svid = attestor
            .attest(&PresentedCreds {
                actor_token: token,
                peer_cert_pem: None,
            })
            .await
            .expect("attest");
        assert_eq!(svid.wkl(), "spiffe://acme.local/ns/prod/sa/oncall-agent");
    }

    #[tokio::test]
    async fn missing_namespace_claim_denies() {
        let kid = "k8s-1";
        let key = build_test_key(kid);
        let jwks = JwkSet {
            keys: vec![key.jwk.clone()],
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let claims = json!({
            "iss": "https://kubernetes.default.svc",
            "exp": now + 300,
            "iat": now,
            "sub": "system:serviceaccount:prod:oncall-agent",
            K8S_SA_NAME_CLAIM: "oncall-agent",
        });
        let token = sign_token(&key, &claims, kid);
        let attestor = K8sServiceAccountAttestor::new(
            jwks,
            "https://kubernetes.default.svc".into(),
            vec![],
            "acme.local".into(),
        );
        let err = attestor
            .attest(&PresentedCreds {
                actor_token: token,
                peer_cert_pem: None,
            })
            .await
            .expect_err("denied");
        assert!(matches!(err, AttestError::Denied(_)));
    }
}
