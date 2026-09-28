use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use ulid::Ulid;

macro_rules! define_id {
    ($name:ident) => {
        #[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Ulid);

        impl $name {
            pub fn new() -> Self {
                Self(Ulid::new())
            }

            pub fn as_str(&self) -> String {
                self.0.to_string()
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl FromStr for $name {
            type Err = ulid::DecodeError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Ok(Self(Ulid::from_string(s)?))
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), self.0)
            }
        }

        // Single source of truth for schema rendering: every ID serializes
        // as a string, so use sites must NOT hand-write
        // `#[schema(value_type = String)]` / `#[schemars(with = "String")]`
        // (previously a format change silently missed half the sites).
        impl schemars::JsonSchema for $name {
            fn schema_name() -> String {
                stringify!($name).to_string()
            }

            fn json_schema(_gen: &mut schemars::gen::SchemaGenerator) -> schemars::schema::Schema {
                schemars::schema::SchemaObject {
                    instance_type: Some(schemars::schema::InstanceType::String.into()),
                    ..Default::default()
                }
                .into()
            }
        }

        impl utoipa::PartialSchema for $name {
            fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
                utoipa::openapi::ObjectBuilder::new()
                    .schema_type(utoipa::openapi::schema::Type::String)
                    .into()
            }
        }

        impl utoipa::ToSchema for $name {
            fn name() -> std::borrow::Cow<'static, str> {
                std::borrow::Cow::Borrowed(stringify!($name))
            }
        }
    };
}

define_id!(SessionId);
define_id!(MessageId);
define_id!(ToolCallId);
define_id!(PermissionId);
define_id!(EventId);
define_id!(PtyId);
define_id!(AgentId);
define_id!(LocationId);
