use axum::{extract::Request, response::Response};

use futures::future::BoxFuture;
use tracing::debug;

use crate::{AuthorizationClaims, authorization::PrincipalExtractor, errors::unauthorized_response};

// pub async fn principal_extractor_layer(request: http::Request<axum::body::Body>, next: Next) -> Response {
//     let maybe_claims = request.extensions().get::<AuthorizationClaims>();
//     let Some(Ok(authorized_principal)) = maybe_claims.map(|claims| {
//         AuthorizedPrincipal::try_from(claims).inspect_err(|e| debug!("Can't extract the principal {e:?}"))
//     }) else {
//         return unauthorized_response("Invalid token. Unable to extract the principal from claims");
//     };
//     let (mut parts, body) = request.into_parts();
//     parts.extensions.insert(authorized_principal);
//     let request = Request::from_parts(parts, body);
//     next.run(request).await
// }

use std::task::{Context, Poll};
use tower::{Layer, Service};

#[derive(Clone)]
pub struct PrincipalExtractorLayer<E> {
    principal_extractor: E,
}

impl<E> PrincipalExtractorLayer<E> {
    pub fn new(principal_extractor: E) -> Self {
        Self { principal_extractor }
    }
}

impl<S, E> Layer<S> for PrincipalExtractorLayer<E>
where
    E: Clone,
{
    type Service = PrincipalExtractorMiddleware<S, E>;

    fn layer(&self, inner: S) -> Self::Service {
        PrincipalExtractorMiddleware { inner, principal_extractor: self.principal_extractor.clone() }
    }
}

#[derive(Clone)]
pub struct PrincipalExtractorMiddleware<S, E> {
    principal_extractor: E,
    inner: S,
}

impl<S, E> Service<Request> for PrincipalExtractorMiddleware<S, E>
where
    S: Service<Request, Response = Response> + Send + 'static + Clone,
    S::Future: Send + 'static,
    E: PrincipalExtractor + Send + Clone + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    // `BoxFuture` is a type alias for `Pin<Box<dyn Future + Send + 'a>>`
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: Request) -> Self::Future {
        let mut inner = self.inner.clone();
        let principal_extractor = self.principal_extractor.clone();
        Box::pin(async move {
            let maybe_authorization_claims = request.extensions().get::<AuthorizationClaims>();
            let Some(Ok(authorized_principal)) = maybe_authorization_claims.map(|authorization_claims| {
                principal_extractor
                    .extract(&authorization_claims.into())
                    .inspect_err(|e| debug!("Can't extract the principal {e:?}"))
            }) else {
                return Ok(unauthorized_response("Invalid token. Unable to extract the principal from claims"));
            };
            let (mut parts, body) = request.into_parts();
            parts.extensions.insert(authorized_principal);
            let request = Request::from_parts(parts, body);
            inner.call(request).await
        })
    }
}
