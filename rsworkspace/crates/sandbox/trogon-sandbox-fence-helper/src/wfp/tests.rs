use super::*;

/// The SDDL template used by `install_filters` must parse for a
/// representative SID. Catches template typos without needing a
/// live WFP engine.
#[test]
fn sddl_template_parses() -> Result<(), Box<dyn std::error::Error>> {
    let sd = OwnedSd::from_sddl(&sddl_sandbox_user("S-1-5-32-545"))?;
    assert!(!sd.ptr.0.is_null());
    assert!(sd.len > 0);
    Ok(())
}

#[test]
fn sandbox_user_sddl_carries_both_package_aces() {
    let s = sddl_sandbox_user("S-1-5-21-1-2-3-1005");
    assert!(s.contains("(A;;CC;;;S-1-5-21-1-2-3-1005)"));
    assert!(s.contains("(A;;CC;;;S-1-15-2-1)"));
    assert!(s.contains("(A;;CC;;;S-1-15-2-2)"));
    // ALLOW only: a DENY ACE in a match descriptor would invert
    // which tokens the filter applies to.
    assert!(!s.contains("(D;"));
}

fn tag(kind: &str) -> FilterTag {
    FilterTag::user(kind, "S-1-5-21-1-2-3-1005", None)
}

/// The closed set, plus the open-loopback PERMIT on `open_layers`.
fn enumerated(open_layers: &[&'static str]) -> Vec<(&'static str, FilterTag)> {
    let mut set: Vec<_> = ALE_LAYERS
        .iter()
        .flat_map(|(_, layer)| [KIND_PERMIT_PROXY, KIND_BLOCK_USER].map(|k| (*layer, tag(k))))
        .collect();
    for layer in open_layers {
        set.push((*layer, tag(KIND_PERMIT_OPEN)));
    }
    set
}

#[test]
fn filter_list_with_loopback_closed_is_the_two_filter_set() {
    let specs = filter_specs((60080, 60089), false);
    assert_eq!(
        specs.iter().map(|s| s.kind).collect::<Vec<_>>(),
        ["permit-loopback-user", "block-user"]
    );
    assert_eq!(specs[0].shape, FilterShape::PermitLoopback((60080, 60089)));
    assert_eq!(specs[0].port_range(), Some((60080, 60089)));
    assert_eq!(specs[1].shape, FilterShape::BlockUser);
    assert_eq!(specs[1].port_range(), None);
    assert_eq!(filter_count(false), 4);
}

#[test]
fn filter_list_with_loopback_open_adds_exactly_one_permit() {
    let closed = filter_specs((60080, 60089), false);
    let open = filter_specs((60080, 60089), true);
    assert_eq!(
        open.iter().map(|s| s.kind).collect::<Vec<_>>(),
        ["permit-loopback-user", "permit-loopback-open", "block-user"]
    );
    assert_eq!(filter_count(true), 6);
    let added = open[1];
    assert_eq!(added.shape, FilterShape::PermitLoopbackOpen);
    assert_eq!(added.weight, W_LOOPBACK);
    assert!(added.weight > W_BLOCK);
    let rest: Vec<FilterSpec> = open.iter().copied().filter(|s| s.kind != KIND_PERMIT_OPEN).collect();
    assert_eq!(rest, closed);
    for s in &open {
        assert_eq!(
            s.weight == W_LOOPBACK,
            matches!(
                s.shape,
                FilterShape::PermitLoopback(_) | FilterShape::PermitLoopbackOpen
            )
        );
    }
}

#[test]
fn the_open_permit_leaves_out_only_the_probe_port_and_port_zero() {
    const { assert!(LOOPBACK_PROBE_PORT == 1) };
    let (lo, hi) = OPEN_LOOPBACK_PORTS;
    let open = |p: u16| (lo..=hi).contains(&p);
    assert!(!open(0) && !open(LOOPBACK_PROBE_PORT));
    assert!((0..=u16::MAX).filter(|p| !open(*p)).count() == 2);
    assert!(open(LOOPBACK_PROBE_PORT + 1) && open(u16::MAX));
    // The proxy PERMIT must not open it either: see `install`.
    let (proxy_lo, proxy_hi) = DEFAULT_PROXY_PORT_RANGE;
    assert!(!(proxy_lo..=proxy_hi).contains(&LOOPBACK_PROBE_PORT));
}

#[test]
fn loopback_is_open_only_with_the_permit_on_every_layer() {
    let (v4, v6) = (ALE_LAYERS[0].1, ALE_LAYERS[1].1);
    assert!(!tags_open_loopback(&enumerated(&[])));
    assert!(tags_open_loopback(&enumerated(&[v4, v6])));
    // One layer alone would leave the other closed.
    assert!(!tags_open_loopback(&enumerated(&[v4])));
    assert!(!tags_open_loopback(&enumerated(&[v6])));
    assert!(!tags_open_loopback(&[]));
}

#[test]
fn recorded_loopback_mode_is_reported_and_compared() -> Result<(), Box<dyn std::error::Error>> {
    let with = |state, open| WfpStatus {
        allow_loopback: open,
        ..status(state)
    };
    for open in [false, true] {
        // Not elevated: the record is all there is to report.
        let st = apply_recorded_loopback(with("cannot-read", false), Some(open));
        assert_eq!((st.state, st.allow_loopback), ("cannot-read", open));
        assert_eq!(st.loopback_notice.is_some(), open);
        // Filters and record agree.
        let st = apply_recorded_loopback(with("installed", open), Some(open));
        assert_eq!((st.state, st.allow_loopback), ("installed", open));
        assert_eq!(st.loopback_notice.is_some(), open);
        // They differ: the filters' mode is reported, and the state says so.
        let st = apply_recorded_loopback(with("installed", open), Some(!open));
        assert_eq!(st.state, "loopback-mismatch", "filters {open}");
        assert_eq!(st.allow_loopback, open);
        assert!(st.hint.ok_or("missing value")?.contains("fence-helper install --force"));
    }
    // No record is how a closed set from an earlier build reads.
    let st = apply_recorded_loopback(with("installed", false), None);
    assert_eq!(st.state, "installed");
    let st = apply_recorded_loopback(with("installed", true), None);
    assert_eq!(st.state, "loopback-mismatch");
    let st = apply_recorded_loopback(with("cannot-read", false), None);
    assert!(!st.allow_loopback);
    // Nothing installed: nothing to compare.
    let st = apply_recorded_loopback(with("absent", false), Some(true));
    assert_eq!((st.state, st.allow_loopback), ("absent", false));
    Ok(())
}

#[test]
fn loopback_hints_name_the_install_command() {
    let h = loopback_mismatch_hint(false, true);
    assert!(h.contains("installed: closed"));
    assert!(h.contains("configured: open"));
    assert!(h.contains("fence-helper install --allow-loopback --force"));
    let h = loopback_mismatch_hint(true, false);
    assert!(h.contains("installed: open"));
    assert!(h.contains("configured: closed"));
    assert!(h.contains("fence-helper install --force"));
    assert!(loopback_closed_hint().contains("fence-helper install --allow-loopback"));
}

#[test]
fn open_loopback_notice_is_plain() {
    let n = open_loopback_notice();
    assert!(n.starts_with("loopback is open"));
    assert!(n.contains("every TCP and UDP port"));
    assert!(n.contains(&format!("port {LOOPBACK_PROBE_PORT} alone stays blocked")));
    assert!(n.contains("forwarder") && n.contains("off the machine"));
}

fn status(state: &'static str) -> WfpStatus {
    WfpStatus {
        state,
        filters: 0,
        port_range: None,
        allow_loopback: false,
        loopback_notice: None,
        user_sid: None,
        hint: None,
    }
}

#[test]
fn sddl_rejects_garbage() {
    assert!(OwnedSd::from_sddl("O:LSG:LSD:(A;;CC;;;NOT-A-SID)").is_err());
}

#[test]
fn filter_tag_round_trip() -> Result<(), Box<dyn std::error::Error>> {
    let sid = "S-1-5-21-1-2-3-1005";
    let bk = FilterTag::user("block-user", sid, None);
    assert_eq!(bk.user_sid.as_deref(), Some(sid));
    assert_eq!(bk.port_range, None);
    let lb = FilterTag::user("permit-loopback-user", sid, Some((60080, 60089)));
    assert_eq!(lb.port_range, Some([60080, 60089]));
    // Round-trips through JSON.
    let back: FilterTag = serde_json::from_slice(&lb.to_blob_bytes()?)?;
    assert_eq!(back, lb);
    Ok(())
}

#[test]
fn filter_tag_parses_legacy() -> Result<(), Box<dyn std::error::Error>> {
    // A pre-port-range / pre-user-sid tag (the legacy
    // discriminator-group shape) must still parse so
    // uninstall/install can clean it up.
    let legacy = br#"{"tool":"fence-helper","kind":"permit-loopback"}"#;
    let t: FilterTag = serde_json::from_slice(legacy)?;
    assert_eq!(t.kind, "permit-loopback");
    assert_eq!(t.port_range, None);
    assert_eq!(t.user_sid, None);
    Ok(())
}

/// const-assert the weight invariant directly so a reshuffle in
/// `install_filters` fails to compile here.
#[test]
fn weight_invariant() {
    const { assert!(W_LOOPBACK > W_BLOCK) };
}

#[test]
fn parse_port_range_ok() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(parse_port_range("60080-60089")?, (60080, 60089));
    assert_eq!(parse_port_range(" 1 - 1 ")?, (1, 1));
    assert_eq!(parse_port_range("1-65")?, (1, 1 + MAX_PROXY_PORT_RANGE_WIDTH));
    Ok(())
}

#[test]
fn parse_port_range_rejects() {
    assert!(parse_port_range("60089-60080").is_err()); // low>high
    assert!(parse_port_range("1-1000").is_err()); // too wide
    assert!(parse_port_range("60080").is_err()); // no dash
    assert!(parse_port_range("a-b").is_err()); // not u16
    assert!(parse_port_range("0-65536").is_err()); // overflow
    assert!(parse_port_range("0-9").is_err()); // port 0
}

#[test]
fn parse_guid_accepts_both_forms() -> Result<(), Box<dyn std::error::Error>> {
    let g1 = parse_guid("2c5d0ad6-5f3b-4d4e-9b8f-1a3e7c9d0b21")?;
    let g2 = parse_guid("{2c5d0ad6-5f3b-4d4e-9b8f-1a3e7c9d0b21}")?;
    assert_eq!(g1, g2);
    assert_eq!(g1, DEFAULT_SUBLAYER_GUID);
    Ok(())
}

#[test]
fn parse_guid_rejects_garbage() {
    assert!(parse_guid("not-a-guid").is_err());
}

#[test]
fn an_unparseable_filter_tag_is_an_error() {
    assert!(parse_filter_tag(b"not json").is_err());
    assert!(parse_filter_tag(b"{}").is_err());
}

#[test]
fn a_well_formed_tag_is_kept_only_for_this_tool() -> Result<(), Box<dyn std::error::Error>> {
    let ours = tag(KIND_BLOCK_USER).to_blob_bytes()?;
    assert!(parse_filter_tag(&ours)?.is_some());
    let mut foreign = tag(KIND_BLOCK_USER);
    foreign.tool = "other".to_owned();
    assert!(parse_filter_tag(&foreign.to_blob_bytes()?)?.is_none());
    Ok(())
}
