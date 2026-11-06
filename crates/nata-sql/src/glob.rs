//! Path patterns for `FROM '<path or glob>'` (dialect section 1): `*`, `?`, `[abc]`, `[a-z]`, `[!abc]` inside one path segment, and `**` for any number
//! of directories. The matches are the regular files, in byte order of their paths, so that file order (and `LIMIT`) is deterministic.

use std::io;
use std::path::{Path, PathBuf};

/// Whether `pattern` contains a glob character.
pub fn has_glob(pattern: &str) -> bool {
    pattern.contains(['*', '?', '['])
}

/// Does `name` match the single-segment pattern `pat` (`*`, `?`, `[...]`)?
pub fn segment_matches(pat: &str, name: &str) -> bool {
    let p: Vec<char> = pat.chars().collect();
    let n: Vec<char> = name.chars().collect();
    // iterative matching with backtracking over the last `*`
    let (mut pi, mut ni) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None;
    while ni < n.len() {
        let step = match p.get(pi) {
            Some('*') => {
                star = Some((pi, ni));
                pi += 1;
                continue;
            }
            Some('?') => Some(pi + 1),
            Some('[') => class_matches(&p, pi, n[ni]),
            Some(c) if *c == n[ni] => Some(pi + 1),
            _ => None,
        };
        match step {
            Some(next) => {
                pi = next;
                ni += 1;
            }
            None => match star {
                Some((sp, sn)) => {
                    pi = sp + 1;
                    ni = sn + 1;
                    star = Some((sp, sn + 1));
                }
                None => return false,
            },
        }
    }
    while p.get(pi) == Some(&'*') {
        pi += 1;
    }
    pi == p.len()
}

/// `[...]` at `p[start]` against `c`: the index after the class when it matches. An unclosed `[` is an ordinary character.
fn class_matches(p: &[char], start: usize, c: char) -> Option<usize> {
    let mut i = start + 1;
    let negate = matches!(p.get(i), Some('!' | '^'));
    if negate {
        i += 1;
    }
    let first = i;
    let mut hit = false;
    loop {
        match p.get(i) {
            None => return (c == '[').then_some(start + 1),
            Some(']') if i > first => break,
            Some(&lo) => {
                if p.get(i + 1) == Some(&'-') && p.get(i + 2).is_some_and(|h| *h != ']') {
                    let hi = p[i + 2];
                    hit |= lo <= c && c <= hi;
                    i += 3;
                } else {
                    hit |= lo == c;
                    i += 1;
                }
            }
        }
    }
    (hit != negate).then_some(i + 1)
}

const MAX_DEPTH: usize = 64;

/// The regular files that match `pattern`, sorted. A pattern without glob characters is returned as it is (its existence is the reader's business).
pub fn expand(pattern: &str) -> io::Result<Vec<PathBuf>> {
    if !has_glob(pattern) {
        return Ok(vec![PathBuf::from(pattern)]);
    }
    let absolute = pattern.starts_with('/');
    let segments: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
    let root = if absolute {
        PathBuf::from("/")
    } else {
        PathBuf::from(".")
    };
    let mut found: Vec<PathBuf> = Vec::new();
    walk(&root, absolute, &segments, 0, &mut found)?;
    found.sort();
    found.dedup();
    Ok(found)
}

fn join(base: &Path, bare: bool, name: &str) -> PathBuf {
    // a relative pattern keeps its relative form: no leading "./"
    if bare && base == Path::new(".") {
        PathBuf::from(name)
    } else {
        base.join(name)
    }
}

fn walk(
    base: &Path,
    absolute: bool,
    segs: &[&str],
    depth: usize,
    out: &mut Vec<PathBuf>,
) -> io::Result<()> {
    let bare = !absolute;
    let Some((seg, rest)) = segs.split_first() else {
        return Ok(());
    };
    if depth > MAX_DEPTH {
        return Ok(());
    }
    if *seg == "**" {
        // zero directories: the rest applies here; or one more directory down
        walk(base, absolute, rest, depth + 1, out)?;
        for entry in std::fs::read_dir(base)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                let name = entry.file_name();
                let sub = join(base, bare, &name.to_string_lossy());
                walk(&sub, absolute, segs, depth + 1, out)?;
            }
        }
        return Ok(());
    }
    if !has_glob(seg) {
        let next = join(base, bare, seg);
        if rest.is_empty() {
            if std::fs::metadata(&next).is_ok_and(|m| m.is_file()) {
                out.push(next);
            }
            return Ok(());
        }
        if std::fs::metadata(&next).is_ok_and(|m| m.is_dir()) {
            walk(&next, absolute, rest, depth + 1, out)?;
        }
        return Ok(());
    }
    let entries = match std::fs::read_dir(base) {
        Ok(e) => e,
        // a directory that cannot be listed (or does not exist) matches nothing
        Err(_) => return Ok(()),
    };
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !segment_matches(seg, &name) {
            continue;
        }
        let next = join(base, bare, &name);
        if rest.is_empty() {
            if std::fs::metadata(&next).is_ok_and(|m| m.is_file()) {
                out.push(next);
            }
        } else if std::fs::metadata(&next).is_ok_and(|m| m.is_dir()) {
            walk(&next, absolute, rest, depth + 1, out)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments_match_like_a_shell() {
        for (p, n, want) in [
            ("*.parquet", "a.parquet", true),
            ("*.parquet", "a.parquet.bak", false),
            ("*.parquet", ".parquet", true),
            ("part-?.parquet", "part-1.parquet", true),
            ("part-?.parquet", "part-12.parquet", false),
            ("part-[0-9].parquet", "part-7.parquet", true),
            ("part-[0-9].parquet", "part-x.parquet", false),
            ("part-[!0-9].parquet", "part-x.parquet", true),
            ("part-[!0-9].parquet", "part-7.parquet", false),
            ("a*b*c", "aXXbYYc", true),
            ("a*b*c", "aXXbYY", false),
            ("*", "", true),
            ("", "", true),
            ("a[", "a[", true),
            ("é*", "éa", true),
            ("[]]", "]", true),
        ] {
            assert_eq!(segment_matches(p, n), want, "{p} against {n}");
        }
    }

    #[test]
    fn a_pattern_without_glob_characters_is_one_path() {
        assert_eq!(
            expand("data/x.parquet").expect("expand"),
            vec![PathBuf::from("data/x.parquet")]
        );
    }
}
