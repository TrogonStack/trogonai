use super::*;

/// The four lowered-integrity attempts blocked, the rest as given.
fn report(p: Outcome, a: Outcome, al: Outcome, d: Outcome) -> Report {
    Report {
        plain: p,
        anonymous: a,
        anonymous_loopback: al,
        deny_only: d,
        low_integrity: Outcome::Blocked,
        low_integrity_loopback: Outcome::Blocked,
        untrusted_integrity: Outcome::Blocked,
        untrusted_integrity_loopback: Outcome::Blocked,
        open_loopback: Outcome::Skipped,
    }
}

/// The verdict with loopback closed.
fn closed(r: &Report) -> Verdict {
    verdict(r, false)
}

/// Loopback open: the residual-only loopback attempts skipped,
/// `plain` and `open_loopback` as given, the rest blocked.
fn opened(plain: Outcome, open_loopback: Outcome) -> Report {
    Report {
        open_loopback,
        ..report(plain, Outcome::Skipped, Outcome::Skipped, Outcome::Skipped)
    }
}

#[test]
fn verdict_with_loopback_open() {
    let v = verdict(&opened(Blocked, Connected), true);
    assert_eq!((v.exit, v.residuals.is_empty()), (EXIT_FENCED, true));
    // Nothing listens at the reserved port: only the filter's refusal
    // passes. Refused by the stack, connected, or no answer in time
    // all mean the block did not fire.
    for out in [Connected, Refused, Unreachable] {
        assert_eq!(verdict(&opened(out, Connected), true).exit, EXIT_NOT_FENCED, "{out:?}");
        let stopped = Report::unfenced_at_plain(out);
        assert_eq!(verdict(&stopped, true).exit, EXIT_NOT_FENCED, "{out:?}");
    }
    // No connect was made at all: unknown, and still not a pass.
    for unknown in [Error, Skipped] {
        assert_eq!(verdict(&opened(unknown, Connected), true).exit, EXIT_INCONCLUSIVE);
    }
    // Loopback recorded open, but the connect to our own listener was refused.
    assert_eq!(verdict(&opened(Blocked, Blocked), true).exit, EXIT_LOOPBACK_MISMATCH);
    for unknown in [Refused, Unreachable, Error, Skipped] {
        assert_eq!(verdict(&opened(Blocked, unknown), true).exit, EXIT_INCONCLUSIVE);
    }
    // A lowered-integrity copy reaching the reserved port fails it.
    let mut leak = opened(Blocked, Connected);
    leak.untrusted_integrity_loopback = Refused;
    assert_eq!(verdict(&leak, true).exit, EXIT_NOT_FENCED);
    // With loopback closed the target has a listener: no answer is unknown.
    assert_eq!(
        closed(&report(Unreachable, Blocked, Blocked, Blocked)).exit,
        EXIT_INCONCLUSIVE
    );
}

#[test]
fn the_blocked_loopback_target_is_the_reserved_port() {
    let t = blocked_loopback_target();
    assert!(t.ip().is_loopback());
    assert_eq!(t.port(), crate::wfp::LOOPBACK_PROBE_PORT);
}

/// `r` with the lowered-integrity attempts replaced, in field order.
fn lowered(r: Report, l: [Outcome; 4]) -> Report {
    Report {
        low_integrity: l[0],
        low_integrity_loopback: l[1],
        untrusted_integrity: l[2],
        untrusted_integrity_loopback: l[3],
        ..r
    }
}

#[test]
fn report_round_trips_every_combination() {
    for p in Outcome::ALL {
        for a in Outcome::ALL {
            for al in Outcome::ALL {
                for d in Outcome::ALL {
                    // Reordered, so every field sees every outcome
                    // beside differing neighbours.
                    let r = lowered(report(p, a, al, d), [d, al, a, p]);
                    assert_eq!(Report::decode(r.encode()), Some(r));
                }
            }
        }
    }
}

#[test]
fn unrelated_exit_codes_do_not_decode() {
    for code in [
        0,
        1,
        2,
        3,
        15,
        255,
        0x0FFF,
        0x1000,
        0x2000,
        REPORT_BASE,
        REPORT_BASE - 1,
        REPORT_BASE << 1,
        0xC000_0005,
    ] {
        assert_eq!(Report::decode(code), None, "{code:#x}");
    }
    // In range, but a field holds no outcome.
    assert_eq!(Report::decode(REPORT_BASE | 7), None);
}

#[test]
fn classify_error_codes() {
    assert_eq!(classify_error(10013), Outcome::Blocked);
    assert_eq!(classify_error(10061), Outcome::Refused);
    // ERROR_ACCESS_DENIED is not a filter refusal.
    assert_eq!(classify_error(5), Outcome::Unreachable);
    assert_eq!(classify_error(10060), Outcome::Unreachable);
}

use Outcome::*;

#[test]
fn worse_ranks_outcomes_for_the_fence() {
    let ranked = [Connected, Refused, Error, Unreachable, Skipped, Blocked];
    assert_eq!(ranked.len(), Outcome::ALL.len());
    for (i, a) in ranked.iter().enumerate() {
        assert_eq!(a.worse(*a), *a);
        for b in &ranked[i + 1..] {
            assert_eq!(a.worse(*b), *a, "{a:?} vs {b:?}");
            assert_eq!(b.worse(*a), *a, "{b:?} vs {a:?}");
        }
    }
}

#[test]
fn worse_of_two_orderings() {
    assert!(Blocked.worse(Connected).passed_fence());
    assert!(Refused.worse(Blocked).passed_fence());
    // A block in one ordering does not stand in for an ordering
    // that was not measured.
    assert_eq!(Blocked.worse(Error), Error);
    assert_eq!(Unreachable.worse(Blocked), Unreachable);
    assert_eq!(Blocked.worse(Blocked), Blocked);
    assert_eq!(Error.worse(Connected), Connected);
}

#[test]
fn socket_orders_are_named_apart() {
    assert_eq!(SocketOrder::BOTH.len(), 2);
    assert_ne!(SocketOrder::BeforeToken.label(), SocketOrder::AfterToken.label());
}

#[test]
fn verdict_fenced_without_residuals() {
    let v = closed(&report(Blocked, Blocked, Blocked, Blocked));
    assert_eq!(v.exit, EXIT_FENCED);
    assert!(v.residuals.is_empty());
}

#[test]
fn verdict_names_each_residual() {
    let v = closed(&report(Blocked, Blocked, Connected, Connected));
    assert_eq!(v.exit, EXIT_FENCED);
    assert_eq!(v.residuals, [RESIDUAL_ANONYMOUS_LOOPBACK, RESIDUAL_DENY_ONLY]);
    let v = closed(&report(Blocked, Connected, Connected, Connected));
    assert_eq!(v.exit, EXIT_FENCED);
    assert_eq!(
        v.residuals,
        [
            RESIDUAL_ANONYMOUS_EGRESS,
            RESIDUAL_ANONYMOUS_LOOPBACK,
            RESIDUAL_DENY_ONLY
        ]
    );
    let v = closed(&report(Blocked, Blocked, Blocked, Refused));
    assert_eq!(v.residuals, [RESIDUAL_DENY_ONLY]);
    // An attempt that could not be made is not a residual.
    let v = closed(&report(Blocked, Blocked, Error, Unreachable));
    assert_eq!(v.exit, EXIT_FENCED);
    assert!(v.residuals.is_empty());
}

#[test]
fn verdict_plain_getting_out_is_not_fenced() {
    for through in [Connected, Refused] {
        let v = closed(&report(through, Blocked, Blocked, Blocked));
        assert_eq!(v.exit, EXIT_NOT_FENCED);
        // Residuals qualify a pass; a failure reports none.
        let v = closed(&report(through, Connected, Connected, Connected));
        assert_eq!(v.exit, EXIT_NOT_FENCED);
        assert!(v.residuals.is_empty());
    }
}

#[test]
fn verdict_anonymous_egress_is_a_residual_never_a_failure() {
    for through in [Connected, Refused] {
        let v = closed(&report(Blocked, through, Blocked, Blocked));
        assert_eq!(v.exit, EXIT_FENCED);
        assert_eq!(v.residuals, [RESIDUAL_ANONYMOUS_EGRESS]);
    }
    for other in [Blocked, Unreachable, Error, Skipped] {
        let v = closed(&report(Blocked, other, Blocked, Blocked));
        assert_eq!(v.exit, EXIT_FENCED, "{other:?}");
        assert!(v.residuals.is_empty(), "{other:?}");
    }
}

#[test]
fn verdict_lowered_integrity_getting_out_is_not_fenced() {
    let all_blocked = report(Blocked, Blocked, Blocked, Blocked);
    for field in 0..4 {
        for through in [Connected, Refused] {
            let mut l = [Blocked; 4];
            l[field] = through;
            let v = closed(&lowered(all_blocked, l));
            assert_eq!(v.exit, EXIT_NOT_FENCED, "field {field} {through:?}");
            assert!(v.residuals.is_empty());
        }
    }
}

#[test]
fn verdict_lowered_integrity_not_set_up_is_reported_not_failed() {
    let all_blocked = report(Blocked, Blocked, Blocked, Blocked);
    for field in 0..4 {
        // A token that could not be assumed is not a block: it is named.
        let mut l = [Blocked; 4];
        l[field] = Error;
        let v = closed(&lowered(all_blocked, l));
        assert_eq!(v.exit, EXIT_FENCED, "field {field}");
        assert_eq!(v.residuals, [RESIDUAL_LOWERED_INTEGRITY_UNPROBED]);
        // A connect that was made and did not answer is still unknown.
        l[field] = Unreachable;
        let v = closed(&lowered(all_blocked, l));
        assert_eq!(v.exit, EXIT_INCONCLUSIVE, "field {field}");
        assert!(v.residuals.is_empty());
    }
    // It does not excuse the plain attempt, nor a leak beside it.
    for plain in [Error, Unreachable, Skipped] {
        let r = lowered(report(plain, Blocked, Blocked, Blocked), [Error; 4]);
        assert_eq!(closed(&r).exit, EXIT_INCONCLUSIVE, "{plain:?}");
    }
    let leak = lowered(all_blocked, [Error, Connected, Error, Error]);
    assert_eq!(closed(&leak).exit, EXIT_NOT_FENCED);
    // It is named after the residuals that connected.
    let r = lowered(report(Blocked, Blocked, Blocked, Connected), [Error; 4]);
    assert_eq!(
        closed(&r).residuals,
        [RESIDUAL_DENY_ONLY, RESIDUAL_LOWERED_INTEGRITY_UNPROBED]
    );
    // Only the attempts at the off-loopback target can be skipped.
    let v = closed(&lowered(all_blocked, [Skipped, Blocked, Skipped, Blocked]));
    assert_eq!(v.exit, EXIT_FENCED);
    for loopback in [1, 3] {
        let mut l = [Blocked; 4];
        l[loopback] = Skipped;
        assert_eq!(closed(&lowered(all_blocked, l)).exit, EXIT_INCONCLUSIVE);
    }
}

#[test]
fn verdict_no_token_copy_can_be_assumed_is_fenced_with_each_reported() -> Result<(), Box<dyn std::error::Error>> {
    // A restricted token: every impersonation of a copy is refused.
    let r = lowered(report(Blocked, Blocked, Blocked, Blocked), [Error; 4]);
    let v = closed(&r);
    assert_eq!(v.exit, EXIT_FENCED);
    assert_eq!(v.residuals, [RESIDUAL_LOWERED_INTEGRITY_UNPROBED]);
    // The anonymous and deny-only copies refused as well.
    let r = lowered(report(Blocked, Error, Error, Error), [Error; 4]);
    let v = closed(&r);
    assert_eq!(v.exit, EXIT_FENCED);
    assert_eq!(v.residuals, [RESIDUAL_LOWERED_INTEGRITY_UNPROBED]);
    // Each reaches the broker as `error`, not as a block.
    let r = Report::decode(r.encode()).ok_or("missing value")?;
    assert_eq!(r.plain.label(), "blocked");
    for (o, _) in r.lowered() {
        assert_eq!(o.label(), "error");
    }
    for o in [r.anonymous, r.anonymous_loopback, r.deny_only] {
        assert_eq!(o.label(), "error");
    }
    Ok(())
}

#[test]
fn a_failed_revert_ends_the_run() -> Result<(), Box<dyn std::error::Error>> {
    assert!(ensure_reverted().is_ok());
    REVERT_FAILURE.with(|f| f.set(Some(0x8007_0005_u32 as i32)));
    let e = ensure_reverted().err().ok_or("expected an error")?.to_string();
    REVERT_FAILURE.with(|f| f.set(None));
    assert!(e.contains("RevertToSelf") && e.contains("0x80070005"), "{e}");
    Ok(())
}

#[test]
fn a_plain_attempt_past_the_fence_settles_the_verdict_alone() {
    for through in [Connected, Refused] {
        let r = Report::unfenced_at_plain(through);
        assert_eq!(r.plain, through);
        assert!(r.fields()[1..].iter().all(|o| *o == Skipped));
        let v = closed(&r);
        assert_eq!(v.exit, EXIT_NOT_FENCED);
        assert!(v.residuals.is_empty());
        assert_eq!(Report::decode(r.encode()), Some(r));
    }
}

#[test]
fn verdict_inconclusive_and_skipped() {
    for odd in [Unreachable, Error, Skipped] {
        assert_eq!(closed(&report(odd, Blocked, Blocked, Blocked)).exit, EXIT_INCONCLUSIVE);
    }
}
