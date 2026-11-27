use trogon_sandbox_capability_matrix::{MatrixError, check_doc};

#[test]
fn committed_matrix_matches_declared_profiles() -> Result<(), MatrixError> {
    check_doc()
}
