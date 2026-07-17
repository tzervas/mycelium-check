//! Tests for `mycelium-check` — extracted from `lib.rs` (as-touched, M-797 retrofit discipline) when
//! DN-124/M-1079 (partial per-nodule `PhylumReport` verdicts, P-A) landed. White-box access to
//! private items via `use crate::*;`.
//!
//! Two families: the pre-existing per-file/whole-phylum coverage (unchanged behavior, migrated
//! verbatim except for the `NoduleClass` shape), and the new DN-124 §5.3 Unit-1 property tests —
//! soundness, never-false-clean, monotonicity, never-fabricate — plus the adversarial fixture from
//! DN-124 §6 Attack 1b (a nodule blocked by a genuinely-failing sibling, never credited `Clean`).

use crate::*;

#[test]
fn a_clean_program_checks_ok() {
    let r = check_sources(&[(
        "ok.myc".to_owned(),
        "nodule d;\nfn f(x: Binary{8}) => Binary{8} = x;\n".to_owned(),
    )]);
    assert!(r.is_ok(), "{:?}", r.findings);
    assert_eq!(r.exit_code(), 0);
}

#[test]
fn a_parse_error_is_an_explicit_finding_exit_2() {
    let r = check_sources(&[(
        "bad.myc".to_owned(),
        "nodule d\nfn f(x: Binary{8}) => Ternary{6} = swap(x, to: Ternary{6})".to_owned(),
    )]);
    assert_eq!(r.findings.len(), 1);
    assert_eq!(r.findings[0].kind, FindingKind::Parse);
    assert_eq!(r.exit_code(), 2);
}

#[test]
fn a_check_error_is_routed_through_the_baseline_exit_3() {
    // An undefined name is a check refusal (UnresolvedName-class), routed at the baseline level.
    let r = check_sources(&[(
        "c.myc".to_owned(),
        "nodule d;\nfn f() => Binary{8} = nope(0b0);\n".to_owned(),
    )]);
    assert_eq!(r.exit_code(), 3, "{:?}", r.findings);
    let c = r
        .findings
        .iter()
        .find(|f| f.kind == FindingKind::Check)
        .expect("a check finding");
    // The M-362 baseline routes static-check refusals to the diagnostic stream at medium detail.
    assert_eq!(c.level, Level::Medium, "{c:?}");
    assert_eq!(c.route.as_deref(), Some("stream"), "{c:?}");
}

#[test]
fn aggregation_is_deterministic_and_reports_all_files() {
    let r = check_sources(&[
        (
            "b.myc".to_owned(),
            "nodule d;\nfn f() => Binary{8} = nope(0b0);\n".to_owned(),
        ),
        (
            "a.myc".to_owned(),
            "nodule d;\nfn g() => Binary{8} = also_nope(0b0);\n".to_owned(),
        ),
    ]);
    // Both files reported, sorted by name (a before b).
    assert_eq!(r.findings.len(), 2);
    assert_eq!(r.findings[0].file, "a.myc");
    assert_eq!(r.findings[1].file, "b.myc");
    assert_eq!(r.exit_code(), 3);
}

#[test]
fn check_source_default_and_builders_are_additive_ergonomics() {
    // M-644: the default-policy convenience checks one source via the same baseline path as
    // check_sources (builds the builtin registry + derived policy, delegates to check_source).
    let mut out = Vec::new();
    check_source_default(
        "a.myc",
        "nodule d;\nfn g() => Binary{8} = also_nope(0b0);\n",
        &mut out,
    );
    assert!(
        !out.is_empty(),
        "an unresolved call is a recorded check finding"
    );
    // The fluent builders compose a Report additively (no canonical constructor changed).
    let f = out.remove(0).with_route("escalate".to_owned());
    assert_eq!(f.route.as_deref(), Some("escalate"));
    let r = Report::default().with_finding(f).with_files_checked(1);
    assert_eq!(r.findings.len(), 1);
    assert_eq!(r.files_checked, 1);
}

// --- Phylum-check mode (M-1006) -----------------------------------------------------------------

#[test]
fn phylum_cross_nodule_reference_resolves() {
    // Nodule `a` exports `helper` (`pub fn`); nodule `b` imports it (`use a.*`) and calls it. As
    // one phylum this resolves (RFC-0006 §4.3, mirrors the l1 `cross_nodule_program_runs_three_way`).
    let a = (
        "a.myc".to_owned(),
        "nodule a;\npub fn helper(x: Binary{8}) => Binary{8} = not(x);\n".to_owned(),
    );
    let b = (
        "b.myc".to_owned(),
        "nodule b;\nuse a.*;\nfn g(x: Binary{8}) => Binary{8} = helper(x);\n".to_owned(),
    );
    let report = check_phylum_sources(&[a, b.clone()]);
    assert!(
        report.ok,
        "phylum should resolve `a.helper`: {:?}",
        report.error
    );
    assert_eq!(report.exit_code(), 0);
    assert_eq!(report.nodules.len(), 2);
    // Monotonicity (DN-124 §5.3 Unit-1 property iii): a wholly-clean phylum yields all-`Clean`.
    assert!(report.nodules.iter().all(|v| v.class.is_clean()));

    // Witness that the phylum path is what makes it resolve: the SAME `b.myc` checked in
    // isolation (a phylum-of-one, the per-file path) FAILS — `a.helper` is unresolved there.
    let isolated = check_sources(&[b]);
    assert!(
        !isolated.is_ok(),
        "b.myc must NOT resolve `a.*` in isolation (proves the phylum lever): {:?}",
        isolated.findings
    );
    assert_eq!(isolated.exit_code(), 3);
}

#[test]
fn phylum_duplicate_nodule_path_is_refused() {
    // Two nodules both declare `nodule a;` — an ambiguous export table, refused never-silently (G2)
    // BEFORE reaching check_phylum. Nodule identity itself is ambiguous here, so no partial credit
    // is attempted: `nodules` stays empty (never fabricated).
    let report = check_phylum_sources(&[
        (
            "x.myc".to_owned(),
            "nodule a;\npub fn helper(x: Binary{8}) => Binary{8} = not(x);\n".to_owned(),
        ),
        (
            "y.myc".to_owned(),
            "nodule a;\npub fn other(x: Binary{8}) => Binary{8} = x;\n".to_owned(),
        ),
    ]);
    assert!(!report.ok);
    assert_eq!(
        report.error.as_ref().map(|e| e.kind),
        Some(PhylumErrorKind::Duplicate),
        "{:?}",
        report.error
    );
    assert_eq!(report.exit_code(), 3);
    assert!(
        report.nodules.is_empty(),
        "a Duplicate refusal has ambiguous nodule identity — never a fabricated partial verdict"
    );
}

#[test]
fn phylum_parse_error_is_reported() {
    // Missing `;` after the nodule header — an unparseable nodule; the phylum cannot be assembled,
    // so `nodules` stays empty (identity of the unparsed nodule is unknown — never fabricated).
    let report = check_phylum_sources(&[(
        "bad.myc".to_owned(),
        "nodule a\npub fn helper(x: Binary{8}) => Binary{8} = not(x);\n".to_owned(),
    )]);
    assert!(!report.ok);
    let e = report.error.as_ref().expect("a parse refusal");
    assert_eq!(e.kind, PhylumErrorKind::Parse);
    assert_eq!(e.site, "bad.myc", "parse site is the file label");
    assert_eq!(report.exit_code(), 2);
    assert!(report.nodules.is_empty());
}

#[test]
fn phylum_check_error_is_reported() {
    // A single nodule with a real check error (an unresolved call) — check_phylum refuses. As of
    // P-A, `nodules` is now populated: the sole nodule's own closure sub-phylum (itself alone)
    // re-checked and attributed to itself (bare-name site — not cleanly recoverable to a nodule, so
    // the weaker `CheckError`, never a guessed `Blocked`).
    let report = check_phylum_sources(&[(
        "c.myc".to_owned(),
        "nodule a;\nfn f() => Binary{8} = nope(0b0);\n".to_owned(),
    )]);
    assert!(!report.ok);
    assert_eq!(
        report.error.as_ref().map(|e| e.kind),
        Some(PhylumErrorKind::Check),
        "{:?}",
        report.error
    );
    assert_eq!(report.exit_code(), 3);
    assert_eq!(report.nodules.len(), 1);
    assert_eq!(report.nodules[0].nodule, "a");
    assert!(
        matches!(report.nodules[0].class, NoduleClass::CheckError { .. }),
        "{:?}",
        report.nodules[0].class
    );
}

// --- P-A partial per-nodule verdicts (DN-124 §5.3 Unit 1) --------------------------------------

/// **Never-false-clean / soundness** (DN-124 §2.1, §5.3 Unit-1 properties i+ii; the adversarial
/// fixture from §6 Attack 1b). Three nodules: `a` has a genuinely unresolved `use` (fails on its
/// OWN site, never resolves) — `b` imports `a`'s public surface and is transitively blocked by it —
/// `c` is wholly unrelated and independently clean. The whole phylum is NOT ok (an unresolved use is
/// a real refusal), but the partial verdicts must show: `a` is `CheckError` (local — the failing
/// `<use>` site is its own), `b` is `Blocked{on: "a"}` (never `Clean` — it depends on the failed
/// `a`), and `c` is `Clean` (its closure is just itself, untouched by `a`/`b`'s failure).
#[test]
fn partial_verdict_blocks_a_dependent_never_credits_it_clean() {
    let a = (
        "a.myc".to_owned(),
        // `a` itself has an unresolved use — `zzz` names no nodule in this batch (a genuine,
        // never-recoverable refusal, not a missing-from-batch false-FAIL corner case).
        "nodule a;\nuse zzz.nonexistent;\npub fn helper(x: Binary{8}) => Binary{8} = not(x);\n"
            .to_owned(),
    );
    let b = (
        "b.myc".to_owned(),
        "nodule b;\nuse a.*;\nfn g(x: Binary{8}) => Binary{8} = helper(x);\n".to_owned(),
    );
    let c = (
        "c.myc".to_owned(),
        "nodule c;\nfn h(x: Binary{8}) => Binary{8} = x;\n".to_owned(),
    );
    let report = check_phylum_sources(&[a, b, c]);
    assert!(
        !report.ok,
        "the whole phylum is NOT clean — `a`'s use never resolves"
    );
    assert_eq!(report.nodules.len(), 3, "{:?}", report.nodules);

    let by_name: std::collections::BTreeMap<&str, &NoduleClass> = report
        .nodules
        .iter()
        .map(|v| (v.nodule.as_str(), &v.class))
        .collect();

    // `a`'s own use fails at its own site — attributed to itself, never `Blocked` on a phantom.
    match by_name["a"] {
        NoduleClass::CheckError { site, .. } => assert_eq!(site, "a.<use>"),
        other => panic!("expected a to be CheckError (local), got {other:?}"),
    }
    // `b` depends on `a` (which fails) — NEVER credited `Clean` (the load-bearing soundness
    // invariant), and confidently attributed to `a` (a nodule-qualified `<use>` site).
    match by_name["b"] {
        NoduleClass::Blocked { on, .. } => assert_eq!(on, "a"),
        other => panic!("expected b to be Blocked on a, got {other:?} — false-clean risk!"),
    }
    // `c` is wholly unrelated (no `use` of `a`/`b`) — independently `Clean`, never dragged down by
    // a failure elsewhere in the phylum (the partial-credit lever DN-124 exists to unblock).
    assert!(
        by_name["c"].is_clean(),
        "c has no dependency on the failed a/b — must still be credited Clean: {:?}",
        by_name["c"]
    );
}

/// **Never-fabricate** (DN-124 §5.3 Unit-1 property iv): a `Blocked` nodule's items must never be
/// counted toward a checked/clean numerator by any consumer keying off `NoduleClass::is_clean()` —
/// this is the exact predicate the transpiler vet loop (Unit 2) will gate on.
#[test]
fn blocked_and_check_error_are_never_is_clean() {
    let a = (
        "a.myc".to_owned(),
        "nodule a;\nuse zzz.nonexistent;\npub fn helper(x: Binary{8}) => Binary{8} = not(x);\n"
            .to_owned(),
    );
    let b = (
        "b.myc".to_owned(),
        "nodule b;\nuse a.*;\nfn g(x: Binary{8}) => Binary{8} = helper(x);\n".to_owned(),
    );
    let report = check_phylum_sources(&[a, b]);
    assert!(!report.ok);
    for v in &report.nodules {
        assert!(
            !v.class.is_clean(),
            "no nodule in this fixture should ever be Clean: {:?}",
            v
        );
    }
}

/// `NoduleVerdict::file` correlates each verdict back to its originating source label — the join key
/// a consumer (the transpiler vet loop, Unit 2) needs to credit a specific emitted file's items off
/// a nodule-keyed verdict (a nodule's dotted path need not equal its file's path/stem).
#[test]
fn nodule_verdict_carries_its_originating_file() {
    let a = (
        "out/a_file.myc".to_owned(),
        "nodule a;\npub fn helper(x: Binary{8}) => Binary{8} = not(x);\n".to_owned(),
    );
    let b = (
        "out/b_file.myc".to_owned(),
        "nodule b;\nuse a.*;\nfn g(x: Binary{8}) => Binary{8} = helper(x);\n".to_owned(),
    );
    // Clean-phylum path.
    let report = check_phylum_sources(&[a.clone(), b.clone()]);
    assert!(report.ok, "{:?}", report.error);
    let by_nodule: std::collections::BTreeMap<&str, &str> = report
        .nodules
        .iter()
        .map(|v| (v.nodule.as_str(), v.file.as_str()))
        .collect();
    assert_eq!(by_nodule["a"], "out/a_file.myc");
    assert_eq!(by_nodule["b"], "out/b_file.myc");

    // Partial (failing) path — the file correlation must hold there too.
    let broken = (
        "out/broken_file.myc".to_owned(),
        "nodule a;\nfn f() => Binary{8} = nope(0b0);\n".to_owned(),
    );
    let report2 = check_phylum_sources(&[broken]);
    assert!(!report2.ok);
    assert_eq!(report2.nodules.len(), 1);
    assert_eq!(report2.nodules[0].file, "out/broken_file.myc");
}

/// A batch nodule that is genuinely independent (no `use` of any failing sibling) is `Clean` even
/// though ITS OWN un-vetted single-file/oracle-mode check would have been fine anyway — the
/// interesting case is the mixed-phylum credit, covered above; this fixture is the minimal two-file
/// witness (the DN-124 §2.4 "partial verdicts are a precondition, not polish" finding: a phylum with
/// one gapped nodule must still credit its clean nodule, never regress to zero).
#[test]
fn a_mixed_phylum_credits_its_clean_nodule_even_though_the_whole_phylum_fails() {
    let broken = (
        "broken.myc".to_owned(),
        "nodule broken;\nfn f() => Binary{8} = nope(0b0);\n".to_owned(),
    );
    let clean = (
        "clean.myc".to_owned(),
        "nodule clean;\nfn g(x: Binary{8}) => Binary{8} = x;\n".to_owned(),
    );
    let report = check_phylum_sources(&[broken, clean]);
    assert!(!report.ok);
    let by_name: std::collections::BTreeMap<&str, &NoduleClass> = report
        .nodules
        .iter()
        .map(|v| (v.nodule.as_str(), &v.class))
        .collect();
    assert!(!by_name["broken"].is_clean());
    assert!(
        by_name["clean"].is_clean(),
        "an all-or-nothing report would have credited ZERO nodules here — the P-A regression this \
         guards against (DN-124 §2.4): {:?}",
        by_name["clean"]
    );
}
