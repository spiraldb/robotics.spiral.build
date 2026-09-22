//! Route registration and OpenAPI generation share the same handlers and extractors.
//!
//! This module knows HTTP framing, never feature request or response types.
use super::delivery::{ApiJson, ApiPath, ApiQuery, HttpFailure, ScopedProject};
use crate::projects::context::StudioContext;
use axum::{
    Json, Router,
    extract::State,
    handler::Handler,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{MethodFilter, MethodRouter, on},
};
use schemars::{JsonSchema, SchemaGenerator, generate::SchemaSettings};
use serde::Serialize;
use serde_json::{Map, Value, json};
use spiral_robotics_common::project::ProjectKey;
use std::future::Future;

pub(crate) struct ApiRoutes {
    router: Router<StudioContext>,
    schemas: SchemaGenerator,
    paths: Map<String, Value>,
}
impl ApiRoutes {
    pub(super) fn new() -> Self {
        let mut settings = SchemaSettings::draft2020_12();
        settings.definitions_path = "/components/schemas".into();
        Self {
            router: Router::new(),
            schemas: settings.into_generator(),
            paths: Map::new(),
        }
    }
    fn register<H, T>(&mut self, route: Route, handler: H)
    where
        H: Handler<T, StudioContext> + DocumentedHandler<T>,
        T: 'static,
    {
        let mut operation = H::describe(&mut self.schemas);
        for segment in route.path.split('/') {
            if let Some(name) = segment.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
                operation.parameters.push(json!({"name": name, "in": "path", "required": true, "schema": {"type": "string"}}));
            }
        }
        let description = operation.into_description();
        self.insert_description(&route, description);
        self.router = std::mem::take(&mut self.router).route(route.path, on(route.filter, handler));
    }
    fn insert_description(&mut self, route: &Route, description: Value) {
        let methods = self
            .paths
            .entry(route.path.to_owned())
            .or_insert_with(|| json!({}));
        assert!(
            methods.get(route.method).is_none(),
            "duplicate API operation: {} {}",
            route.method,
            route.path
        );
        methods[route.method] = description;
    }
    pub(crate) fn media(&mut self, path: &'static str, endpoint: MediaEndpoint) {
        self.insert_description(
            &Route::new("get", path, MethodFilter::GET),
            endpoint.description,
        );
        self.router = std::mem::take(&mut self.router).route(path, endpoint.router);
    }
    pub(super) fn into_router(mut self, studio: StudioContext) -> Router {
        let router = std::mem::take(&mut self.router);
        let document = self.into_document();
        router
            .route(
                "/openapi.json",
                axum::routing::get(move || {
                    let document = document.clone();
                    async move { Json(document) }
                }),
            )
            .fallback(HttpFailure::not_found)
            .method_not_allowed_fallback(HttpFailure::method_not_allowed)
            .layer(tower_http::set_header::SetResponseHeaderLayer::overriding(
                header::CACHE_CONTROL,
                axum::http::HeaderValue::from_static("no-store"),
            ))
            .with_state(studio)
    }
    pub(super) fn into_document(mut self) -> Value {
        json!({
            "openapi": "3.1.0",
            "info": { "title": "Spiral Robotics HTTP API", "version": "5.0.0",
                "description": "Backend-owned commands and frontend-initiated snapshots. X-Spiral-Endpoint and X-Spiral-Project name the project a request acts on; they are not authentication. Reads are never cached. Commands are never automatically retried. See docs/http-api.md for operation lifetimes and polling." },
            "servers": [{ "url": "/api" }],
            "paths": self.paths,
            "components": { "schemas": self.schemas.take_definitions(false) }
        })
    }
}
struct Route {
    method: &'static str,
    path: &'static str,
    filter: MethodFilter,
}
impl Route {
    fn new(method: &'static str, path: &'static str, filter: MethodFilter) -> Self {
        Self {
            method,
            path,
            filter,
        }
    }
}
macro_rules! register_methods {
    ($($method:ident => $filter:ident),* $(,)?) => {
        impl ApiRoutes {
            $(pub(crate) fn $method<H, T>(&mut self, path: &'static str, handler: H)
            where H: Handler<T, StudioContext> + DocumentedHandler<T>, T: 'static {
                self.register(Route::new(stringify!($method), path, MethodFilter::$filter), handler);
            })*
        }
    };
}
register_methods!(get => GET, post => POST, put => PUT, patch => PATCH, delete => DELETE, head => HEAD, options => OPTIONS);

/// Explicit descriptions only for native media responses (MJPEG, downloads, byte ranges).
pub(crate) struct MediaEndpoint {
    router: MethodRouter<StudioContext>,
    description: Value,
}
impl MediaEndpoint {
    pub(crate) fn new(router: MethodRouter<StudioContext>, description: Value) -> Self {
        Self {
            router,
            description,
        }
    }
}

pub(crate) struct OperationSchema {
    name: &'static str,
    parameters: Vec<Value>,
    body: Option<Value>,
    responses: Value,
}
impl OperationSchema {
    fn new<H, Output: ResponseSchema>(schemas: &mut SchemaGenerator) -> Self {
        Self {
            name: std::any::type_name::<H>()
                .rsplit("::")
                .next()
                .expect("handler name"),
            parameters: Vec::new(),
            body: None,
            responses: Output::responses(schemas),
        }
    }
    fn into_description(self) -> Value {
        let mut description = json!({"operationId": self.name, "parameters": self.parameters, "responses": self.responses});
        if let Some(body) = self.body {
            description["requestBody"] = body;
        }
        description
    }
    pub(crate) fn upload(&mut self, content_type: &str) {
        self.body = Some(
            json!({"required": true, "content": {content_type: {"schema": {"type": "string", "format": "binary"}}}}),
        );
    }
    pub(crate) fn proxy(&mut self) {
        self.parameters.push(json!({"name":"url", "in":"query", "required":true, "schema":{"type":"string", "format":"uri"}}));
        self.body = Some(
            json!({"required":false, "content":{"*/*":{"schema":{"type":"string", "format":"binary"}}}}),
        );
    }
}

pub(crate) trait RequestSchema {
    fn describe(operation: &mut OperationSchema, schemas: &mut SchemaGenerator);
}
impl<Capability> RequestSchema for ScopedProject<Capability> {
    fn describe(operation: &mut OperationSchema, _: &mut SchemaGenerator) {
        operation.parameters.push(json!({"name": ProjectKey::ENDPOINT_HEADER, "in": "header", "required": true,
            "description": "Spiral endpoint of the project; must match the saved connection from GET /settings/connection.",
            "schema": {"type": "string", "minLength": 1}}));
        operation.parameters.push(json!({"name": ProjectKey::PROJECT_HEADER, "in": "header", "required": false,
            "description": "Spiral project ID. Omit it before a project is chosen; Studio-only features still work.",
            "schema": {"type": "string"}}));
    }
}
impl RequestSchema for State<StudioContext> {
    fn describe(_: &mut OperationSchema, _: &mut SchemaGenerator) {}
}
impl<T> RequestSchema for ApiPath<T> {
    fn describe(_: &mut OperationSchema, _: &mut SchemaGenerator) {}
}
impl<T: JsonSchema> RequestSchema for ApiJson<T> {
    fn describe(operation: &mut OperationSchema, schemas: &mut SchemaGenerator) {
        let schema = schemas.subschema_for::<T>();
        operation.body =
            Some(json!({"required": true, "content": {"application/json": {"schema": schema}}}));
    }
}
impl<T: JsonSchema> RequestSchema for ApiQuery<T> {
    fn describe(operation: &mut OperationSchema, schemas: &mut SchemaGenerator) {
        let schema = schemas.subschema_for::<T>();
        let schema = schema.as_value();
        let definition = schema
            .get("$ref")
            .and_then(Value::as_str)
            .and_then(|path| path.rsplit('/').next())
            .and_then(|name| schemas.definitions().get(name))
            .unwrap_or(schema);
        if let Some(properties) = definition.get("properties").and_then(Value::as_object) {
            for (name, property) in properties {
                let required = definition
                    .get("required")
                    .and_then(Value::as_array)
                    .is_some_and(|fields| fields.iter().any(|field| field == name));
                operation.parameters.push(
                    json!({"name": name, "in": "query", "required": required, "schema": property}),
                );
            }
        }
    }
}

/// The status code is part of the response type, so implementation and schema agree.
pub(crate) struct Created<T>(pub(crate) T);
impl<T: Serialize> IntoResponse for Created<T> {
    fn into_response(self) -> Response {
        (StatusCode::CREATED, Json(self.0)).into_response()
    }
}
pub(crate) trait ResponseSchema {
    fn responses(schemas: &mut SchemaGenerator) -> Value;
}
struct JsonResponses<T>(std::marker::PhantomData<T>);
impl<T: JsonSchema> JsonResponses<T> {
    fn describe(schemas: &mut SchemaGenerator, status: &str) -> Value {
        let response = schemas.subschema_for::<T>();
        let failure =
            schemas.subschema_for::<spiral_robotics_common::command_failure::CommandFailure>();
        json!({
            status: {"description": "Completed request; background work may remain active as reported in state.", "content": {"application/json": {"schema": response}}},
            "default": {"description": "Structured failure. Inspect code; never infer a command outcome from a lost HTTP response.", "content": {"application/json": {"schema": failure}}}
        })
    }
}
impl<T: JsonSchema> ResponseSchema for Result<Json<T>, HttpFailure> {
    fn responses(schemas: &mut SchemaGenerator) -> Value {
        JsonResponses::<T>::describe(schemas, "200")
    }
}
impl<T: JsonSchema> ResponseSchema for Result<Created<T>, HttpFailure> {
    fn responses(schemas: &mut SchemaGenerator) -> Value {
        JsonResponses::<T>::describe(schemas, "201")
    }
}

/// A bounded IPC stream with its native schema and record batches.
pub(crate) struct ArrowResponse(pub(crate) Vec<u8>);
impl IntoResponse for ArrowResponse {
    fn into_response(self) -> Response {
        (
            [(
                axum::http::header::CONTENT_TYPE,
                "application/vnd.apache.arrow.stream",
            )],
            self.0,
        )
            .into_response()
    }
}
impl ResponseSchema for Result<ArrowResponse, HttpFailure> {
    fn responses(schemas: &mut SchemaGenerator) -> Value {
        let failure =
            schemas.subschema_for::<spiral_robotics_common::command_failure::CommandFailure>();
        json!({"200": {"description": "Bounded message page; schema metadata includes its read identity and continuation cursor.", "content": {"application/vnd.apache.arrow.stream": {"schema": {"type": "string", "format": "binary"}}}}, "default": {"description": "Structured failure", "content": {"application/json": {"schema": failure}}}})
    }
}

pub(crate) trait DocumentedHandler<Arguments> {
    fn describe(schemas: &mut SchemaGenerator) -> OperationSchema;
}
// Axum's handler tuple starts with its extraction marker. The remaining types
// are the real function arguments, including the body extractor in last position.
macro_rules! describe_handlers {
    ($($argument:ident),+) => {
        impl<F, Fut, Marker, $($argument,)+> DocumentedHandler<(Marker, $($argument,)+)> for F
        where F: FnOnce($($argument),+) -> Fut, Fut: Future, Fut::Output: ResponseSchema,
            $($argument: RequestSchema,)+ {
            fn describe(schemas: &mut SchemaGenerator) -> OperationSchema {
                let mut operation = OperationSchema::new::<F, Fut::Output>(schemas);
                $($argument::describe(&mut operation, schemas);)+
                operation
            }
        }
    };
}
describe_handlers!(A);
describe_handlers!(A, B);
describe_handlers!(A, B, C);
