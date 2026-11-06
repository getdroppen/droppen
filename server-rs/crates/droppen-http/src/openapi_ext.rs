//! The OpenAPI document's shape fixes, applied in a single
//! `GotchaRouter::openapi(..)` pass.
//!
//! Responses are declared on the handlers themselves, with
//! `#[api(responses(..), errors(..))]`; what remains here is what no per-route
//! declaration can say. [`normalize`] registers the `bearerAuth` scheme, folds
//! gotcha's `anyOf` nullability back onto its `$ref`, hoists it onto the
//! component, and restores the `required` lists gotcha loosens. The wrappers
//! [`Nullable`], [`Optional`] and [`AnyValue`] express the zod shapes gotcha's
//! `Option<T>` cannot, and [`BinaryBody`] documents a raw-bytes response.
//!
//! The generated TypeScript must stay identical to the checked-in contract
//! (`openapi/openapi.json`), which is why the nullability placement matters:
//! the contract carries it on the registered component (`Injection` is
//! `type: object` with `nullable: true`), whereas a `Nullable<Injection>` field
//! would carry `nullable` next to its `$ref`, where generators ignore it.

use std::collections::BTreeMap;

use gotcha::oas::{OpenAPIV3, Referenceable, Schema};
use gotcha::{EnhancedSchema, Schematic};
use serde::{Deserialize, Serialize};

/// A value that is always present in the payload but may be `null`.
///
/// The contract's required-and-nullable shape: `required` in the schema,
/// `nullable: true` on the value. Plain `Option<T>` cannot express this because
/// `<Option<T> as Schematic>::required()` is hard-coded to `false`
/// (`gotcha_core-0.5.1/src/lib.rs:624`).
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
/// The contract's optional-and-not-nullable shape: the key is missing from
/// `required` and the value carries no `nullable`. Plain `Option<T>` cannot
/// express this either, because gotcha hard-codes
/// `<Option<T> as Schematic>::nullable()` to `Some(true)`
/// (`gotcha_core-0.5.1/src/lib.rs:627`), which generators read as `T | null`.
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

/// A value of any JSON shape, as the contract documents it.
///
/// `serde_json::Value` already generates the empty schema `{}`, but it is
/// *required*; `Option<Value>` is optional yet also marks the schema
/// `nullable`, which is a different TypeScript type. This is the third
/// combination — optional, no nullability — and it is what every client-owned
/// blob in the contract (`Screen.layout`) uses.
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
            schema: Schema::default(),
            required: false,
        }
    }
}

/// Raw bytes, as the contract documents a file download.
///
/// A handler that streams a `Response` has no `Schematic` of its own, so it
/// declares that response with `skip_response` and names this type as the body.
/// gotcha 0.5's own `Binary` emits `contentMediaType`, but the contract spells
/// the schema `{"type": "string", "format": "binary"}`.
pub struct BinaryBody;

impl Schematic for BinaryBody {
    fn name() -> &'static str {
        "BinaryBody"
    }

    fn required() -> bool {
        true
    }

    fn type_() -> &'static str {
        "string"
    }

    fn generate_schema() -> EnhancedSchema {
        EnhancedSchema {
            schema: Schema {
                _type: Some("string".into()),
                format: Some("binary".into()),
                ..Default::default()
            },
            required: true,
        }
    }
}

/// The security scheme name the guarded operations reference, and the key it is
/// registered under in `components/securitySchemes`.
pub const BEARER_SCHEME: &str = "bearerAuth";

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
        impl $crate::gotcha::Schematic for $name {
            fn name() -> &'static str {
                ::std::stringify!($name)
            }

            fn required() -> bool {
                true
            }

            fn type_() -> &'static str {
                "string"
            }

            fn generate_schema() -> $crate::gotcha::EnhancedSchema {
                $crate::gotcha::EnhancedSchema {
                    schema: $crate::gotcha::oas::Schema {
                        _type: Some("string".into()),
                        extras: ::std::collections::BTreeMap::from([(
                            "enum".to_string(),
                            $crate::gotcha::serde_json::json!([$($variant),+]),
                        )]),
                        ..::std::default::Default::default()
                    },
                    required: true,
                }
            }
        }
    };
}

/// The document normalizations this module makes, on the document they amend.
trait SpecAmendmentExt {
    /// Register the `bearerAuth` scheme every `#[api(security = "bearerAuth")]`
    /// operation refers to.
    ///
    /// gotcha emits the *requirement* on the operation
    /// (`gotcha-0.5.1/src/openapi/mod.rs:169`) but hard-codes
    /// `security_schemes: None` on the components it builds (`:445`), so a
    /// document with requirements and no definitions would be one no generator
    /// could resolve.
    ///
    /// The scheme is named on the `/api/auth/*` operations rather than on the
    /// document as a whole because the requirement is conditional: a machine
    /// with no accounts needs no token anywhere, and a machine with accounts
    /// needs one almost everywhere. Neither is expressible in OpenAPI, so the
    /// spec states the part that is always true — these routes take a bearer
    /// token — and `GET /api/auth/status` is what a client asks about the rest.
    fn declare_bearer_scheme(&mut self);

    /// Move `nullable` off a `{ "$ref": X, "nullable": true }` property and
    /// onto the component `X` itself.
    ///
    /// `$ref` siblings are ignored by every generator, so the field would come
    /// out as a non-nullable `X`. The checked-in contract puts the nullability
    /// on the component too (`Injection` is `type: object` with `nullable:
    /// true` in `openapi/openapi.json`), so this both fixes the type and
    /// matches it.
    fn hoist_nullable_refs(&mut self);

    /// Turn each component property's `anyOf` of a `$ref` and `null` back into
    /// a nullable `$ref`, which [`Self::hoist_nullable_refs`] then moves onto
    /// the component. Only the properties `hoist` looks at are rewritten.
    fn fold_nullable_refs(&mut self);
}

impl SpecAmendmentExt for OpenAPIV3 {
    fn declare_bearer_scheme(&mut self) {
        use gotcha::oas::{SecurityScheme, SecurityType};

        let components = self.components.get_or_insert_with(default_components);
        components
            .security_schemes
            .get_or_insert_with(BTreeMap::new)
            .entry(BEARER_SCHEME.to_string())
            .or_insert_with(|| {
                Referenceable::Data(SecurityScheme {
                    _type: SecurityType::Http {
                        scheme: "bearer".to_string(),
                        // Not a JWT and deliberately so: it is 32 random bytes
                        // with no structure, which is why the server can store
                        // nothing but its sha256 and still check it.
                        bearer_format: Some("opaque".to_string()),
                    },
                    description: Some("A token from `POST /api/auth/login`.".to_string()),
                })
            });
    }

    fn fold_nullable_refs(&mut self) {
        let Some(schemas) = self.components.as_mut().and_then(|c| c.schemas.as_mut()) else {
            return;
        };
        for schema in schemas.values_mut() {
            let Some(schema) = schema.as_object_mut() else { continue };
            let Some(serde_json::Value::Object(properties)) = schema.extras.get_mut("properties") else {
                continue;
            };
            for property in properties.values_mut() {
                fold_nullable_ref_in(property);
            }
        }
    }

    fn hoist_nullable_refs(&mut self) {
        let Some(schemas) = self.components.as_mut().and_then(|c| c.schemas.as_mut()) else {
            return;
        };

        let mut to_null: Vec<String> = Vec::new();
        for schema in schemas.values_mut() {
            let Some(schema) = schema.as_object_mut() else { continue };
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
            if let Some(schema) = schemas.get_mut(&name).and_then(gotcha::oas::SchemaValue::as_object_mut) {
                schema.nullable = Some(true);
            }
        }
    }
}

/// Normalize the generated document to the shape the checked-in contract has.
///
/// gotcha's own assembly is left alone: this registers the `bearerAuth` scheme
/// its operations refer to, folds the `anyOf`-of-null nullability back into a
/// `$ref`, hoists it onto the referenced component, and restores the
/// `required` lists that a container-level `#[serde(default)]` loosens.
pub fn normalize(mut spec: OpenAPIV3) -> OpenAPIV3 {
    spec.declare_bearer_scheme();
    spec.fold_nullable_refs();
    spec.hoist_nullable_refs();
    restore_always_written_required(&mut spec);
    spec
}

/// Fold one component property's `anyOf: [{ "$ref": X }, { "type": "null" }]`
/// back into the `$ref` plus `nullable` sibling [gotcha 0.4 wrote][hoist].
///
/// The generated TypeScript client imports component names as its own types
/// (`type AccountRef = components["schemas"]["AccountRef"]`), and the contract
/// has always meant "this component is nullable" rather than "this field may
/// be null, the component itself never is". The two describe the same wire
/// values, but only the former typechecks against code written for it.
///
/// Only a component's own properties are folded — the exact positions
/// [`SpecAmendmentExt::hoist_nullable_refs`] moves nullability from. A `$ref`
/// deeper in (a `Vec<Option<X>>` item, a nested object) keeps gotcha's
/// `anyOf`, which generators do read as `X | null`; folding it there would
/// leave a `nullable` sibling no generator reads. Literal-value keys
/// (`examples`, `default`, `enum`) are never visited either.
///
/// [hoist]: SpecAmendmentExt::hoist_nullable_refs
fn fold_nullable_ref_in(value: &mut serde_json::Value) {
    let serde_json::Value::Object(object) = value else { return };
    let Some(serde_json::Value::Array(branches)) = object.get("anyOf") else { return };
    let null = branches
        .iter()
        .position(|branch| branch.as_object().is_some_and(|branch| branch.len() == 1 && branch.get("type").and_then(|t| t.as_str()) == Some("null")));
    let reference = branches
        .iter()
        .position(|branch| branch.as_object().is_some_and(|branch| branch.len() == 1 && branch.contains_key("$ref")));
    let (Some(null), Some(reference)) = (null, reference) else { return };
    if branches.len() != 2 || null == reference {
        return;
    }
    let reference = branches[reference].clone();
    object.remove("anyOf");
    object.extend(reference.as_object().cloned().unwrap_or_default());
    object.insert("nullable".to_string(), serde_json::Value::Bool(true));
}

/// The components whose Rust types carry a container-level `#[serde(default)]`.
///
/// gotcha 0.5 reads that attribute as "every field may be absent", which is
/// right for the lenient *reader* these types implement — a hand-edited or
/// older row still loads — but wrong for the *writer*: `Serialize` always
/// emits every field, and every response built from them does too. Left as
/// gotcha derives it, each field becomes optional in the generated TypeScript
/// and the client loses the guarantee the contract used to give it.
const ALWAYS_WRITTEN: [&str; 8] = [
    "ApiChat",
    "ApiChatMessage",
    "FirstProgress",
    "SetupProgress",
    "StepFlows",
    "StepStates",
    "WhereProgress",
    "WrapProgress",
];

/// Put every property of [`ALWAYS_WRITTEN`] back into its `required` list.
fn restore_always_written_required(spec: &mut OpenAPIV3) {
    let Some(schemas) = spec.components.as_mut().and_then(|components| components.schemas.as_mut()) else {
        return;
    };
    for name in ALWAYS_WRITTEN {
        let Some(schema) = schemas.get_mut(name).and_then(gotcha::oas::SchemaValue::as_object_mut) else {
            continue;
        };
        let Some(serde_json::Value::Object(properties)) = schema.extras.get("properties") else {
            continue;
        };
        // Sorted: gotcha builds `properties` from a hash map, whose order
        // differs between runs, and this array is not an object the server's
        // final sort touches. The committed document has to be reproducible.
        let mut names: Vec<String> = properties.keys().cloned().collect();
        names.sort();
        let required = names.into_iter().map(serde_json::Value::String).collect();
        schema.extras.insert("required".to_string(), serde_json::Value::Array(required));
    }
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


#[cfg(test)]
mod tests {
    use gotcha::gotcha_core::registry;

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
        spec.hoist_nullable_refs();

        let schemas = spec.components.unwrap().schemas.unwrap();
        let Some(outer) = schemas["Outer"].as_object() else {
            panic!("Outer inlined")
        };
        let properties = outer.extras["properties"].as_object().unwrap();
        let inner = properties["inner"].as_object().unwrap();
        assert!(inner.contains_key("$ref"), "the field stays a $ref");
        assert!(!inner.contains_key("nullable"), "a $ref sibling is ignored by generators");

        let Some(inner_component) = schemas["Inner"].as_object() else {
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
                schemas: Some(components.into_iter().map(|(k, v)| (k, v.into())).collect()),
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
