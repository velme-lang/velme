//! The built-in type names (`language/11` §2).

/// A built-in type name. `List` is the one that takes a type in `< >`; no declaration may reuse any of them (R-TYP-19).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BuiltinType {
    /// `Number`
    Number,
    /// `Text`
    Text,
    /// `Boolean`
    Boolean,
    /// `Nothing`
    Nothing,
    /// `List`
    List,
}

impl BuiltinType {
    /// Every built-in type name.
    pub const ALL: [BuiltinType; 5] = [
        BuiltinType::Number,
        BuiltinType::Text,
        BuiltinType::Boolean,
        BuiltinType::Nothing,
        BuiltinType::List,
    ];

    /// The name as written in source.
    pub fn as_str(self) -> &'static str {
        match self {
            BuiltinType::Number => "Number",
            BuiltinType::Text => "Text",
            BuiltinType::Boolean => "Boolean",
            BuiltinType::Nothing => "Nothing",
            BuiltinType::List => "List",
        }
    }

    /// The built-in type called `name`, if any.
    pub fn from_name(name: &str) -> Option<BuiltinType> {
        BuiltinType::ALL.into_iter().find(|t| t.as_str() == name)
    }
}
