use super::*;

const CORE_PROFILES: &[&str] = &["full"];
const FULL_PROFILE: &[&str] = &["full"];
const STORAGE_PROFILE: &[&str] = &["storage"];
const INGEST_PROFILE: &[&str] = &["isolated"];
const AUTH_PROFILE: &[&str] = &["auth"];
const AGENT_PROFILE: &[&str] = &["agent"];
const SECURITY_PROFILE: &[&str] = &["security"];
const MCP_PROFILE: &[&str] = &["mcp"];
const UNIX_PLATFORMS: &[Platform] = &[Platform::Unix, Platform::Linux];
const ANY_PLATFORM: &[Platform] = &[Platform::Any];

/// Construction-time audit used by every non-REST router. A route constructor
/// must enumerate the exact method/path pairs it mounts; an absent contract
/// entry fails immediately in tests and startup rather than producing a green
/// qualification ledger.
pub fn assert_mounted_routes(routes: &[&str]) {
    for route in routes {
        let api_registered = route.split_once(' ').is_some_and(|(method, path)| {
            let method = match method {
                "GET" => Some(HttpMethod::Get),
                "POST" => Some(HttpMethod::Post),
                _ => None,
            };
            method.is_some_and(|method| {
                api_bindings().any(|binding| binding.path == path && binding.method == method)
            })
        });
        assert!(
            api_registered
                || INGEST_SURFACES
                    .iter()
                    .any(|(registered, _, _)| registered == route
                        || aliases_for(registered).iter().any(|alias| alias == route)),
            "mounted route {route} is absent from SurfaceContract"
        );
    }
}

pub fn contract_path(binding: &'static str) -> &'static str {
    assert_mounted_routes(&[binding]);
    binding
        .split_once(' ')
        .expect("contract route must be METHOD /path")
        .1
}

pub fn contracted_external_router<S>(
    bindings: &'static [&'static str],
    router: axum::Router<S>,
) -> axum::Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    assert_mounted_routes(bindings);
    router
}

/// A method router whose mounted verbs are carried alongside the Axum value.
///
/// This prevents a textual `"GET /path"` contract from being paired with a
/// `post(handler)` router. Callers can only obtain this value through the
/// verb-specific constructors below.
pub struct ContractMethodRouter<S, E = std::convert::Infallible> {
    methods: Vec<HttpMethod>,
    inner: axum::routing::MethodRouter<S, E>,
}

pub fn get<H, T, S>(handler: H) -> ContractMethodRouter<S>
where
    H: axum::handler::Handler<T, S>,
    T: 'static,
    S: Clone + Send + Sync + 'static,
{
    ContractMethodRouter {
        methods: vec![HttpMethod::Get],
        inner: axum::routing::get(handler),
    }
}

pub fn post<H, T, S>(handler: H) -> ContractMethodRouter<S>
where
    H: axum::handler::Handler<T, S>,
    T: 'static,
    S: Clone + Send + Sync + 'static,
{
    ContractMethodRouter {
        methods: vec![HttpMethod::Post],
        inner: axum::routing::post(handler),
    }
}

impl<S> ContractMethodRouter<S>
where
    S: Clone + Send + Sync + 'static,
{
    pub fn post<H, T>(mut self, handler: H) -> Self
    where
        H: axum::handler::Handler<T, S>,
        T: 'static,
    {
        self.methods.push(HttpMethod::Post);
        self.inner = self.inner.post(handler);
        self
    }

    pub fn request_body_limit(self, limit: usize) -> Self {
        ContractMethodRouter {
            methods: self.methods,
            inner: self
                .inner
                .layer(tower_http::limit::RequestBodyLimitLayer::new(limit)),
        }
    }
}

/// Axum extension that makes route construction consume a contracted
/// method/path. Unlike a post-hoc inventory, adding a mounted route through
/// this API cannot omit registration.
pub trait ContractRouterExt<S> {
    fn contract_route(self, binding: &'static str, method_router: ContractMethodRouter<S>) -> Self;
    fn contract_routes(
        self,
        bindings: &'static [&'static str],
        method_router: ContractMethodRouter<S>,
    ) -> Self;
}

impl<S> ContractRouterExt<S> for axum::Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    fn contract_route(self, binding: &'static str, method_router: ContractMethodRouter<S>) -> Self {
        let (method, _) = parse_binding(binding);
        assert_eq!(
            method_router.methods,
            [method],
            "contracted route method mismatch"
        );
        self.route(contract_path(binding), method_router.inner)
    }

    fn contract_routes(
        self,
        bindings: &'static [&'static str],
        method_router: ContractMethodRouter<S>,
    ) -> Self {
        assert_mounted_routes(bindings);
        let (_, path) = bindings[0]
            .split_once(' ')
            .expect("contract route must be METHOD /path");
        assert!(
            bindings.iter().all(|binding| binding.ends_with(path)),
            "combined methods must share a path"
        );
        let mut expected = bindings
            .iter()
            .map(|binding| parse_binding(binding).0)
            .collect::<Vec<_>>();
        expected.sort();
        expected.dedup();
        let mut mounted = method_router.methods;
        mounted.sort();
        mounted.dedup();
        assert_eq!(mounted, expected, "contracted route methods mismatch");
        self.route(path, method_router.inner)
    }
}

fn parse_binding(binding: &'static str) -> (HttpMethod, &'static str) {
    let (method, path) = binding
        .split_once(' ')
        .expect("contract route must be METHOD /path");
    let method = match method {
        "GET" => HttpMethod::Get,
        "POST" => HttpMethod::Post,
        _ => panic!("unsupported contracted HTTP method {method}"),
    };
    (method, path)
}

fn access_name(access: SurfaceAccess) -> &'static str {
    match access {
        SurfaceAccess::Read => "read",
        SurfaceAccess::Admin => "admin",
        SurfaceAccess::Info => "info",
        SurfaceAccess::LocalOnly => "local-only",
    }
}

fn required_cases(access: SurfaceAccess) -> Vec<RequiredCaseKind> {
    let mut cases = vec![
        RequiredCaseKind::SemanticPositive,
        RequiredCaseKind::ValidationNegative,
    ];
    if matches!(access, SurfaceAccess::Read | SurfaceAccess::Admin) {
        cases.push(RequiredCaseKind::Authorization);
    }
    cases
}

fn required_cases_for(spelling: &str, access: SurfaceAccess) -> Vec<RequiredCaseKind> {
    let mut cases = required_cases(access);
    if spelling == "GET /auth/google/callback" {
        cases[0] = RequiredCaseKind::ExecutedRefusalSemantic;
    }
    cases
}

fn cli_mutation(path: &str) -> MutationClass {
    if matches!(
        path,
        "search"
            | "filter"
            | "tail"
            | "hosts"
            | "sessions"
            | "analysis"
            | "state"
            | "status"
            | "correlate"
            | "stats"
            | "timeline"
            | "apps"
            | "entity"
            | "graph"
            | "config get"
            | "config list"
    ) || path.ends_with(" status")
        || path.ends_with(" list")
        || path.ends_with(" doctor")
    {
        return MutationClass::None;
    }
    if path.contains(" ack") || path.contains(" unack") || path.contains("filetail") {
        return MutationClass::Reversible;
    }
    if path.contains("prune") || path.ends_with(" down") {
        return MutationClass::Destructive;
    }
    if path.contains(" ingest") || path.starts_with("ingest ") || path.contains(" add") {
        return MutationClass::AppendOnly;
    }
    MutationClass::Operational
}

fn cli_access(path: &str, inherited: SurfaceAccess) -> SurfaceAccess {
    if matches!(
        path,
        "analysis incident"
            | "graph rebuild"
            | "graph status"
            | "sessions add"
            | "sessions assess"
            | "sessions doctor"
            | "sessions hooksbackfill"
            | "sessions index"
            | "sessions mcpassess"
            | "sessions skillassess"
            | "sessions smokewatch"
            | "sessions watch"
            | "sessions watchstatus"
    ) || path.starts_with("ingest inventory")
        || path.starts_with("ingest shell")
        || path.starts_with("ingest syslog")
        || path.starts_with("ingest docker")
    {
        SurfaceAccess::LocalOnly
    } else {
        inherited
    }
}

fn mcp_mutation(action: &str) -> MutationClass {
    match action {
        "ack_error" | "unack_error" | "file_tails" => MutationClass::Reversible,
        "notifications_test" => MutationClass::Operational,
        "artifact_evidence_record" => MutationClass::AppendOnly,
        _ => MutationClass::None,
    }
}

fn aliases_for(spelling: &str) -> Vec<String> {
    match spelling {
        "config list" => vec!["config ls".into()],
        "update clients" => vec!["update agents".into()],
        "update config clients" => vec!["update config agents".into()],
        "GET /app" => vec!["GET /app/".into()],
        _ => Vec::new(),
    }
}

fn cleanup_for(spelling: &str, mutation: MutationClass) -> Option<&'static str> {
    if mutation == MutationClass::None {
        return None;
    }
    if spelling.contains("filetail") || spelling.contains("file-tail") {
        return Some("remove-run-owned-file-tail-registration");
    }
    if spelling.contains("ack") {
        return Some("restore-error-acknowledgement-state");
    }
    if spelling.contains("notification") {
        return Some("delete-run-owned-mock-notification-evidence");
    }
    if spelling.contains("db") {
        return Some("discard-run-owned-database-volume");
    }
    if spelling.contains("compose") {
        return Some("remove-exact-run-owned-compose-resources");
    }
    if spelling.starts_with("POST /v1/") || spelling.contains("artifact") {
        return Some("purge-or-discard-run-owned-ingest-store");
    }
    Some("reconcile-surface-specific-run-state")
}

fn profiles_for(kind: &str, spelling: &str, mutation: MutationClass) -> &'static [&'static str] {
    if spelling.contains("/db/") || spelling.starts_with("db ") {
        return STORAGE_PROFILE;
    }
    if kind == "mcp" {
        return MCP_PROFILE;
    }
    if kind == "cli" || mutation == MutationClass::Destructive {
        return FULL_PROFILE;
    }
    if kind == "ingest" {
        if spelling == "agent-docker" {
            return AGENT_PROFILE;
        }
        if matches!(
            spelling,
            "GET /.well-known/oauth-authorization-server"
                | "GET /.well-known/oauth-protected-resource"
                | "GET /mcp/.well-known/oauth-authorization-server"
                | "GET /mcp/.well-known/oauth-protected-resource"
                | "GET /mcp/.well-known/openid-configuration"
                | "GET /auth/login"
                | "GET /auth/google/callback"
                | "GET /authorize"
                | "GET /jwks"
                | "POST /register"
                | "POST /token"
        ) {
            return AUTH_PROFILE;
        }
        if matches!(
            spelling,
            "GET /app" | "GET /app/{*path}" | "GET /app/assets/{*path}" | "GET /app/investigate"
        ) {
            return SECURITY_PROFILE;
        }
        return INGEST_PROFILE;
    }
    CORE_PROFILES
}

fn platforms_for(kind: &str, spelling: &str) -> &'static [Platform] {
    if kind == "cli"
        && (spelling.starts_with("compose")
            || spelling.starts_with("setup")
            || spelling.starts_with("heartbeat")
            || spelling.starts_with("update"))
    {
        UNIX_PLATFORMS
    } else {
        ANY_PLATFORM
    }
}

fn entry(
    kind: &'static str,
    spelling: String,
    method: Option<HttpMethod>,
    access: SurfaceAccess,
    mutation: MutationClass,
) -> SurfaceContractEntry {
    let mut token = spelling
        .trim_start_matches('/')
        .replace(['/', ' ', '_', '{', '}', '*'], "-")
        .to_ascii_lowercase();
    while token.contains("--") {
        token = token.replace("--", "-");
    }
    token = token.trim_matches('-').to_string();
    let method_token = method
        .map(|m| match m {
            HttpMethod::Get => "get-",
            HttpMethod::Post => "post-",
        })
        .unwrap_or("");
    let id = format!("{kind}.{method_token}{token}");
    let aliases = aliases_for(&spelling);
    let cleanup = cleanup_for(&spelling, mutation);
    let profiles = profiles_for(kind, &spelling, mutation);
    let platforms = platforms_for(kind, &spelling);
    let required_cases = required_cases_for(&spelling, access);
    SurfaceContractEntry {
        scenario_id: None,
        id,
        kind,
        spelling,
        aliases,
        method,
        auth: access_name(access),
        mutation,
        profiles,
        platforms,
        parity_group: matches!(kind, "cli" | "mcp" | "rest").then(|| format!("capability.{token}")),
        required_cases,
        allowed_dispositions: &[ProfileDisposition::PendingScenario],
        cleanup,
    }
}

/// Deterministic, versioned runtime inventory used by the live qualification
/// ledger. Removed compatibility spellings remain in `SURFACE_SPECS` but are
/// intentionally excluded from executable entries.
pub fn contract() -> SurfaceContractExport {
    let mut entries = Vec::new();
    for spec in SURFACE_SPECS.iter().filter(|s| {
        !matches!(
            s.disposition,
            SurfaceDisposition::RemovedCleanBreak | SurfaceDisposition::MovedIntoGroupedDomain
        )
    }) {
        match spec.kind {
            SurfaceKind::Cli => entries.push(entry(
                "cli",
                spec.spelling.into(),
                None,
                spec.access,
                cli_mutation(spec.spelling),
            )),
            SurfaceKind::McpAction => entries.push(entry(
                "mcp",
                spec.spelling.into(),
                None,
                spec.access,
                mcp_mutation(spec.spelling),
            )),
            _ => {}
        }
    }
    for (parent, children) in CLI_CHILDREN {
        for child in *children {
            let path = format!("{parent} {child}");
            let root = path.split_whitespace().next().unwrap_or(parent);
            let inherited_access = find(SurfaceKind::Cli, root)
                .map(|spec| spec.access)
                .unwrap_or(SurfaceAccess::LocalOnly);
            let access = cli_access(&path, inherited_access);
            entries.push(entry(
                "cli",
                path.clone(),
                None,
                access,
                cli_mutation(&path),
            ));
        }
    }
    for binding in api_bindings() {
        entries.push(entry(
            "rest",
            binding.path.into(),
            Some(binding.method),
            binding.access,
            binding.mutation,
        ));
    }
    for (name, access, mutation) in INGEST_SURFACES {
        entries.push(entry("ingest", (*name).into(), None, *access, *mutation));
    }
    entries.sort_by(|a, b| a.id.cmp(&b.id));
    SurfaceContractExport {
        version: SURFACE_CONTRACT_VERSION,
        entries,
    }
}

pub fn export_json() -> serde_json::Result<String> {
    serde_json::to_string_pretty(&contract())
}
