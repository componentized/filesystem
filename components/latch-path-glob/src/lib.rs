use std::cell::RefCell;
use std::path::Path;

use glob::{MatchOptions, Pattern};

use crate::{
    componentized::filesystem::latch::{Decision::Deferred, DescriptorOperation},
    exports::componentized::filesystem::latch::{
        Decision, ErrorCode, FilesystemErrorCode, Guest as Latch, Operation, PreopensOperation,
    },
    wasi::logging::logging::{log, Level},
};

const LATCH_NAME: &str = "latch-path-glob";

struct PathGlobLatch {}

/// Whether the host filesystem is case sensitive is unknown, matching is case insensitive unless
/// the config turns case sensitivity on.
fn match_options(case_sensitive: bool) -> MatchOptions {
    MatchOptions {
        case_sensitive,
        // a single `*` must not match across path segments, `**` spans them
        require_literal_separator: true,
        require_literal_leading_dot: false,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Action {
    // ordered so that deny sorts first when patterns are equally specific
    Deny,
    Defer,
}

struct Rule {
    pattern: Pattern,
    specificity: Vec<(u8, usize)>,
    action: Action,
}

impl Rule {
    fn new(pattern: &str, action: Action) -> Result<Rule, String> {
        Ok(Rule {
            pattern: Pattern::new(pattern).map_err(|err| err.to_string())?,
            specificity: specificity(pattern),
            action,
        })
    }
}

/// Rank each segment of a pattern, starting with the segment closest to the root.
///
/// Rankings compare segment by segment, the first segment that differs decides which pattern is
/// more specific. Within a segment, a literal beats a partial wildcard (more literal characters
/// win), which beats `*`, which beats `**`.
fn specificity(pattern: &str) -> Vec<(u8, usize)> {
    pattern
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(|segment| match segment {
            "**" => (0, 0),
            "*" => (2, 0),
            _ if segment.contains(['*', '?', '[']) => (3, literal_chars(segment)),
            _ => (4, segment.len()),
        })
        .collect()
}

/// A pattern that has no more segments only matches when the path also has no more segments,
/// which is more specific than `**` but less specific than any other segment.
const END_OF_PATTERN: (u8, usize) = (1, 0);

fn literal_chars(segment: &str) -> usize {
    let mut count = 0;
    let mut in_class = false;
    for c in segment.chars() {
        match c {
            '[' => in_class = true,
            ']' => in_class = false,
            '*' | '?' => {}
            _ if !in_class => count += 1,
            _ => {}
        }
    }
    count
}

fn compare_specificity(a: &[(u8, usize)], b: &[(u8, usize)]) -> std::cmp::Ordering {
    (0..a.len().max(b.len()))
        .map(|i| {
            let a = a.get(i).unwrap_or(&END_OF_PATTERN);
            let b = b.get(i).unwrap_or(&END_OF_PATTERN);
            a.cmp(b)
        })
        .find(|ordering| ordering.is_ne())
        .unwrap_or(std::cmp::Ordering::Equal)
}

struct Patterns {
    /// most specific first
    rules: Vec<Rule>,
    reason: FilesystemErrorCode,
    default: Action,
    match_options: MatchOptions,
}

impl Patterns {
    /// Load the patterns from config, logging why when the config is invalid.
    fn load() -> Result<Patterns, ErrorCode> {
        wasi::config::store::get_all()
            .map_err(|err| match err {
                wasi::config::store::Error::Upstream(message) => {
                    format!("ERROR=upstream: {message}")
                }
                wasi::config::store::Error::Io(message) => format!("ERROR=io: {message}"),
            })
            .and_then(Patterns::parse)
            .map_err(|message| {
                log(
                    Level::Critical,
                    "componentized-latch",
                    &format!("Invalid config LATCH={LATCH_NAME} {message}"),
                );
                ErrorCode::InvalidConfig(LATCH_NAME.to_string())
            })
    }

    fn parse(config: Vec<(String, String)>) -> Result<Patterns, String> {
        let mut rules: Vec<Rule> = vec![];
        let mut reason = FilesystemErrorCode::NotPermitted;
        let mut default = Action::Defer;
        let mut case_sensitive = false;

        for (key, value) in config {
            let invalid = |err: String| format!("KEY={key} VALUE={value} ERROR={err}");
            if key.starts_with("deny") {
                rules.push(Rule::new(&value, Action::Deny).map_err(invalid)?);
            } else if key.starts_with("defer") {
                rules.push(Rule::new(&value, Action::Defer).map_err(invalid)?);
            } else if key == "reason" {
                reason = get_error_code(value).unwrap_or(FilesystemErrorCode::NotPermitted);
            } else if key == "default" {
                default = match value.as_str() {
                    "deny" => Action::Deny,
                    "defer" => Action::Defer,
                    _ => return Err(invalid("expected 'deny' or 'defer'".to_string())),
                }
            } else if key == "case-sensitive" {
                case_sensitive = match value.as_str() {
                    "true" => true,
                    "false" => false,
                    _ => return Err(invalid("expected 'true' or 'false'".to_string())),
                }
            }
        }

        rules.sort_by(|a, b| {
            compare_specificity(&b.specificity, &a.specificity).then(a.action.cmp(&b.action))
        });

        Ok(Patterns {
            rules,
            reason,
            default,
            match_options: match_options(case_sensitive),
        })
    }

    fn authorize(path: String, paths: Vec<String>) -> Result<Decision, ErrorCode> {
        // config is loaded once, an invalid config fails every authorization
        STATE.with_borrow_mut(|state| match state.get_or_insert_with(Patterns::load) {
            Ok(patterns) => Ok(patterns.decide(&path, &paths)),
            Err(err) => Err(err.clone()),
        })
    }

    /// Deny when the base path, or any of the paths relative to it, is denied.
    ///
    /// Each path is decided by the most specific matching pattern, or the default when no pattern
    /// matches.
    fn decide(&self, path: &str, paths: &[String]) -> Decision {
        let base = Path::new(path);
        let denied = match paths {
            [] => self.action(base) == Action::Deny,
            paths => paths
                .iter()
                .any(|p| self.action(&base.join(p)) == Action::Deny),
        };
        match denied {
            true => Decision::Denied(self.reason.clone()),
            false => Decision::Deferred,
        }
    }

    fn action(&self, path: &Path) -> Action {
        self.authorize_path(path).unwrap_or(self.default)
    }

    /// The action of the most specific matching pattern.
    fn authorize_path(&self, path: &Path) -> Option<Action> {
        self.rules
            .iter()
            .find(|rule| rule.pattern.matches_path_with(path, self.match_options))
            .map(|rule| rule.action)
    }
}

thread_local! {
    /// `None` until the config is loaded. Components are single threaded, this is the latch's only
    /// state.
    static STATE: RefCell<Option<Result<Patterns, ErrorCode>>> = const { RefCell::new(None) };
}

impl Latch for PathGlobLatch {
    fn authorize(operation: Operation) -> Result<Decision, ErrorCode> {
        match operation {
            Operation::Preopens(preopens_operation) => match preopens_operation {
                PreopensOperation::GetDirectories => Ok(Decision::Deferred),
                PreopensOperation::GetDirectoriesItem((_, path)) => {
                    Patterns::authorize(path, vec![])
                }
            },
            Operation::Descriptor(descriptor_operation) => match descriptor_operation {
                DescriptorOperation::ReadViaStream(_) => Ok(Deferred),
                DescriptorOperation::WriteViaStream(_) => Ok(Deferred),
                DescriptorOperation::AppendViaStream(_) => Ok(Deferred),
                DescriptorOperation::Advise(_) => Ok(Deferred),
                DescriptorOperation::SyncData(_) => Ok(Deferred),
                DescriptorOperation::GetFlags(_) => Ok(Deferred),
                DescriptorOperation::GetType(_) => Ok(Deferred),
                DescriptorOperation::SetSize(_) => Ok(Deferred),
                DescriptorOperation::SetTimes(_) => Ok(Deferred),
                DescriptorOperation::ReadDirectory(_) => Ok(Deferred),
                DescriptorOperation::ReadDirectoryEntry((_, path, entry)) => {
                    Patterns::authorize(path, vec![entry.name])
                }
                DescriptorOperation::Sync(_) => Ok(Deferred),
                DescriptorOperation::CreateDirectoryAt((_, path, args)) => {
                    Patterns::authorize(path, vec![args.path])
                }
                DescriptorOperation::Stat(_) => Ok(Deferred),
                DescriptorOperation::StatAt((_, path, args)) => {
                    Patterns::authorize(path, vec![args.path])
                }
                DescriptorOperation::SetTimesAt((_, path, args)) => {
                    Patterns::authorize(path, vec![args.path])
                }
                DescriptorOperation::LinkAt((_, path, args)) => {
                    Patterns::authorize(path, vec![args.old_path, args.new_path])
                }
                DescriptorOperation::OpenAt((_, path, args)) => {
                    Patterns::authorize(path, vec![args.path])
                }
                DescriptorOperation::ReadlinkAt((_, path, args)) => {
                    Patterns::authorize(path, vec![args.path])
                }
                DescriptorOperation::RemoveDirectoryAt((_, path, args)) => {
                    Patterns::authorize(path, vec![args.path])
                }
                DescriptorOperation::RenameAt((_, path, args)) => {
                    Patterns::authorize(path, vec![args.old_path, args.new_path])
                }
                DescriptorOperation::SymlinkAt((_, path, args)) => {
                    Patterns::authorize(path, vec![args.old_path, args.new_path])
                }
                DescriptorOperation::UnlinkFileAt((_, path, args)) => {
                    Patterns::authorize(path, vec![args.path])
                }
                DescriptorOperation::MetadataHash(_) => Ok(Deferred),
                DescriptorOperation::MetadataHashAt((_, path, args)) => {
                    Patterns::authorize(path, vec![args.path])
                }
            },
        }
    }

    #[doc = " Observe the final decision for an operation, after `authorize` returned a decision."]
    #[doc = ""]
    #[doc = " Called for both deferred and denied operations, including operations this latch was not"]
    #[doc = " asked to authorize because an aggregated latch denied them first. The final decision is"]
    #[doc = " what the gate enforces, so this is where a latch updates any state it keeps, for example"]
    #[doc = " remembering peers the caller is allowed to exchange traffic with. Not called when"]
    #[doc = " `authorize` returned an error."]
    #[doc = ""]
    #[doc = " An error fails the operation, since the latch could not act on the decision."]
    #[allow(async_fn_in_trait)]
    fn observe_decision(
        _final_decision: Decision,
        _operation: Operation<'_>,
    ) -> Result<(), ErrorCode> {
        Ok(())
    }
}

/// The reason to deny with, `None` when the value is empty. Any value that is not the name of an
/// `error-code` case is `other` with the value as the message.
fn get_error_code(value: String) -> Option<FilesystemErrorCode> {
    match value.as_str() {
        "" => None,
        "other" => Some(FilesystemErrorCode::Other(None)),
        // error_code_from_name is defined by build.rs
        name => Some(error_code_from_name(name).unwrap_or(FilesystemErrorCode::Other(Some(value)))),
    }
}

include!(concat!(env!("OUT_DIR"), "/error_code.rs"));

wit_bindgen::generate!({
    path: "../wit",
    world: "latch",
    merge_structurally_equal_types: true,
    generate_all
});

export!(PathGlobLatch);

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(config: &[(&str, &str)]) -> Result<Patterns, String> {
        Patterns::parse(
            config
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        )
    }

    fn patterns(config: &[(&str, &str)]) -> Patterns {
        parse(config).expect("valid config")
    }

    fn decision(patterns: &Patterns, path: &str, paths: &[&str]) -> String {
        let paths: Vec<String> = paths.iter().map(|p| p.to_string()).collect();
        format!("{:?}", patterns.decide(path, &paths))
    }

    fn denied(reason: FilesystemErrorCode) -> String {
        format!("{:?}", Decision::Denied(reason))
    }

    fn deferred() -> String {
        format!("{:?}", Decision::Deferred)
    }

    #[test]
    fn matching_base_path_is_denied() {
        let patterns = patterns(&[("deny", "/secret")]);
        assert_eq!(
            decision(&patterns, "/secret", &[]),
            denied(FilesystemErrorCode::NotPermitted)
        );
        assert_eq!(decision(&patterns, "/", &[]), deferred());
    }

    #[test]
    fn relative_paths_are_joined_to_the_base_path() {
        let patterns = patterns(&[("deny", "/data/*.key")]);
        assert_eq!(
            decision(&patterns, "/data", &["id.key"]),
            denied(FilesystemErrorCode::NotPermitted)
        );
        assert_eq!(decision(&patterns, "/data", &["id.pub"]), deferred());
        // only the joined paths are matched, not the base path alone
        assert_eq!(decision(&patterns, "/data/id.key", &["x"]), deferred());
    }

    #[test]
    fn any_matching_path_denies() {
        let patterns = patterns(&[("deny", "/data/*.key")]);
        assert_eq!(
            decision(&patterns, "/data", &["id.pub", "id.key"]),
            denied(FilesystemErrorCode::NotPermitted)
        );
    }

    #[test]
    fn multiple_deny_keys() {
        let patterns = patterns(&[("deny-1", "/a"), ("deny-2", "/b")]);
        let not_permitted = denied(FilesystemErrorCode::NotPermitted);
        assert_eq!(decision(&patterns, "/a", &[]), not_permitted);
        assert_eq!(decision(&patterns, "/b", &[]), not_permitted);
        assert_eq!(decision(&patterns, "/c", &[]), deferred());
    }

    #[test]
    fn unrelated_keys_are_ignored() {
        let patterns = patterns(&[("other", "/a")]);
        assert_eq!(decision(&patterns, "/a", &[]), deferred());
    }

    #[test]
    fn denies_with_configured_reason() {
        for (value, expected) in [
            ("access", FilesystemErrorCode::Access),
            ("read-only", FilesystemErrorCode::ReadOnly),
            ("other", FilesystemErrorCode::Other(None)),
            ("", FilesystemErrorCode::NotPermitted),
            (
                "custom-reason",
                FilesystemErrorCode::Other(Some("custom-reason".to_string())),
            ),
        ] {
            let patterns = patterns(&[("reason", value), ("deny", "/a")]);
            assert_eq!(
                decision(&patterns, "/a", &[]),
                denied(expected),
                "reason={value}"
            );
        }
    }

    fn action(patterns: &Patterns, path: &str) -> Option<Action> {
        patterns.authorize_path(Path::new(path))
    }

    #[test]
    fn single_star_matches_within_a_segment() {
        let patterns = patterns(&[("deny", "/data/*")]);
        assert_eq!(action(&patterns, "/data/file.txt"), Some(Action::Deny));
        assert_eq!(action(&patterns, "/data/dir/file.txt"), None);
        assert_eq!(action(&patterns, "/other/file.txt"), None);
    }

    #[test]
    fn double_star_spans_segments() {
        let patterns = patterns(&[("deny", "/data/**")]);
        assert_eq!(action(&patterns, "/data/file.txt"), Some(Action::Deny));
        assert_eq!(action(&patterns, "/data/dir/file.txt"), Some(Action::Deny));
        assert_eq!(action(&patterns, "/other/file.txt"), None);
    }

    #[test]
    fn matching_is_case_insensitive_by_default() {
        let patterns = patterns(&[("deny", "/Data/*.TXT")]);
        assert_eq!(action(&patterns, "/Data/file.TXT"), Some(Action::Deny));
        assert_eq!(action(&patterns, "/data/file.txt"), Some(Action::Deny));
        assert_eq!(action(&patterns, "/DATA/FILE.txt"), Some(Action::Deny));
    }

    #[test]
    fn matching_is_case_sensitive_when_configured() {
        for config in [
            [("case-sensitive", "true"), ("deny", "/Data/*")],
            [("deny", "/Data/*"), ("case-sensitive", "true")],
        ] {
            let patterns = patterns(&config);
            assert_eq!(action(&patterns, "/Data/file.txt"), Some(Action::Deny));
            assert_eq!(action(&patterns, "/data/file.txt"), None);
        }
        let patterns = patterns(&[("case-sensitive", "false"), ("deny", "/Data/*")]);
        assert_eq!(action(&patterns, "/data/file.txt"), Some(Action::Deny));
    }

    #[test]
    fn unknown_case_sensitive_is_invalid() {
        assert_eq!(
            parse(&[("case-sensitive", "yes")]).err().unwrap(),
            "KEY=case-sensitive VALUE=yes ERROR=expected 'true' or 'false'"
        );
    }

    #[test]
    fn more_specific_defer_overrides_general_deny() {
        for config in [
            [("deny", "/data/**"), ("defer", "/data/public/*")],
            [("defer", "/data/public/*"), ("deny", "/data/**")],
        ] {
            let patterns = patterns(&config);
            assert_eq!(action(&patterns, "/data/public/a.txt"), Some(Action::Defer));
            assert_eq!(action(&patterns, "/data/private/a.txt"), Some(Action::Deny));
        }
    }

    #[test]
    fn more_specific_deny_overrides_general_defer() {
        for config in [
            [("defer", "/data/**"), ("deny", "/data/*.key")],
            [("deny", "/data/*.key"), ("defer", "/data/**")],
        ] {
            let patterns = patterns(&config);
            assert_eq!(action(&patterns, "/data/id.key"), Some(Action::Deny));
            assert_eq!(action(&patterns, "/data/id.pub"), Some(Action::Defer));
        }
    }

    #[test]
    fn nested_exceptions() {
        let patterns = patterns(&[
            ("deny-1", "/**"),
            ("defer-1", "/data/**"),
            ("deny-2", "/data/secret.txt"),
        ]);
        assert_eq!(action(&patterns, "/etc/passwd"), Some(Action::Deny));
        assert_eq!(action(&patterns, "/data/a.txt"), Some(Action::Defer));
        assert_eq!(action(&patterns, "/data/secret.txt"), Some(Action::Deny));
    }

    #[test]
    fn segments_closer_to_the_root_are_more_significant() {
        // `data` is a literal where the other pattern has a wildcard
        let patterns = patterns(&[("deny", "/*/a.txt"), ("defer", "/data/*")]);
        assert_eq!(action(&patterns, "/data/a.txt"), Some(Action::Defer));
    }

    #[test]
    fn literal_segment_beats_wildcards() {
        let star = patterns(&[("deny", "/*"), ("defer", "/a.txt")]);
        assert_eq!(action(&star, "/a.txt"), Some(Action::Defer));
        assert_eq!(action(&star, "/b.txt"), Some(Action::Deny));

        let partial = patterns(&[("deny", "/a*"), ("defer", "/a.txt")]);
        assert_eq!(action(&partial, "/a.txt"), Some(Action::Defer));
        assert_eq!(action(&partial, "/ab.txt"), Some(Action::Deny));
    }

    #[test]
    fn partial_wildcard_with_more_literals_wins() {
        let patterns = patterns(&[("deny", "/*.txt"), ("defer", "/read*.txt")]);
        assert_eq!(action(&patterns, "/readme.txt"), Some(Action::Defer));
        assert_eq!(action(&patterns, "/notes.txt"), Some(Action::Deny));
    }

    #[test]
    fn single_star_beats_double_star() {
        let patterns = patterns(&[("deny", "/data/**"), ("defer", "/data/*")]);
        assert_eq!(action(&patterns, "/data/a.txt"), Some(Action::Defer));
        assert_eq!(action(&patterns, "/data/dir/a.txt"), Some(Action::Deny));
    }

    #[test]
    fn deny_wins_when_equally_specific() {
        for config in [
            [("deny", "/a*"), ("defer", "/*z")],
            [("defer", "/*z"), ("deny", "/a*")],
        ] {
            let patterns = patterns(&config);
            assert_eq!(action(&patterns, "/abz"), Some(Action::Deny));
        }
    }

    #[test]
    fn specificity_ranks_segments_from_the_root() {
        assert_eq!(specificity("/**/re?d*.txt/*"), vec![(0, 0), (3, 7), (2, 0)]);
        assert_eq!(specificity("[abc]x/data"), vec![(3, 1), (4, 4)]);
    }

    #[test]
    fn a_deferred_path_does_not_hide_a_denied_one() {
        let patterns = patterns(&[("default", "deny"), ("defer", "/data/*")]);
        assert_eq!(
            decision(&patterns, "/data", &["a.txt", "b.txt"]),
            deferred()
        );
        assert_eq!(
            decision(&patterns, "/data", &["a.txt", "../etc/passwd"]),
            denied(FilesystemErrorCode::NotPermitted)
        );
    }

    #[test]
    fn default_deny_uses_configured_reason_regardless_of_order() {
        for config in [
            [("reason", "access"), ("default", "deny")],
            [("default", "deny"), ("reason", "access")],
        ] {
            let patterns = patterns(&config);
            assert_eq!(
                decision(&patterns, "/a", &[]),
                denied(FilesystemErrorCode::Access)
            );
        }
    }

    #[test]
    fn reason_does_not_change_the_default() {
        let patterns = patterns(&[("reason", "access")]);
        assert_eq!(decision(&patterns, "/a", &[]), deferred());
    }

    #[test]
    fn default_defer() {
        let patterns = patterns(&[("default", "defer")]);
        assert_eq!(decision(&patterns, "/a", &[]), deferred());
    }

    #[test]
    fn matching_pattern_overrides_default() {
        let patterns = patterns(&[("default", "deny"), ("defer", "/data/**")]);
        assert_eq!(decision(&patterns, "/data/a.txt", &[]), deferred());
        assert_eq!(
            decision(&patterns, "/etc/passwd", &[]),
            denied(FilesystemErrorCode::NotPermitted)
        );
    }

    #[test]
    fn unknown_default_is_invalid() {
        assert_eq!(
            parse(&[("default", "grant")]).err().unwrap(),
            "KEY=default VALUE=grant ERROR=expected 'deny' or 'defer'"
        );
    }

    #[test]
    fn every_error_code_is_parsed_by_name() {
        for (name, code) in [
            ("not-permitted", FilesystemErrorCode::NotPermitted),
            ("access", FilesystemErrorCode::Access),
            ("cross-device", FilesystemErrorCode::CrossDevice),
            (
                "illegal-byte-sequence",
                FilesystemErrorCode::IllegalByteSequence,
            ),
            ("other", FilesystemErrorCode::Other(None)),
            (
                "i-am-a-teapot",
                FilesystemErrorCode::Other(Some("i-am-a-teapot".to_string())),
            ),
        ] {
            assert_eq!(
                format!("{:?}", get_error_code(name.to_string())),
                format!("{:?}", Some(code)),
                "{name}"
            );
        }
        assert!(get_error_code(String::new()).is_none());
    }

    #[test]
    fn invalid_glob_is_invalid() {
        let err = parse(&[("deny-1", "/a"), ("deny-2", "[")]).err().unwrap();
        assert!(err.starts_with("KEY=deny-2 VALUE=[ ERROR="), "{err}");
    }
}
