use std::fmt;

fn validate_ref(value: impl Into<String>) -> Result<String, &'static str> {
    let value = value.into();
    if value.trim().is_empty() {
        return Err("reference must not be empty");
    }
    Ok(value)
}

/// Opaque identity supplied and owned by a semantic client.
///
/// Workcell preserves this value for provenance but never parses it for domain
/// meaning. Wire encoding compatibility is an interoperability concern, not a
/// reason for Workcell to own the referenced semantic object.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ExternalRef(String);

impl ExternalRef {
    pub fn new(value: impl Into<String>) -> Result<Self, &'static str> {
        validate_ref(value).map(Self)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ExternalRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

macro_rules! nonempty_ref {
    ($name:ident) => {
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, &'static str> {
                validate_ref(value).map(Self)
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

nonempty_ref!(WorkcellRef);
nonempty_ref!(DemandRef);
nonempty_ref!(OfferRef);
nonempty_ref!(ProviderRef);
nonempty_ref!(PlanRef);
nonempty_ref!(WorldRef);
nonempty_ref!(BindingRef);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_refs_are_opaque_and_nonempty() {
        let value = ExternalRef::new("project/client-owned/anything").unwrap();
        assert_eq!(value.as_str(), "project/client-owned/anything");
        assert!(ExternalRef::new(" ").is_err());
    }
}
