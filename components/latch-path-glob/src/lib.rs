use std::cell::RefCell;
use std::path::{Path, PathBuf};

use glob::{MatchOptions, Pattern};

use filesystem_latch::{
    Decision, DescriptorFlags, DescriptorOperation, ErrorCode, FilesystemErrorCode, Latch, OpenFlags,
    Operation, PreopensOperation,
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

/// Whether an operation only reads the filesystem, or may change it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Access {
    Read,
    Mutate,
}

/// Whether opening a file with the flags would let the descriptor change the filesystem.
fn open_access(open_flags: OpenFlags, flags: DescriptorFlags) -> Access {
    let mutating_open = OpenFlags::CREATE | OpenFlags::EXCLUSIVE | OpenFlags::TRUNCATE;
    let mutating_flags = DescriptorFlags::WRITE
        | DescriptorFlags::FILE_INTEGRITY_SYNC
        | DescriptorFlags::DATA_INTEGRITY_SYNC
        | DescriptorFlags::REQUESTED_WRITE_SYNC
        | DescriptorFlags::MUTATE_DIRECTORY;
    match open_flags.intersects(mutating_open) || flags.intersects(mutating_flags) {
        true => Access::Mutate,
        false => Access::Read,
    }
}

struct Patterns {
    /// most specific first
    rules: Vec<Rule>,
    /// paths that may be read but not changed, when the rules defer
    readonly: Vec<Pattern>,
    reason: FilesystemErrorCode,
    default: Action,
    match_options: MatchOptions,
}

impl Patterns {
    /// Load the patterns from config, logging why when the config is invalid.
    fn load() -> Result<Patterns, ErrorCode> {
        filesystem_latch::load_config(LATCH_NAME, Patterns::parse)
    }

    fn parse(config: Vec<(String, String)>) -> Result<Patterns, String> {
        let mut rules: Vec<Rule> = vec![];
        let mut readonly: Vec<Pattern> = vec![];
        let mut reason = FilesystemErrorCode::Access;
        let mut default = Action::Defer;
        let mut case_sensitive = false;

        for (key, value) in config {
            let invalid = |err: String| format!("KEY={key} VALUE={value} ERROR={err}");
            if key.starts_with("deny") {
                rules.push(Rule::new(&value, Action::Deny).map_err(invalid)?);
            } else if key.starts_with("defer") {
                rules.push(Rule::new(&value, Action::Defer).map_err(invalid)?);
            } else if key.starts_with("readonly") {
                readonly.push(Pattern::new(&value).map_err(|err| invalid(err.to_string()))?);
            } else if key == "reason" {
                reason = filesystem_latch::reason(value).unwrap_or(FilesystemErrorCode::Access);
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
            readonly,
            reason,
            default,
            match_options: match_options(case_sensitive),
        })
    }

    /// Decide an operation on the base path, or on the paths relative to it.
    fn authorize(path: String, paths: Vec<String>, access: Access) -> Result<Decision, ErrorCode> {
        Patterns::with(|patterns| patterns.decide(&path, &paths, access))
    }

    /// Decide an operation that changes the descriptor at the path, without a path argument.
    ///
    /// The deny and defer patterns are not evaluated, only the readonly patterns: the descriptor was
    /// already deferred when it was opened or preopened, it can't be obtained otherwise.
    fn authorize_mutation(path: String) -> Result<Decision, ErrorCode> {
        Patterns::with(|patterns| patterns.readonly_decision(&[PathBuf::from(path)]))
    }

    fn with(decide: impl FnOnce(&Patterns) -> Decision) -> Result<Decision, ErrorCode> {
        // config is loaded once, an invalid config fails every authorization
        STATE.with_borrow_mut(|state| match state.get_or_insert_with(Patterns::load) {
            Ok(patterns) => Ok(decide(patterns)),
            Err(err) => Err(err.clone()),
        })
    }

    /// Deny when the base path, or any of the paths relative to it, is denied.
    ///
    /// Each path is decided by the most specific matching pattern, or the default when no pattern
    /// matches. An operation that is otherwise deferred and changes the filesystem is denied with
    /// `read-only` when any of its paths matches a readonly pattern.
    fn decide(&self, path: &str, paths: &[String], access: Access) -> Decision {
        let base = Path::new(path);
        let paths: Vec<PathBuf> = match paths {
            [] => vec![base.to_path_buf()],
            paths => paths.iter().map(|p| base.join(p)).collect(),
        };
        if paths.iter().any(|p| self.action(p) == Action::Deny) {
            return Decision::Denied(self.reason.clone());
        }
        match access {
            Access::Read => Decision::Deferred,
            Access::Mutate => self.readonly_decision(&paths),
        }
    }

    /// Deny with `read-only` when any of the paths matches a readonly pattern, regardless of the
    /// configured reason.
    fn readonly_decision(&self, paths: &[PathBuf]) -> Decision {
        let readonly = paths.iter().any(|path| {
            self.readonly
                .iter()
                .any(|pattern| pattern.matches_path_with(path, self.match_options))
        });
        match readonly {
            true => Decision::Denied(FilesystemErrorCode::ReadOnly),
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
        use Access::{Mutate, Read};

        match operation {
            Operation::Preopens(preopens_operation) => match preopens_operation {
                PreopensOperation::GetDirectories => Ok(Decision::Deferred),
                PreopensOperation::GetDirectoriesItem((_, path)) => {
                    Patterns::authorize(path, vec![], Read)
                }
            },
            Operation::Descriptor(descriptor_operation) => match descriptor_operation {
                DescriptorOperation::ReadViaStream(_) => Ok(Decision::Deferred),
                DescriptorOperation::WriteViaStream((_, path, _)) => {
                    Patterns::authorize_mutation(path)
                }
                DescriptorOperation::AppendViaStream((_, path)) => {
                    Patterns::authorize_mutation(path)
                }
                DescriptorOperation::Advise(_) => Ok(Decision::Deferred),
                DescriptorOperation::SyncData((_, path)) => Patterns::authorize_mutation(path),
                DescriptorOperation::GetFlags(_) => Ok(Decision::Deferred),
                DescriptorOperation::GetType(_) => Ok(Decision::Deferred),
                DescriptorOperation::SetSize((_, path, _)) => Patterns::authorize_mutation(path),
                DescriptorOperation::SetTimes((_, path, _)) => Patterns::authorize_mutation(path),
                DescriptorOperation::ReadDirectory(_) => Ok(Decision::Deferred),
                DescriptorOperation::ReadDirectoryEntry((_, path, entry)) => {
                    Patterns::authorize(path, vec![entry.name], Read)
                }
                DescriptorOperation::Sync((_, path)) => Patterns::authorize_mutation(path),
                DescriptorOperation::CreateDirectoryAt((_, path, args)) => {
                    Patterns::authorize(path, vec![args.path], Mutate)
                }
                DescriptorOperation::Stat(_) => Ok(Decision::Deferred),
                DescriptorOperation::StatAt((_, path, args)) => {
                    Patterns::authorize(path, vec![args.path], Read)
                }
                DescriptorOperation::SetTimesAt((_, path, args)) => {
                    Patterns::authorize(path, vec![args.path], Mutate)
                }
                DescriptorOperation::LinkAt((_, path, args)) => {
                    Patterns::authorize(path, vec![args.old_path, args.new_path], Mutate)
                }
                DescriptorOperation::OpenAt((_, path, args)) => {
                    let access = open_access(args.open_flags, args.flags);
                    Patterns::authorize(path, vec![args.path], access)
                }
                DescriptorOperation::ReadlinkAt((_, path, args)) => {
                    Patterns::authorize(path, vec![args.path], Read)
                }
                DescriptorOperation::RemoveDirectoryAt((_, path, args)) => {
                    Patterns::authorize(path, vec![args.path], Mutate)
                }
                DescriptorOperation::RenameAt((_, path, args)) => {
                    Patterns::authorize(path, vec![args.old_path, args.new_path], Mutate)
                }
                DescriptorOperation::SymlinkAt((_, path, args)) => {
                    Patterns::authorize(path, vec![args.old_path, args.new_path], Mutate)
                }
                DescriptorOperation::UnlinkFileAt((_, path, args)) => {
                    Patterns::authorize(path, vec![args.path], Mutate)
                }
                DescriptorOperation::MetadataHash(_) => Ok(Decision::Deferred),
                DescriptorOperation::MetadataHashAt((_, path, args)) => {
                    Patterns::authorize(path, vec![args.path], Read)
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

filesystem_latch::export!(PathGlobLatch with_types_in filesystem_latch::bindings);

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
        decision_for(patterns, path, paths, Access::Read)
    }

    fn mutation(patterns: &Patterns, path: &str, paths: &[&str]) -> String {
        decision_for(patterns, path, paths, Access::Mutate)
    }

    fn decision_for(patterns: &Patterns, path: &str, paths: &[&str], access: Access) -> String {
        let paths: Vec<String> = paths.iter().map(|p| p.to_string()).collect();
        format!("{:?}", patterns.decide(path, &paths, access))
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
            denied(FilesystemErrorCode::Access)
        );
        assert_eq!(decision(&patterns, "/", &[]), deferred());
    }

    #[test]
    fn relative_paths_are_joined_to_the_base_path() {
        let patterns = patterns(&[("deny", "/data/*.key")]);
        assert_eq!(
            decision(&patterns, "/data", &["id.key"]),
            denied(FilesystemErrorCode::Access)
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
            denied(FilesystemErrorCode::Access)
        );
    }

    #[test]
    fn multiple_deny_keys() {
        let patterns = patterns(&[("deny-1", "/a"), ("deny-2", "/b")]);
        let access = denied(FilesystemErrorCode::Access);
        assert_eq!(decision(&patterns, "/a", &[]), access);
        assert_eq!(decision(&patterns, "/b", &[]), access);
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
            ("", FilesystemErrorCode::Access),
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
            denied(FilesystemErrorCode::Access)
        );
    }

    #[test]
    fn default_deny_uses_configured_reason_regardless_of_order() {
        for config in [
            [("reason", "no-entry"), ("default", "deny")],
            [("default", "deny"), ("reason", "no-entry")],
        ] {
            let patterns = patterns(&config);
            assert_eq!(
                decision(&patterns, "/a", &[]),
                denied(FilesystemErrorCode::NoEntry)
            );
        }
    }

    #[test]
    fn reason_does_not_change_the_default() {
        let patterns = patterns(&[("reason", "no-entry")]);
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
            denied(FilesystemErrorCode::Access)
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
    fn readonly_paths_deny_mutations_with_read_only() {
        let patterns = patterns(&[("readonly", "/data/**")]);
        let read_only = denied(FilesystemErrorCode::ReadOnly);
        assert_eq!(mutation(&patterns, "/data", &["a.txt"]), read_only);
        assert_eq!(decision(&patterns, "/data", &["a.txt"]), deferred());
        assert_eq!(mutation(&patterns, "/data/a.txt", &[]), read_only);
        assert_eq!(mutation(&patterns, "/other", &["a.txt"]), deferred());
    }

    #[test]
    fn readonly_paths_allow_reads() {
        let patterns = patterns(&[("readonly", "/data/**")]);
        assert_eq!(decision(&patterns, "/data", &["a.txt"]), deferred());
    }

    #[test]
    fn readonly_ignores_the_configured_reason() {
        let patterns = patterns(&[("reason", "no-entry"), ("readonly", "/data/**")]);
        assert_eq!(
            mutation(&patterns, "/data", &["a.txt"]),
            denied(FilesystemErrorCode::ReadOnly)
        );
    }

    #[test]
    fn readonly_is_only_evaluated_when_otherwise_deferred() {
        let denying = patterns(&[
            ("reason", "no-entry"),
            ("deny", "/data/secret/*"),
            ("readonly", "/data/**"),
        ]);
        // the deny pattern decides first, with the configured reason
        assert_eq!(
            mutation(&denying, "/data/secret", &["a.txt"]),
            denied(FilesystemErrorCode::NoEntry)
        );
        assert_eq!(
            mutation(&denying, "/data", &["a.txt"]),
            denied(FilesystemErrorCode::ReadOnly)
        );
        assert_eq!(decision(&denying, "/data", &["a.txt"]), deferred());

        let default_deny = patterns(&[("default", "deny"), ("readonly", "/data/**")]);
        assert_eq!(
            mutation(&default_deny, "/data", &["a.txt"]),
            denied(FilesystemErrorCode::Access)
        );
    }

    #[test]
    fn deferring_pattern_does_not_override_readonly() {
        let patterns = patterns(&[
            ("default", "deny"),
            ("defer", "/data/**"),
            ("readonly", "/data/archive/**"),
        ]);
        assert_eq!(mutation(&patterns, "/data", &["a.txt"]), deferred());
        assert_eq!(
            mutation(&patterns, "/data/archive", &["a.txt"]),
            denied(FilesystemErrorCode::ReadOnly)
        );
        assert_eq!(decision(&patterns, "/data/archive", &["a.txt"]), deferred());
        assert_eq!(
            mutation(&patterns, "/", &["a.txt"]),
            denied(FilesystemErrorCode::Access)
        );
    }

    #[test]
    fn any_readonly_path_denies_a_mutation() {
        let patterns = patterns(&[("readonly", "/data/archive/**")]);
        // moving a file out of a readonly directory changes it
        assert_eq!(
            mutation(&patterns, "/data", &["archive/a.txt", "a.txt"]),
            denied(FilesystemErrorCode::ReadOnly)
        );
    }

    #[test]
    fn readonly_matching_follows_case_sensitivity() {
        let insensitive = patterns(&[("readonly", "/Data/**")]);
        assert_eq!(
            mutation(&insensitive, "/data", &["a.txt"]),
            denied(FilesystemErrorCode::ReadOnly)
        );
        let sensitive = patterns(&[("case-sensitive", "true"), ("readonly", "/Data/**")]);
        assert_eq!(mutation(&sensitive, "/data", &["a.txt"]), deferred());
    }

    #[test]
    fn readonly_descriptor_mutations() {
        let patterns = patterns(&[("readonly", "/data/**")]);
        let decide =
            |path: &str| format!("{:?}", patterns.readonly_decision(&[PathBuf::from(path)]));
        assert_eq!(decide("/data/a.txt"), denied(FilesystemErrorCode::ReadOnly));
        assert_eq!(decide("/other/a.txt"), deferred());
    }

    #[test]
    fn opening_for_mutation() {
        let read = DescriptorFlags::READ;
        assert_eq!(open_access(OpenFlags::empty(), read), Access::Read);
        assert_eq!(open_access(OpenFlags::DIRECTORY, read), Access::Read);
        for open_flags in [OpenFlags::CREATE, OpenFlags::EXCLUSIVE, OpenFlags::TRUNCATE] {
            assert_eq!(
                open_access(open_flags, read),
                Access::Mutate,
                "{open_flags:?}"
            );
        }
        for flags in [
            DescriptorFlags::WRITE,
            DescriptorFlags::FILE_INTEGRITY_SYNC,
            DescriptorFlags::DATA_INTEGRITY_SYNC,
            DescriptorFlags::REQUESTED_WRITE_SYNC,
            DescriptorFlags::MUTATE_DIRECTORY,
        ] {
            assert_eq!(
                open_access(OpenFlags::empty(), read | flags),
                Access::Mutate,
                "{flags:?}"
            );
        }
    }

    #[test]
    fn invalid_readonly_glob_is_invalid() {
        let err = parse(&[("readonly", "[")]).err().unwrap();
        assert!(err.starts_with("KEY=readonly VALUE=[ ERROR="), "{err}");
    }

    #[test]
    fn double_star_matches_every_path() {
        let patterns = patterns(&[("readonly", "**")]);
        for path in ["/", "/a.txt", "/a/b/c.txt", ".", "./a.txt", "data", "data/a/b.txt", ".hidden/.x"] {
            assert_eq!(
                format!("{:?}", patterns.readonly_decision(&[PathBuf::from(path)])),
                denied(FilesystemErrorCode::ReadOnly),
                "{path}"
            );
        }
    }

    #[test]
    fn invalid_glob_is_invalid() {
        let err = parse(&[("deny-1", "/a"), ("deny-2", "[")]).err().unwrap();
        assert!(err.starts_with("KEY=deny-2 VALUE=[ ERROR="), "{err}");
    }
}
