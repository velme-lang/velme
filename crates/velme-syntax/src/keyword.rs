//! Keywords and reserved words (`language/10` §2.2, D-24).

use std::fmt;

macro_rules! keywords {
    (keywords: [$($kw:ident = $kws:literal,)+] reserved: [$($rw:ident = $rws:literal,)+]) => {
        /// A v0.1 keyword or a reserved word (`language/10` §2.2, CC-CONST-03).
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum Keyword {
            $(
                #[doc = concat!("`", $kws, "`")]
                $kw,
            )+
            $(
                #[doc = concat!("`", $rws, "`, reserved (D-24)")]
                $rw,
            )+
        }

        impl Keyword {
            /// Every keyword and reserved word.
            pub const ALL: &[Keyword] = &[$(Keyword::$kw,)+ $(Keyword::$rw,)+];

            /// The word as written in source.
            pub fn as_str(self) -> &'static str {
                match self {
                    $(Keyword::$kw => $kws,)+
                    $(Keyword::$rw => $rws,)+
                }
            }

            /// Whether this is a reserved word (D-24): lexed, then rejected with `VL0104`.
            pub fn is_reserved(self) -> bool {
                match self {
                    $(Keyword::$kw => false,)+
                    $(Keyword::$rw => true,)+
                }
            }

            /// The keyword spelled `word`, if any.
            pub fn from_word(word: &str) -> Option<Keyword> {
                match word {
                    $($kws => Some(Keyword::$kw),)+
                    $($rws => Some(Keyword::$rw),)+
                    _ => None,
                }
            }
        }
    };
}

keywords! {
    keywords: [
    Language = "language",
    Type = "type",
    Goal = "goal",
    Call = "call",
    Plan = "plan",
    Check = "check",
    Examples = "examples",
    Budget = "budget",
    And = "and",
    Or = "or",
    Not = "not",
    If = "if",
    Then = "then",
    Every = "every",
    Some = "some",
    In = "in",
    Has = "has",
    Is = "is",
    Empty = "empty",
    Nothing = "nothing",
    True = "true",
    False = "false",
    Result = "result",
    ]
    reserved: [
    Pure = "pure",
    Effects = "effects",
    When = "when",
    Choose = "choose",
    Otherwise = "otherwise",
    Import = "import",
    Module = "module",
    Fallback = "fallback",
    Retry = "retry",
    Optional = "optional",
    Assume = "assume",
    ]
}

impl fmt::Display for Keyword {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
