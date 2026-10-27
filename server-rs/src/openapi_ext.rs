//! Everything gotcha 0.4.1 cannot express about the OpenAPI document, expressed
//! declaratively and applied in a single `GotchaRouter::openapi(..)` pass.
//!
//! gotcha derives one `200` response per handler from the return type and folds
//! a `Result`'s error side into `default` (`gotcha_core-0.4.1/src/responsible.rs:129`),
//! so a route cannot document a `404`/`409` of its own. The registry below closes
//! that gap without touching the framework: [`ResponseOverride`] entries name a
//! `(method, path, status)` and a body type, and [`apply`] inserts the
//! `Schematic`-generated schema into `components/schemas` (reusing the generated
//! name) plus the status entry into the operation's `responses`.
//!
//! It also carries two shape fixes that keep the generated TypeScript identical
//! to the zod contract in the former TypeScript server's `schemas.ts`:
//!
//! * [`Nullable`] — a value that is always present but may be `null`. gotcha maps
//!   `Option<T>` to *optional and nullable*, but zod's `.nullable()` (without
//!   `.optional()`) means *required and nullable*, which is a different TS type.
//! * [`hoist_nullable_refs`] — zod-openapi puts `.nullable()` on a registered
//!   object schema *inside* the component (`Injection` is `["object","null"]`),
//!   whereas a `Nullable<Injection>` field would carry `nullable` next to its
//!   `$ref`, where generators ignore it.

use std::collections::BTreeMap;

use gotcha::oas::{MediaType, OpenAPIV3, Operation, Referenceable, Response, Schema};
use gotcha::Schematic;
use gotcha_core::{registry, EnhancedSchema};
use serde::{Deserialize, Serialize};

/// A value that is always present in the payload but may be `null`.
///
/// Mirrors zod's `.nullable()` without `.optional()`: `required` in the schema,
/// `nullable: true` on the value. Plain `Option<T>` cannot express this because
/// `<Option<T> as Schematic>::required()` is hard-coded to `false`
/// (`gotcha_core-0.4.1/src/lib.rs:262`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Nullable<T>(pub Option<T>);

impl<T> Nullable<T> {
    /// The wrapped value, if any.
    pub fn as_ref(&self) -> Option<&T> {
        self.0.as_ref()
    }
}

impl<T> From<Option<T>> for Nullable<T> {
    fn from(value: Option<T>) -> Self {
        Self(value)
    }
}

impl<T> From<T> for Nullable<T> {
    fn from(value: T) -> Self {
        Self(Some(value))
    }
}

impl<T> Default for Nullable<T> {
    fn default() -> Self {
        Self(None)
    }
}

impl<T: Schematic> Schematic for Nullable<T> {
    fn name() -> &'static str {
        T::name()
    }

    fn required() -> bool {
        true
    }

    fn nullable() -> Option<bool> {
        Some(true)
    }

    fn type_() -> &'static str {
        T::type_()
    }

    fn doc() -> Option<String> {
        T::doc()
    }

    fn generate_schema() -> EnhancedSchema {
        let mut schema = T::generate_schema().schema;
        schema.nullable = Some(true);
        EnhancedSchema { schema, required: true }
    }
}

/// A value that may be absent from the payload, but is never `null`.
///
/// Mirrors zod's `.optional()` without `.nullable()`: the key is missing from
/// `required` and the value carries no `nullable`. Plain `Option<T>` cannot
/// express this either, because gotcha hard-codes
/// `<Option<T> as Schematic>::nullable()` to `Some(true)`
/// (`gotcha_core-0.4.1/src/lib.rs:275`), which generators read as `T | null`.
///
/// Pair it with `#[serde(default, skip_serializing_if = "Optional::is_none")]`
/// so an absent value stays absent on the wire rather than becoming `null`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Optional<T>(pub Option<T>);

impl<T> Optional<T> {
    /// Whether the value is absent.
    pub fn is_none(&self) -> bool {
        self.0.is_none()
    }

    /// The wrapped value, if any.
    pub fn as_ref(&self) -> Option<&T> {
        self.0.as_ref()
    }
}

impl Optional<String> {
    /// The wrapped string as a `&str`, if present.
    pub fn as_deref(&self) -> Option<&str> {
        self.0.as_deref()
    }
}

impl<T> From<Option<T>> for Optional<T> {
    fn from(value: Option<T>) -> Self {
        Self(value)
    }
}

impl<T> Default for Optional<T> {
    fn default() -> Self {
        Self(None)
    }
}

impl<T: Schematic> Schematic for Optional<T> {
    fn name() -> &'static str {
        T::name()
    }

    fn required() -> bool {
        false
    }

    fn nullable() -> Option<bool> {
        None
    }

    fn type_() -> &'static str {
        T::type_()
    }

    fn doc() -> Option<String> {
        T::doc()
    }

    fn generate_schema() -> EnhancedSchema {
        EnhancedSchema {
            schema: T::generate_schema().schema,
            required: false,
        }
    }
}

/// Deserialize a [`Nullable`] field that the client must actually send.
///
/// serde's `missing_field` helper hands an absent field a deserializer that
/// answers `deserialize_option` with `None`, so a plain `Nullable` field is
/// silently optional. Naming a `deserialize_with` function takes that path away:
/// serde has no `Deserialize` impl to fall back on, so an absent key is an error
/// — which is what zod's `.nullable()` (without `.optional()`) means.
pub fn required_nullable<'de, D, T>(deserializer: D) -> Result<Nullable<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Nullable)
}

/// The double-`Option` that tells an absent field from a null one.
///
/// serde folds `null` into `None` for a plain `Option`, so the outer layer has
/// to be filled in explicitly; `#[serde(default)]` then supplies the absent
/// case. The generated schema is the same as a plain `Option<T>`'s — optional
/// and nullable — which is exactly `T | null | undefined` on the wire.
pub fn double_option<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Deserialize::deserialize(deserializer).map(Some)
}

/// A value of any JSON shape, as zod's `z.any()` documents it.
///
/// `serde_json::Value` already generates the empty schema `{}` that `z.any()`
/// produces, but it is *required*; `Option<Value>` is optional yet also marks
/// the schema `nullable`, which is a different TypeScript type. This is the
/// third combination — optional, no nullability — and it is what every
/// client-owned blob in the contract (`Screen.layout`) uses.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AnyValue(pub serde_json::Value);

impl Default for AnyValue {
    fn default() -> Self {
        Self(serde_json::Value::Null)
    }
}

impl Schematic for AnyValue {
    fn name() -> &'static str {
        "any"
    }

    fn required() -> bool {
        false
    }

    fn type_() -> &'static str {
        "object"
    }

    fn generate_schema() -> EnhancedSchema {
        EnhancedSchema {
            schema: Schema {
                _type: None,
                format: None,
                nullable: None,
                description: None,
                extras: Default::default(),
            },
            required: false,
        }
    }
}

/// Builds a schema plus every named component it pulled in, ready to be merged
/// into `components/schemas`.
type SchemaFactory = fn() -> (Schema, BTreeMap<String, Schema>);

fn collect_schema<T: Schematic>() -> (Schema, BTreeMap<String, Schema>) {
    // Run inside a fresh collection scope so a derived type comes back as a
    // `$ref` and its component (plus anything it references) lands in the map,
    // exactly as it would during gotcha's own assembly.
    let (schema, components) = registry::collect(|| T::generate_schema());
    (schema.schema, components)
}

/// The media type a JSON body is served as.
const JSON: &str = "application/json";

/// Implement `Schematic` for a string enum that must stay *inline* in its
/// parent schema.
///
/// A derived `Schematic` registers a named component and refers to it by
/// `$ref`. The contract spells these enums as inline string enums in the field
/// they belong to, and `openapi/openapi.json` is a checked-in contract, so the
/// generated document must stay byte-for-byte what it was.
#[macro_export]
macro_rules! inline_string_enum {
    ($name:ident, [$($variant:literal),+ $(,)?]) => {
        impl ::gotcha::Schematic for $name {
            fn name() -> &'static str {
                ::std::stringify!($name)
            }

            fn required() -> bool {
                true
            }

            fn type_() -> &'static str {
                "string"
            }

            fn generate_schema() -> ::gotcha_core::EnhancedSchema {
                ::gotcha_core::EnhancedSchema {
                    schema: ::gotcha::oas::Schema {
                        _type: Some("string".to_string()),
                        format: None,
                        nullable: None,
                        description: None,
                        extras: ::std::collections::BTreeMap::from([(
                            "enum".to_string(),
                            ::serde_json::json!([$($variant),+]),
                        )]),
                    },
                    required: true,
                }
            }
        }
    };
}

/// One declarative amendment to a generated operation.
struct ResponseOverride {
    /// HTTP method, uppercase (`"GET"`).
    pub method: &'static str,
    /// Route path as registered (`"/api/sessions/{source}/{id}"`).
    pub path: &'static str,
    /// Status code the entry documents.
    pub status: u16,
    /// The response description.
    pub description: &'static str,
    /// Body schema, or `None` to only set the description of an existing entry.
    pub schema: Option<SchemaFactory>,
    /// Media type the body is served as.
    pub content_type: &'static str,
    /// Drop the `default` response gotcha derives from the handler's `Result`
    /// error side, now that the real status codes are documented.
    pub drop_default: bool,
    /// Move the response generated under this status to [`Self::status`],
    /// keeping its body. gotcha derives every success as `200`
    /// (`gotcha_core-0.4.1/src/responsible.rs:25`), so a `201` route has to say so.
    pub rename_from: Option<u16>,
}

/// One declarative amendment to a generated operation's request body.
///
/// gotcha derives a body schema from the extractor, and `Multipart` carries
/// none — the generated `requestBody` names the media type and stops there. A
/// route whose parts the handler really does know therefore documents them
/// through this.
pub struct RequestBodyOverride {
    /// HTTP method, uppercase (`"POST"`).
    pub method: &'static str,
    /// Route path as registered.
    pub path: &'static str,
    /// Media type the body is read as.
    pub content_type: &'static str,
    /// The body's description.
    pub description: &'static str,
    /// Body schema.
    pub schema: SchemaFactory,
}

/// Every override a subsystem contributes, collected in one place.
///
/// Each module's `describe(&mut ExtraResponses)` pushes its own amendments here,
/// so `api::routes` registers a subsystem with one line for its routes and one
/// for its documentation, and no module has to know the shape of the registry.
#[derive(Default)]
pub struct ExtraResponses {
    responses: Vec<ResponseOverride>,
    request_bodies: Vec<RequestBodyOverride>,
}

impl ExtraResponses {
    /// Reword a response gotcha already generated (it hard-codes `"default return"`).
    pub fn describe(&mut self, method: &'static str, path: &'static str, status: u16, description: &'static str) -> &mut Self {
        self.responses.push(ResponseOverride {
            method,
            path,
            status,
            description,
            schema: None,
            content_type: JSON,
            drop_default: false,
            rename_from: None,
        });
        self
    }

    /// Document a failure status whose body is the API's [`crate::api::error::Error`].
    pub fn error(&mut self, method: &'static str, path: &'static str, status: u16, description: &'static str) -> &mut Self {
        self.body::<crate::api::error::Error>(method, path, status, description)
    }

    /// Document an extra status whose body is `T`.
    pub fn body<T: Schematic>(&mut self, method: &'static str, path: &'static str, status: u16, description: &'static str) -> &mut Self {
        self.responses.push(ResponseOverride {
            method,
            path,
            status,
            description,
            schema: Some(collect_schema::<T>),
            content_type: JSON,
            drop_default: true,
            rename_from: None,
        });
        self
    }

    /// [`Self::describe`], and drop the `default` response gotcha derives from a
    /// `Result` handler's error side — for a route whose documented statuses are
    /// exactly the ones it already carries.
    pub fn describe_final(&mut self, method: &'static str, path: &'static str, status: u16, description: &'static str) -> &mut Self {
        self.responses.push(ResponseOverride {
            method,
            path,
            status,
            description,
            schema: None,
            content_type: JSON,
            drop_default: true,
            rename_from: None,
        });
        self
    }

    /// Restate the generated `200` under the status the handler actually sends.
    ///
    /// The body schema is kept exactly as gotcha derived it from the return
    /// type; only the key and the description change.
    pub fn restatus(&mut self, method: &'static str, path: &'static str, from: u16, to: u16, description: &'static str) -> &mut Self {
        self.responses.push(ResponseOverride {
            method,
            path,
            status: to,
            description,
            schema: None,
            content_type: JSON,
            drop_default: false,
            rename_from: Some(from),
        });
        self
    }

    /// Document a success status that carries raw bytes.
    ///
    /// A handler that streams a file returns `axum::response::Response`, which
    /// is not `Schematic`, so it cannot be documented from the return type at
    /// all; the route documents its success case through this instead.
    pub fn binary(&mut self, method: &'static str, path: &'static str, status: u16, description: &'static str) -> &mut Self {
        fn binary_schema() -> (Schema, BTreeMap<String, Schema>) {
            (
                Schema {
                    _type: Some("string".to_string()),
                    format: Some("binary".to_string()),
                    nullable: None,
                    description: None,
                    extras: Default::default(),
                },
                BTreeMap::new(),
            )
        }
        self.responses.push(ResponseOverride {
            method,
            path,
            status,
            description,
            schema: Some(binary_schema),
            content_type: "application/octet-stream",
            drop_default: true,
            rename_from: None,
        });
        self
    }

    /// Document the body of a route gotcha could derive nothing for.
    pub fn request_body<T: Schematic>(
        &mut self,
        method: &'static str,
        path: &'static str,
        content_type: &'static str,
        description: &'static str,
    ) -> &mut Self {
        self.request_bodies.push(RequestBodyOverride {
            method,
            path,
            content_type,
            description,
            schema: collect_schema::<T>,
        });
        self
    }
}

fn operation_mut<'a>(spec: &'a mut OpenAPIV3, method: &str, path: &str) -> Option<&'a mut Operation> {
    let item = spec.paths.get_mut(path)?;
    match method {
        "GET" => item.get.as_mut(),
        "POST" => item.post.as_mut(),
        "PUT" => item.put.as_mut(),
        "PATCH" => item.patch.as_mut(),
        "DELETE" => item.delete.as_mut(),
        _ => None,
    }
}

fn body_response(description: &str, content_type: &str, schema: Schema) -> Response {
    Response {
        description: description.to_string(),
        headers: None,
        content: Some(BTreeMap::from([(
            content_type.to_string(),
            MediaType {
                schema: Some(Referenceable::Data(schema)),
                example: None,
                examples: None,
                encoding: None,
            },
        )])),
        links: None,
    }
}

/// Apply every override, then normalize the nullable-`$ref` placement.
///
/// A `(method, path)` that matches no generated operation is a programming
/// error — the route was renamed or never registered — so it is logged loudly
/// rather than silently skipped.
pub fn apply(mut spec: OpenAPIV3, overrides: &ExtraResponses) -> OpenAPIV3 {
    let mut new_components: BTreeMap<String, Schema> = BTreeMap::new();

    for entry in &overrides.request_bodies {
        let (schema, components) = (entry.schema)();
        let Some(operation) = operation_mut(&mut spec, entry.method, entry.path) else {
            tracing::error!(method = entry.method, path = entry.path, "openapi override targets an unregistered route");
            continue;
        };
        new_components.extend(components);
        operation.request_body = Some(Referenceable::Data(gotcha::oas::RequestBody {
            description: Some(entry.description.to_string()),
            required: Some(true),
            content: BTreeMap::from([(
                entry.content_type.to_string(),
                MediaType {
                    schema: Some(Referenceable::Data(schema)),
                    example: None,
                    examples: None,
                    encoding: None,
                },
            )]),
        }));
    }

    for entry in &overrides.responses {
        let built = entry.schema.map(|factory| factory());
        let Some(operation) = operation_mut(&mut spec, entry.method, entry.path) else {
            tracing::error!(method = entry.method, path = entry.path, "openapi override targets an unregistered route");
            continue;
        };
        let status = entry.status.to_string();
        if let Some(from) = entry.rename_from {
            match operation.responses.data.remove(&from.to_string()) {
                Some(response) => {
                    operation.responses.data.insert(status.clone(), response);
                }
                None => tracing::error!(
                    method = entry.method,
                    path = entry.path,
                    from,
                    "openapi override renames a response that was never generated"
                ),
            }
        }
        match built {
            Some((schema, components)) => {
                new_components.extend(components);
                operation
                    .responses
                    .data
                    .insert(status, Referenceable::Data(body_response(entry.description, entry.content_type, schema)));
            }
            None => match operation.responses.data.get_mut(&status) {
                Some(Referenceable::Data(response)) => response.description = entry.description.to_string(),
                _ => tracing::error!(
                    method = entry.method,
                    path = entry.path,
                    status,
                    "openapi override targets a missing response"
                ),
            },
        }
        if entry.drop_default {
            operation.responses.default = None;
        }
    }

    if !new_components.is_empty() {
        let components = spec.components.get_or_insert_with(default_components);
        let schemas = components.schemas.get_or_insert_with(BTreeMap::new);
        for (name, schema) in new_components {
            schemas.entry(name).or_insert(Referenceable::Data(schema));
        }
    }

    declare_bearer_scheme(&mut spec);
    hoist_nullable_refs(&mut spec);
    spec
}

fn default_components() -> gotcha::oas::Components {
    gotcha::oas::Components {
        schemas: None,
        responses: None,
        parameters: None,
        examples: None,
        request_bodies: None,
        headers: None,
        security_schemes: None,
        links: None,
        callbacks: None,
    }
}

/// Register the `bearerAuth` scheme every `#[api(security = "bearerAuth")]`
/// operation refers to.
///
/// gotcha emits the *requirement* on the operation
/// (`gotcha-0.4.1/src/openapi/mod.rs:127`) but hard-codes
/// `security_schemes: None` on the components it builds (`:175`), so a document
/// with requirements and no definitions would be one no generator could resolve.
///
/// The scheme is named on the `/api/auth/*` operations rather than on the
/// document as a whole because the requirement is conditional: a machine with no
/// accounts needs no token anywhere, and a machine with accounts needs one
/// almost everywhere. Neither is expressible in OpenAPI, so the spec states the
/// part that is always true — these routes take a bearer token — and
/// `GET /api/auth/status` is what a client asks about the rest.
fn declare_bearer_scheme(spec: &mut OpenAPIV3) {
    use gotcha::oas::{SecurityScheme, SecurityType};

    let components = spec.components.get_or_insert_with(default_components);
    components
        .security_schemes
        .get_or_insert_with(BTreeMap::new)
        .entry(crate::auth::routes::BEARER_SCHEME.to_string())
        .or_insert_with(|| {
            Referenceable::Data(SecurityScheme {
                _type: SecurityType::Http {
                    scheme: "bearer".to_string(),
                    // Not a JWT and deliberately so: it is 32 random bytes with
                    // no structure, which is why the server can store nothing but
                    // its sha256 and still check it.
                    bearer_format: Some("opaque".to_string()),
                },
                description: Some("A token from `POST /api/auth/login`.".to_string()),
            })
        });
}

/// Move `nullable` off a `{ "$ref": X, "nullable": true }` property and onto the
/// component `X` itself.
///
/// `$ref` siblings are ignored by every generator, so the field would come out
/// as a non-nullable `X`. zod-openapi puts the nullability on the component too
/// (`Injection` is `["object","null"]` in `openapi/openapi.json`), so this both
/// fixes the type and matches the golden contract.
fn hoist_nullable_refs(spec: &mut OpenAPIV3) {
    let Some(schemas) = spec.components.as_mut().and_then(|c| c.schemas.as_mut()) else {
        return;
    };

    let mut to_null: Vec<String> = Vec::new();
    for schema in schemas.values_mut() {
        let Referenceable::Data(schema) = schema else { continue };
        let Some(serde_json::Value::Object(properties)) = schema.extras.get_mut("properties") else {
            continue;
        };
        for property in properties.values_mut() {
            let Some(object) = property.as_object_mut() else { continue };
            let is_nullable_ref = object.get("nullable") == Some(&serde_json::Value::Bool(true)) && object.contains_key("$ref");
            if !is_nullable_ref {
                continue;
            }
            object.remove("nullable");
            if let Some(name) = object.get("$ref").and_then(|r| r.as_str()).and_then(|r| r.rsplit('/').next()) {
                to_null.push(name.to_string());
            }
        }
    }

    for name in to_null {
        if let Some(Referenceable::Data(schema)) = schemas.get_mut(&name) {
            schema.nullable = Some(true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Schematic)]
    #[allow(dead_code)]
    struct Inner {
        value: String,
    }

    #[derive(Schematic)]
    #[allow(dead_code)]
    struct Outer {
        inner: Nullable<Inner>,
    }

    #[test]
    fn nullable_is_required_and_nullable() {
        assert!(<Nullable<String> as Schematic>::required(), "zod .nullable() keeps the field required");
        let schema = <Nullable<String> as Schematic>::generate_schema();
        assert_eq!(schema.schema.nullable, Some(true));
    }

    #[test]
    fn a_nullable_field_must_actually_be_present() {
        #[derive(Deserialize)]
        struct Body {
            #[allow(dead_code)]
            #[serde(deserialize_with = "required_nullable")]
            value: Nullable<String>,
        }
        assert!(serde_json::from_str::<Body>(r#"{"value": null}"#).is_ok(), "null is a value");
        assert!(serde_json::from_str::<Body>(r#"{"value": "x"}"#).is_ok());
        assert!(
            serde_json::from_str::<Body>("{}").is_err(),
            "zod's .nullable() without .optional() means the key has to be there"
        );
    }

    #[test]
    fn optional_is_neither_required_nor_nullable() {
        assert!(
            !<Optional<String> as Schematic>::required(),
            "zod .optional() drops the field from `required`"
        );
        assert_eq!(
            <Optional<String> as Schematic>::nullable(),
            None,
            "…but never widens the value to `T | null`"
        );
        let schema = <Optional<String> as Schematic>::generate_schema();
        assert_eq!(schema.schema.nullable, None);
        assert!(!schema.required);
    }

    #[test]
    fn nullable_ref_moves_onto_the_component() {
        let (_, components) = registry::collect(<Outer as Schematic>::generate_schema);
        let mut spec = empty_spec(components);
        hoist_nullable_refs(&mut spec);

        let schemas = spec.components.unwrap().schemas.unwrap();
        let Referenceable::Data(outer) = &schemas["Outer"] else {
            panic!("Outer inlined")
        };
        let properties = outer.extras["properties"].as_object().unwrap();
        let inner = properties["inner"].as_object().unwrap();
        assert!(inner.contains_key("$ref"), "the field stays a $ref");
        assert!(!inner.contains_key("nullable"), "a $ref sibling is ignored by generators");

        let Referenceable::Data(inner_component) = &schemas["Inner"] else {
            panic!("Inner inlined")
        };
        assert_eq!(inner_component.nullable, Some(true), "nullability lands on the component");
    }

    fn empty_spec(components: BTreeMap<String, Schema>) -> OpenAPIV3 {
        OpenAPIV3 {
            info: gotcha::oas::Info {
                title: String::new(),
                description: None,
                terms_of_service: None,
                contact: None,
                license: None,
                version: String::new(),
            },
            paths: BTreeMap::default(),
            servers: None,
            components: Some(gotcha::oas::Components {
                schemas: Some(components.into_iter().map(|(k, v)| (k, Referenceable::Data(v))).collect()),
                ..default_components()
            }),
            security: None,
            tags: None,
            openapi: "3.0.0".to_string(),
            external_docs: None,
            extras: None,
        }
    }
}
