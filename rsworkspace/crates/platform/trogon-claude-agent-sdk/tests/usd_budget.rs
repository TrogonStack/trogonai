use trogon_claude_agent_sdk::{ClaudeAgentOptions, UsdBudget, UsdBudgetError};

#[test]
fn checked_budget_rejects_negative_and_non_finite_amounts_with_a_typed_error() {
    for amount in [-1.0, f64::NEG_INFINITY, f64::INFINITY, f64::NAN] {
        assert!(matches!(UsdBudget::checked(amount), Err(UsdBudgetError)));
    }
}

#[test]
fn budget_keeps_any_amount_like_both_upstream_sdks() {
    for amount in [-1.0, f64::NEG_INFINITY, f64::INFINITY, f64::NAN] {
        assert_eq!(UsdBudget::new(amount).get().to_bits(), amount.to_bits());
        assert_eq!(UsdBudget::from(amount).get().to_bits(), amount.to_bits());
    }
}

#[test]
fn budget_preserves_omission_zero_and_finite_amounts() -> Result<(), UsdBudgetError> {
    assert_eq!(ClaudeAgentOptions::default().max_budget_usd, None);
    for amount in [0.0_f64, -0.0, 0.25, f64::MAX] {
        let budget = UsdBudget::checked(amount)?;
        let options = ClaudeAgentOptions {
            max_budget_usd: Some(budget),
            ..ClaudeAgentOptions::default()
        };
        assert_eq!(options.max_budget_usd.map(UsdBudget::get), Some(amount));
        assert_eq!(budget.get().to_bits(), amount.to_bits());
    }
    Ok(())
}
