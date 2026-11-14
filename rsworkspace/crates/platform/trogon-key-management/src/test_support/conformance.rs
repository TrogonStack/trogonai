use super::{ConformanceCase, ConformanceError};
use crate::{AuthenticatedBinding, Dek, KeyOperationError, KeyRef, UnwrapDek, WrapDek, WrappedDek};
use zeroize::Zeroizing;

// The caller classifies its native error at the test boundary. Operational
// failures never count as cryptographic refusal, and both keys need positive
// controls. Native authorization/custody/deployment tests remain separate.
pub async fn check_wrapping_conformance<T>(
    adapter: &T,
    key: &KeyRef,
    other_key: &KeyRef,
    binding: &AuthenticatedBinding,
    other_binding: &AuthenticatedBinding,
    classify: impl Fn(&<T as WrapDek>::Error) -> KeyOperationError,
) -> Result<(), ConformanceError<<T as WrapDek>::Error>>
where
    T: WrapDek + UnwrapDek<Error = <T as WrapDek>::Error>,
{
    if key == other_key || binding == other_binding {
        return Err(ConformanceError::AliasedFixture);
    }
    let wrapped = adapter
        .wrap_dek(key, Dek::from_bytes(Zeroizing::new([0x42; 32])), binding)
        .await
        .map_err(ConformanceError::Wrap)?;
    check_positive(adapter, key, &wrapped, binding, &[0x42; 32]).await?;
    let alternate = adapter
        .wrap_dek(other_key, Dek::from_bytes(Zeroizing::new([0x24; 32])), binding)
        .await
        .map_err(ConformanceError::Wrap)?;
    check_positive(adapter, other_key, &alternate, binding, &[0x24; 32]).await?;
    match adapter.unwrap_dek(other_key, &wrapped, binding).await {
        Ok(_) => return Err(ConformanceError::KeySubstitutionAccepted),
        Err(source) => {
            let failure = classify(&source);
            if !matches!(
                failure,
                KeyOperationError::IntegrityFailure | KeyOperationError::BindingMismatch
            ) {
                return Err(ConformanceError::UnexpectedRefusal {
                    case: ConformanceCase::KeySubstitution,
                    failure,
                    source,
                });
            }
        }
    }
    match adapter.unwrap_dek(key, &wrapped, other_binding).await {
        Ok(_) => return Err(ConformanceError::BindingSubstitutionAccepted),
        Err(source) => {
            let failure = classify(&source);
            if !matches!(
                failure,
                KeyOperationError::IntegrityFailure | KeyOperationError::BindingMismatch
            ) {
                return Err(ConformanceError::UnexpectedRefusal {
                    case: ConformanceCase::BindingSubstitution,
                    failure,
                    source,
                });
            }
        }
    }
    let mut altered = wrapped.adapter_payload().to_vec();
    altered[0] ^= 1;
    let altered = WrappedDek::new(wrapped.suite(), wrapped.format_version(), altered)
        .map_err(|_| ConformanceError::TamperingAccepted)?;
    match adapter.unwrap_dek(key, &altered, binding).await {
        Ok(_) => return Err(ConformanceError::TamperingAccepted),
        Err(source) => {
            let failure = classify(&source);
            if failure != KeyOperationError::IntegrityFailure {
                return Err(ConformanceError::UnexpectedRefusal {
                    case: ConformanceCase::Tampering,
                    failure,
                    source,
                });
            }
        }
    }
    check_positive(adapter, key, &wrapped, binding, &[0x42; 32]).await?;
    check_positive(adapter, other_key, &alternate, binding, &[0x24; 32]).await?;
    Ok(())
}

async fn check_positive<T>(
    adapter: &T,
    key: &KeyRef,
    wrapped: &WrappedDek,
    binding: &AuthenticatedBinding,
    expected: &[u8; 32],
) -> Result<(), ConformanceError<<T as WrapDek>::Error>>
where
    T: WrapDek + UnwrapDek<Error = <T as WrapDek>::Error>,
{
    let opened = adapter
        .unwrap_dek(key, wrapped, binding)
        .await
        .map_err(ConformanceError::Unwrap)?;
    if opened.expose() != expected {
        return Err(ConformanceError::PlaintextMismatch);
    }
    Ok(())
}
