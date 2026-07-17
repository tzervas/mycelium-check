//! `mycelium-check` — the project-aware correctness/type-check **driver** (M-365; the `myc-check`
//! prototype grows up).
//!
//! It resolves a `mycelium-proj.toml` project, checks the **whole** `phylum`/program, and aggregates
//! every refusal as a structured diagnostic **routed through the M-362 auto-baseline** (RFC-0013 levels +
//! routes), exiting non-zero on any error so CI can gate on it. It changes nothing about *what* the
//! checker decides — the trusted M-210 checker ([`mycelium_l1::check_nodule`]) is the base (KC-3); this is
//! the *driver* above it: discovery + aggregation + honest routing.
//!
//! Honesty: a `ParseError`/`CheckError` is an **explicit** finding with a site (never a silent pass; G2),
//! and a check refusal is routed at the baseline level/route for the umbrella `NotValidated` class —
//! `CheckError` is a flat `{site, message}`, so the driver does **not** fabricate a finer class
//! (`TypeMismatch` vs `UnresolvedName`) it cannot structurally distinguish (VR-5: report what is known,
//! never invent). Per-op guarantee tags computed by the checker are untouched.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};

use mycelium_l1::ast::Item;
use mycelium_l1::{check_nodule, parse, UsePath};
use mycelium_lsp::{
    derive_baseline, present, ClassRegistry, DiagnosticPolicy, Level, ReasonedError,
};

#[cfg(test)]
mod tests;

/// What kind of refusal a finding records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindingKind {
    /// A syntactic refusal (parse).
    Parse,
    /// A static-check refusal (type/totality/name/validation).
    Check,
}

/// One aggregated diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// The file it occurred in.
    pub file: String,
    /// Parse or check.
    pub kind: FindingKind,
    /// The site within the file (a definition name, or empty for a whole-file parse error).
    pub site: String,
    /// The author-facing message.
    pub message: String,
    /// The baseline presentation level (check refusals; `Minimal` for parse).
    pub level: Level,
    /// The baseline route, if any (check refusals).
    pub route: Option<String>,
}

impl Finding {
    /// Attach a baseline route, fluently (M-644 ergonomics). Additive builder; sets `route`.
    #[must_use]
    pub fn with_route(mut self, route: String) -> Self {
        self.route = Some(route);
        self
    }
}

/// The aggregated result of checking a set of sources.
#[derive(Debug, Clone, Default)]
pub struct Report {
    /// Every finding, deterministically ordered (by file).
    pub findings: Vec<Finding>,
    /// How many files were checked.
    pub files_checked: usize,
}

impl Report {
    /// Push a finding, fluently (M-644 ergonomics). Additive builder; appends to `findings` (does
    /// **not** touch `files_checked` — set that explicitly with [`Report::with_files_checked`]).
    #[must_use]
    pub fn with_finding(mut self, finding: Finding) -> Self {
        self.findings.push(finding);
        self
    }

    /// Set the checked-file count, fluently (M-644 ergonomics). Additive builder.
    #[must_use]
    pub fn with_files_checked(mut self, files_checked: usize) -> Self {
        self.files_checked = files_checked;
        self
    }

    /// Whether the report is clean (no findings).
    #[must_use]
    pub fn is_ok(&self) -> bool {
        self.findings.is_empty()
    }

    /// The CI exit code: 2 if any parse error, else 3 if any check error, else 0.
    #[must_use]
    pub fn exit_code(&self) -> u8 {
        if self.findings.iter().any(|f| f.kind == FindingKind::Parse) {
            2
        } else if self.findings.iter().any(|f| f.kind == FindingKind::Check) {
            3
        } else {
            0
        }
    }
}

/// A project-resolution failure — no/ambiguous input (no sources, an unreadable file). Exit 5.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolveError(pub String);

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "resolution-error: {}", self.0)
    }
}

impl std::error::Error for ResolveError {}

/// Check one source, appending any finding. Check refusals are routed through `policy` (the M-362
/// baseline) for their level/route; parse refusals are syntactic (pre-class) and presented minimally.
pub fn check_source(
    file: &str,
    src: &str,
    policy: &DiagnosticPolicy,
    registry: &ClassRegistry,
    out: &mut Vec<Finding>,
) {
    match parse(src) {
        Err(e) => out.push(Finding {
            file: file.to_owned(),
            kind: FindingKind::Parse,
            site: String::new(),
            message: e.to_string(),
            level: Level::Minimal,
            route: None,
        }),
        Ok(nodule) => {
            if let Err(ce) = check_nodule(&nodule) {
                // Route through the baseline at the umbrella static-check class (honest: the flat
                // CheckError carries no finer class to claim — VR-5).
                let class = registry
                    .resolve("NotValidated")
                    .expect("NotValidated is a builtin class");
                let p = present(
                    ReasonedError::new(class, ce.message.clone(), ce.site.clone()),
                    Some(policy),
                );
                out.push(Finding {
                    file: file.to_owned(),
                    kind: FindingKind::Check,
                    site: ce.site,
                    message: ce.message,
                    level: p.diagnostic.level,
                    route: p.diagnostic.route,
                });
            }
        }
    }
}

/// Check one source under the **default baseline policy** — the M-644 ergonomic convenience for the
/// common case where a caller has no custom `policy`/`registry`. Derives the builtin
/// [`ClassRegistry`] + the [`derive_baseline`] policy (exactly as [`check_sources`] does) and delegates
/// to the 5-arg [`check_source`]. A *new name* (Rust has no overloading; renaming `check_source` would
/// be breaking — M-644 is additive-only). For many sources prefer [`check_sources`], which builds the
/// registry/policy once.
pub fn check_source_default(file: &str, src: &str, out: &mut Vec<Finding>) {
    let registry = ClassRegistry::with_builtins();
    let policy = derive_baseline(&registry);
    check_source(file, src, &policy, &registry, out);
}

/// Check an explicit set of `(path, contents)` sources, aggregating findings deterministically.
#[must_use]
pub fn check_sources(sources: &[(String, String)]) -> Report {
    let registry = ClassRegistry::with_builtins();
    let policy = derive_baseline(&registry);
    let mut findings = Vec::new();
    for (file, src) in sources {
        check_source(file, src, &policy, &registry, &mut findings);
    }
    findings.sort_by(|a, b| a.file.cmp(&b.file));
    Report {
        findings,
        files_checked: sources.len(),
    }
}

/// Resolve and check a whole project: every `.myc` under `dir`.
///
/// # Errors
/// [`ResolveError`] when there are no `.myc` sources, or a source cannot be read (the project input is
/// missing/ambiguous; G2 — never a silent empty pass).
pub fn check_project(dir: &Path) -> Result<Report, ResolveError> {
    let files = collect_myc(dir)?;
    if files.is_empty() {
        return Err(ResolveError(format!(
            "no `.myc` sources under {} — nothing to check (a clean exit here would be a silent pass; G2)",
            dir.display()
        )));
    }
    let mut sources = Vec::with_capacity(files.len());
    for f in files {
        let src = std::fs::read_to_string(&f)
            .map_err(|e| ResolveError(format!("{}: {e}", f.display())))?;
        let rel = f
            .strip_prefix(dir)
            .unwrap_or(&f)
            .to_string_lossy()
            .replace('\\', "/");
        sources.push((rel, src));
    }
    Ok(check_sources(&sources))
}

// ---------------------------------------------------------------------------------------------------
// Phylum-check mode (M-1006) — check a set of `.myc` files as **one cross-resolving phylum**, not as
// isolated phyla-of-one. `check_sources`/`check_project` above run [`check_nodule`] per file, so a
// cross-nodule `use a.Foo;` cannot resolve (each file is a phylum-of-one). This mode assembles the
// files into a single `Phylum` and runs [`mycelium_l1::check_phylum`], the kernel's cross-nodule
// resolver — additive, alongside (never replacing) the per-file modes above.
//
// Honesty (VR-5/G2): the kernel `check_phylum` is **all-or-nothing** — it returns either the whole
// `PhylumEnv` or one `CheckError`; the whole-phylum `PhylumReport::ok`/`error` pair reports that
// faithfully, unchanged. **P-A (DN-124 §2, Accepted 2026-07-12)** additionally gives `PhylumReport` a
// *partial*, per-nodule verdict on `nodules` even when the whole phylum did **not** check clean — a
// **driver-level** import-closure sub-phylum re-check that reuses [`mycelium_l1::check_phylum`]
// **unchanged** (KC-3/DRY: zero kernel growth). A parse failure or a duplicate `nodule` path is still
// refused **before** assembly, and `nodules` stays empty there (identity itself is ambiguous/unknown —
// no partial credit is attempted; conservative, never fabricated).
//
// **The load-bearing soundness invariant (DN-124 §2.1):** a nodule *N* is reported `Clean` **iff**
// (a) *N* itself checks clean **and** (b) every nodule in *N*'s transitive in-batch import closure
// checks clean. Concretely: assemble the sub-phylum `{N} ∪ closure(N)` (via the intra-phylum `use`
// edges) and run the **unmodified** kernel `check_phylum` on just that sub-phylum. A missing or
// failing dependency can only make *N* `CheckError`/`Blocked`, **never** `Clean` (DN-124 §6 Attacks
// 1b/2, held). When the whole phylum *does* check clean, every nodule is trivially `Clean` without
// re-checking (a clean superset entails every closed subset also checks clean — dropping unrelated
// members can only remove candidate coherence conflicts, never introduce one, and every `use` *N*
// resolves within the full phylum resolves identically within `{N} ∪ closure(N)` by construction).
// The guarantee is `Empirical` (real toolchain).

/// What kind of refusal blocked a phylum from checking clean.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhylumErrorKind {
    /// A source failed to parse — the phylum cannot be assembled from an unparseable nodule.
    Parse,
    /// Two nodules declared the same dotted `nodule` path — an ambiguous phylum export table (G2).
    Duplicate,
    /// The assembled phylum failed [`mycelium_l1::check_phylum`] (a cross-nodule type/name refusal).
    Check,
}

impl PhylumErrorKind {
    /// The stable lowercase tag used in the `--json` contract (`"parse"`/`"duplicate"`/`"check"`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            PhylumErrorKind::Parse => "parse",
            PhylumErrorKind::Duplicate => "duplicate",
            PhylumErrorKind::Check => "check",
        }
    }
}

/// The single refusal that blocked a phylum (all-or-nothing — there is at most one).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhylumError {
    /// Which class of refusal.
    pub kind: PhylumErrorKind,
    /// Where it occurred — a file label (parse) or a nodule path (duplicate/check site).
    pub site: String,
    /// The author-facing message.
    pub message: String,
}

/// A per-nodule verdict class (P-A, DN-124 §2.2/§2.3). Never fabricated (G2): built either
/// trivially (the whole phylum checked clean) or by the import-closure sub-phylum re-check — always
/// grounded in a real `check_phylum` run, never guessed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoduleClass {
    /// *N*'s whole transitive import closure checked clean (the DN-124 §2.1 soundness invariant).
    Clean,
    /// *N*'s own closure sub-phylum failed, attributed to *N* itself — either the failure's site was
    /// resolved to *N*'s own nodule path, or the attribution was **not cleanly recoverable** from the
    /// evidence (most check-error sites are bare item names, not nodule-qualified — VR-5: never claim
    /// a finer `Blocked` class than the evidence supports; report the weaker class instead).
    CheckError {
        /// The failure's site, as reported by the kernel checker.
        site: String,
        /// The failure's message, as reported by the kernel checker.
        message: String,
    },
    /// *N*'s own closure sub-phylum failed, and the failure was **confidently** attributed (via a
    /// nodule-qualified site, e.g. an unresolved `<use>`) to a **different** nodule in *N*'s import
    /// closure — named explicitly, never a silent "it's someone's fault" (G2).
    Blocked {
        /// The dotted path of the closure member the failure was attributed to.
        on: String,
        /// The failure's message, as reported by the kernel checker.
        message: String,
    },
}

impl NoduleClass {
    /// The stable lowercase-agnostic label used in the `--json` contract
    /// (`"Clean"`/`"CheckError"`/`"Blocked"`).
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            NoduleClass::Clean => "Clean",
            NoduleClass::CheckError { .. } => "CheckError",
            NoduleClass::Blocked { .. } => "Blocked",
        }
    }

    /// Whether this class credits the checked numerator (only [`NoduleClass::Clean`] does).
    #[must_use]
    pub fn is_clean(&self) -> bool {
        matches!(self, NoduleClass::Clean)
    }
}

/// A per-nodule verdict. As of **P-A (DN-124, Accepted 2026-07-12)** this is populated for **every**
/// nodule of a phylum that was successfully assembled (parsed, no duplicate paths) — not only on a
/// clean phylum (the pre-DN-124 all-or-nothing behavior).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoduleVerdict {
    /// The nodule's dotted path (`a`, `core.binary`).
    pub nodule: String,
    /// The originating source's file label (the `sources`/directory-walk key this nodule was parsed
    /// from) — the join key a consumer needs to credit a specific **emitted file**'s items off a
    /// nodule-keyed verdict (a nodule's dotted path need not equal its file's path/stem). Threaded
    /// through 1:1 by construction: [`check_phylum_sources`] only reaches nodule-verdict computation
    /// after every source has parsed, so `nodules[i]` and `sources[i]` share an index.
    pub file: String,
    /// The verdict class.
    pub class: NoduleClass,
}

/// The result of checking a set of sources **as one phylum**.
#[derive(Debug, Clone)]
pub struct PhylumReport {
    /// Whether the whole phylum checked clean.
    pub ok: bool,
    /// How many sources were assembled into the phylum.
    pub files_checked: usize,
    /// The single blocking refusal, if any (`None` iff `ok`).
    pub error: Option<PhylumError>,
    /// One verdict per nodule (P-A, DN-124 §2.3) — populated whenever the phylum was successfully
    /// **assembled** (every source parsed, no duplicate `nodule` paths); left empty on a `Parse`/
    /// `Duplicate` refusal (nodule identity itself is ambiguous/unknown there — no partial credit is
    /// attempted, VR-5). **Never conflated with `ok`**: `ok`/`error` stay the whole-phylum verdict
    /// unchanged; `nodules` is the additional, strictly-more-informative partial view. A reader can
    /// never mistake "k nodules Clean" for "the phylum builds" — both signals are always present.
    pub nodules: Vec<NoduleVerdict>,
}

impl PhylumReport {
    /// The CI exit code: 0 if clean, 2 for a parse error, 3 for a check/duplicate refusal.
    #[must_use]
    pub fn exit_code(&self) -> u8 {
        match &self.error {
            None => 0,
            Some(e) => match e.kind {
                PhylumErrorKind::Parse => 2,
                PhylumErrorKind::Check | PhylumErrorKind::Duplicate => 3,
            },
        }
    }
}

/// Check an explicit set of `(path, contents)` sources **as one cross-resolving phylum** (the
/// FS-free core, mirroring [`check_sources`]'s shape so it is unit-testable). See the module note
/// above for the honesty contract.
///
/// Steps: parse each source (any parse failure → an explicit `Parse` refusal — the phylum cannot be
/// assembled), guard against duplicate `nodule` paths (a `Duplicate` refusal — never a silent export
/// collision, G2), assemble one `Phylum { path: None, .. }`, and run [`mycelium_l1::check_phylum`]
/// (all-or-nothing: `Clean` verdicts on success, one faithful `Check` refusal on failure).
#[must_use]
pub fn check_phylum_sources(sources: &[(String, String)]) -> PhylumReport {
    let files_checked = sources.len();

    // 1. Parse each source. Any parse failure refuses the whole phylum (never a silent partial).
    // `files` stays index-parallel to `nodules` — see [`NoduleVerdict::file`]'s doc.
    let mut nodules: Vec<mycelium_l1::Nodule> = Vec::with_capacity(files_checked);
    let mut files: Vec<String> = Vec::with_capacity(files_checked);
    for (file, src) in sources {
        match parse(src) {
            Ok(nodule) => {
                nodules.push(nodule);
                files.push(file.clone());
            }
            Err(e) => {
                return PhylumReport {
                    ok: false,
                    files_checked,
                    error: Some(PhylumError {
                        kind: PhylumErrorKind::Parse,
                        site: file.clone(),
                        message: e.to_string(),
                    }),
                    nodules: Vec::new(),
                };
            }
        }
    }

    // 2. Never-silent duplicate-nodule-path guard (mirrors mycelium-cli's
    // `check_no_duplicate_nodule_paths`): `check_phylum` keys its export table by nodule path and
    // would otherwise let a second nodule of the same path collide silently (G2).
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for nodule in &nodules {
        let key = nodule.path.0.join(".");
        if !seen.insert(key.clone()) {
            return PhylumReport {
                ok: false,
                files_checked,
                error: Some(PhylumError {
                    kind: PhylumErrorKind::Duplicate,
                    site: key.clone(),
                    message: format!(
                        "nodule `{key}` is declared more than once — every nodule path in a phylum \
                         must be unique"
                    ),
                }),
                nodules: Vec::new(),
            };
        }
    }

    // 3. Assemble one header-less phylum and check it as a whole.
    let phylum = mycelium_l1::Phylum {
        path: None,
        nodules,
    };
    match mycelium_l1::check_phylum(&phylum) {
        Ok(_) => {
            // A clean whole phylum trivially credits every nodule `Clean` — no re-check needed (see
            // the module note above for why this is sound, not just an optimization).
            let verdicts = phylum
                .nodules
                .iter()
                .zip(files.iter())
                .map(|(n, file)| NoduleVerdict {
                    nodule: n.path.0.join("."),
                    file: file.clone(),
                    class: NoduleClass::Clean,
                })
                .collect();
            PhylumReport {
                ok: true,
                files_checked,
                error: None,
                nodules: verdicts,
            }
        }
        Err(ce) => {
            // P-A (DN-124 §2.2): the whole-phylum verdict stays exactly the faithful all-or-nothing
            // `PhylumError` it always was — but `nodules` is now populated via the driver-level
            // import-closure sub-phylum re-check, never fabricated.
            let partial = compute_partial_verdicts(&phylum.nodules, &files);
            PhylumReport {
                ok: false,
                files_checked,
                error: Some(PhylumError {
                    kind: PhylumErrorKind::Check,
                    site: ce.site,
                    message: ce.message,
                }),
                nodules: partial,
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------------
// P-A mechanism (DN-124 §2.2) — the driver-level import-closure sub-phylum re-check.

/// Build the intra-phylum `use` **edge relation**: for each nodule (keyed by its dotted path), the
/// dotted paths of the OTHER in-batch nodules its `use`s target. Mirrors exactly the target-path
/// arithmetic [`mycelium_l1`]'s own `resolve_imports` uses (a specific `use a.b.Item` targets nodule
/// `a.b`; a glob `use a.b.*` targets nodule `a.b`) — so an edge here means "this is the nodule
/// `check_phylum`'s own resolver would look up for this `use`", not a guess.
///
/// Cross-phylum `use dep::…` references ([`UsePath::phylum`] `Some`) contribute **no** edge (DN-113/
/// M-1060, DN-124 OQ-2 — out of scope for the driver-level MVP; a phylum whose only uses are
/// cross-phylum degrades to each nodule's closure being itself, the conservative default: it cannot
/// resolve such a use locally either way, so this can only under-credit, never over-credit).
fn build_use_edges(nodules: &[mycelium_l1::Nodule]) -> BTreeMap<String, Vec<String>> {
    let mut edges = BTreeMap::new();
    for n in nodules {
        let mut targets = Vec::new();
        for item in &n.items {
            if let Item::Use(UsePath {
                phylum: None,
                path,
                glob,
            }) = item
            {
                let target = if *glob {
                    path.0.join(".")
                } else if path.0.len() > 1 {
                    path.0[..path.0.len() - 1].join(".")
                } else {
                    // A single-segment specific `use X` names no nodule (empty prefix) — malformed;
                    // `resolve_imports` itself refuses this at check time. No edge to add here.
                    continue;
                };
                targets.push(target);
            }
        }
        edges.insert(n.path.0.join("."), targets);
    }
    edges
}

/// *N*'s transitive in-batch import closure, `{N} ∪ closure(N)` (DN-124 §2.1/§2.2) — a BFS over
/// `edges`, restricted to nodule paths actually **present** in this batch. A `use` target not present
/// in the batch contributes no edge: it is not a false-clean risk (DN-124 §6 Attack 2) — it simply
/// leaves that `use` unresolved when the sub-phylum is later checked, a conservative under-credit.
fn closure_of(
    start: &str,
    edges: &BTreeMap<String, Vec<String>>,
    present: &BTreeSet<String>,
) -> BTreeSet<String> {
    let mut visited: BTreeSet<String> = BTreeSet::new();
    let mut queue: VecDeque<String> = VecDeque::new();
    queue.push_back(start.to_owned());
    while let Some(cur) = queue.pop_front() {
        if !visited.insert(cur.clone()) {
            continue;
        }
        if let Some(targets) = edges.get(&cur) {
            for t in targets {
                if present.contains(t) && !visited.contains(t) {
                    queue.push_back(t.clone());
                }
            }
        }
    }
    visited
}

/// Attribute a [`mycelium_l1::CheckError`]'s `site` to the closure member it belongs to, when
/// **cleanly recoverable**: most check-error sites are bare item names (a fn/type/trait name, never
/// nodule-qualified — see `check_fn_body`/`register_nodule_decls` in `mycelium-l1`), so this can only
/// confidently attribute the small set of sites the kernel *does* nodule-qualify (`<use>`/
/// `<totality>`, via its own `qualify(&nodule.path, …)`). Picks the **longest** matching nodule-path
/// prefix (the most specific enclosing nodule, for a batch with nested paths like `a` and `a.b`).
/// Returns `None` when no closure member's path is a prefix of `site` — the honest "not cleanly
/// recoverable" case (VR-5: [`compute_partial_verdicts`] falls back to the weaker `CheckError` there,
/// never guesses `Blocked`).
fn site_owner(site: &str, closure_paths: &BTreeSet<String>) -> Option<String> {
    closure_paths
        .iter()
        .filter(|p| !p.is_empty() && (site == p.as_str() || site.starts_with(&format!("{p}."))))
        .max_by_key(|p| p.len())
        .cloned()
}

/// P-A (DN-124 §2.2): sound partial per-nodule verdicts via a driver-level import-closure sub-phylum
/// re-check that reuses [`mycelium_l1::check_phylum`] **unchanged** (KC-3/DRY — zero kernel growth).
/// Called only when the whole phylum did **not** check clean (see the call site for why the clean
/// case needs no re-check). `files` is index-parallel to `nodules` (see [`NoduleVerdict::file`]).
fn compute_partial_verdicts(
    nodules: &[mycelium_l1::Nodule],
    files: &[String],
) -> Vec<NoduleVerdict> {
    let edges = build_use_edges(nodules);
    let present: BTreeSet<String> = nodules.iter().map(|n| n.path.0.join(".")).collect();

    nodules
        .iter()
        .zip(files.iter())
        .map(|(n, file)| {
            let n_path = n.path.0.join(".");
            let closure = closure_of(&n_path, &edges, &present);
            let sub_nodules: Vec<mycelium_l1::Nodule> = nodules
                .iter()
                .filter(|m| closure.contains(&m.path.0.join(".")))
                .cloned()
                .collect();
            let sub_phylum = mycelium_l1::Phylum {
                path: None,
                nodules: sub_nodules,
            };
            let class = match mycelium_l1::check_phylum(&sub_phylum) {
                Ok(_) => NoduleClass::Clean,
                Err(ce) => match site_owner(&ce.site, &closure) {
                    Some(owner) if owner == n_path => NoduleClass::CheckError {
                        site: ce.site,
                        message: ce.message,
                    },
                    Some(owner) => NoduleClass::Blocked {
                        on: owner,
                        message: ce.message,
                    },
                    None => NoduleClass::CheckError {
                        site: ce.site,
                        message: ce.message,
                    },
                },
            };
            NoduleVerdict {
                nodule: n_path,
                file: file.clone(),
                class,
            }
        })
        .collect()
}

/// Resolve and check every `.myc` under `dir` **as one phylum** (the FS wrapper over
/// [`check_phylum_sources`], mirroring [`check_project`]'s discovery/read path).
///
/// # Errors
/// [`ResolveError`] when there are no `.myc` sources, or a source cannot be read (missing/ambiguous
/// input; G2 — never a silent empty pass).
pub fn check_phylum_dir(dir: &Path) -> Result<PhylumReport, ResolveError> {
    let files = collect_myc(dir)?;
    if files.is_empty() {
        return Err(ResolveError(format!(
            "no `.myc` sources under {} — nothing to check (a clean exit here would be a silent pass; G2)",
            dir.display()
        )));
    }
    let mut sources = Vec::with_capacity(files.len());
    for f in files {
        let src = std::fs::read_to_string(&f)
            .map_err(|e| ResolveError(format!("{}: {e}", f.display())))?;
        let rel = f
            .strip_prefix(dir)
            .unwrap_or(&f)
            .to_string_lossy()
            .replace('\\', "/");
        sources.push((rel, src));
    }
    Ok(check_phylum_sources(&sources))
}

/// Collect every `.myc` under `dir` (recursively), sorted; skipping hidden entries and `target/`.
fn collect_myc(dir: &Path) -> Result<Vec<PathBuf>, ResolveError> {
    let mut out = Vec::new();
    walk(dir, &mut out)?;
    out.sort();
    Ok(out)
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), ResolveError> {
    let entries =
        std::fs::read_dir(dir).map_err(|e| ResolveError(format!("{}: {e}", dir.display())))?;
    let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort();
    for path in paths {
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        if name.starts_with('.') || name == "target" {
            continue;
        }
        if path.is_dir() {
            walk(&path, out)?;
        } else if path.extension().is_some_and(|x| x == "myc") {
            out.push(path);
        }
    }
    Ok(())
}
