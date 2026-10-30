//! Evaluation of the `rules` arrays found in version JSON files.
//!
//! A rule set is "allowed" when the last matching rule says `allow`. If there
//! are rules but none match, the element is disallowed.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Rule {
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os: Option<OsRule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub features: Option<HashMap<String, bool>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct OsRule {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arch: Option<String>,
}

/// The environment rules are evaluated against.
#[derive(Debug, Clone)]
pub struct RuleContext {
    pub os_name: String,
    /// `x86`, `x86_64`, `arm64`...
    pub os_arch: String,
    pub os_version: String,
    pub features: HashMap<String, bool>,
}

impl RuleContext {
    pub fn current() -> Self {
        let arch = match std::env::consts::ARCH {
            "x86" => "x86",
            "x86_64" => "x86_64",
            "aarch64" => "arm64",
            other => other,
        };
        Self {
            os_name: crate::paths::os_name().to_string(),
            os_arch: arch.to_string(),
            os_version: os_version(),
            features: HashMap::new(),
        }
    }

    pub fn with_feature(mut self, name: &str, value: bool) -> Self {
        self.features.insert(name.to_string(), value);
        self
    }
}

fn os_version() -> String {
    sysinfo::System::os_version().unwrap_or_default()
}

impl Rule {
    pub fn matches(&self, ctx: &RuleContext) -> bool {
        if let Some(os) = &self.os {
            if let Some(name) = &os.name {
                if name != &ctx.os_name {
                    return false;
                }
            }
            if let Some(arch) = &os.arch {
                // Mojang uses "x86" to mean 32-bit; be lenient with other names.
                let want = arch.as_str();
                let have = ctx.os_arch.as_str();
                let ok = want == have
                    || (want == "x86" && have == "i686")
                    || (want == "x64" && have == "x86_64")
                    || (want == "arm64" && have == "aarch64");
                if !ok {
                    return false;
                }
            }
            if let Some(ver) = &os.version {
                if !version_matches(ver, &ctx.os_version) {
                    return false;
                }
            }
        }
        if let Some(features) = &self.features {
            for (k, v) in features {
                if ctx.features.get(k).copied().unwrap_or(false) != *v {
                    return false;
                }
            }
        }
        true
    }
}

/// Mojang's version rules are regexes such as `^10\\.`; we support the
/// anchored-prefix form that is actually used, falling back to substring.
fn version_matches(pattern: &str, version: &str) -> bool {
    let p = pattern.replace("\\.", ".");
    if let Some(prefix) = p.strip_prefix('^') {
        let prefix = prefix.trim_end_matches('$');
        version.starts_with(prefix)
    } else {
        version.contains(&p)
    }
}

/// Evaluate a (possibly empty) rule list.
pub fn allowed(rules: &[Rule], ctx: &RuleContext) -> bool {
    if rules.is_empty() {
        return true;
    }
    let mut result = false;
    for r in rules {
        if r.matches(ctx) {
            result = r.action == "allow";
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(os: &str) -> RuleContext {
        RuleContext {
            os_name: os.into(),
            os_arch: "x86_64".into(),
            os_version: "10.0".into(),
            features: HashMap::new(),
        }
    }

    #[test]
    fn empty_rules_allow() {
        assert!(allowed(&[], &ctx("linux")));
    }

    #[test]
    fn os_specific() {
        let rules = vec![Rule {
            action: "allow".into(),
            os: Some(OsRule {
                name: Some("osx".into()),
                ..Default::default()
            }),
            features: None,
        }];
        assert!(allowed(&rules, &ctx("osx")));
        assert!(!allowed(&rules, &ctx("linux")));
    }

    #[test]
    fn allow_then_disallow() {
        let rules = vec![
            Rule {
                action: "allow".into(),
                os: None,
                features: None,
            },
            Rule {
                action: "disallow".into(),
                os: Some(OsRule {
                    name: Some("osx".into()),
                    ..Default::default()
                }),
                features: None,
            },
        ];
        assert!(allowed(&rules, &ctx("windows")));
        assert!(!allowed(&rules, &ctx("osx")));
    }

    #[test]
    fn features() {
        let mut f = HashMap::new();
        f.insert("is_demo_user".to_string(), true);
        let rules = vec![Rule {
            action: "allow".into(),
            os: None,
            features: Some(f),
        }];
        assert!(!allowed(&rules, &ctx("linux")));
        assert!(allowed(
            &rules,
            &ctx("linux").with_feature("is_demo_user", true)
        ));
    }

    #[test]
    fn windows_version_regex() {
        let rules = vec![Rule {
            action: "allow".into(),
            os: Some(OsRule {
                name: Some("windows".into()),
                version: Some("^10\\.".into()),
                arch: None,
            }),
            features: None,
        }];
        assert!(allowed(&rules, &ctx("windows")));
        let mut c = ctx("windows");
        c.os_version = "6.1".into();
        assert!(!allowed(&rules, &c));
    }
}
