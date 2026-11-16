use super::TestKeyState;
use crate::{KeyPurpose, TenantId};
use zeroize::Zeroizing;

pub(super) struct TestKey {
    pub material: Zeroizing<[u8; 32]>,
    pub tenant: TenantId,
    pub purpose: KeyPurpose,
    pub state: TestKeyState,
}
