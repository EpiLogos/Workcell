use std::fmt;

macro_rules! nonempty_ref {
    ($name:ident) => {
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, &'static str> {
                let value = value.into();
                if value.trim().is_empty() {
                    return Err("reference must not be empty");
                }
                Ok(Self(value))
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

/// Opaque identity supplied and owned by a semantic client.
///
/// Workcell preserves this value for provenance but never parses it for domain
/// meaning. Wire encoding compatibility is an interoperability concern, not a
/// reason for Workcell to own the referenced semantic object.
nonempty_ref!(ExternalRef);

nonempty_ref!(WorkcellRef);
nonempty_ref!(DemandRef);
nonempty_ref!(OfferRef);
nonempty_ref!(ProviderRef);
nonempty_ref!(PlanRef);
nonempty_ref!(WorldRef);
nonempty_ref!(BindingRef);
