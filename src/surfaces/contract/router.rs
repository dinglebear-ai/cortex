use super::*;

/// Construction-time audit for routers that opt into the surface contract.
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
                || INGEST_SURFACES.iter().any(|(registered, _, _)| {
                    registered == route
                        || aliases_for(registered).iter().any(|alias| alias == route)
                }),
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

/// Axum extension that makes route construction consume a contracted method/path.
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
